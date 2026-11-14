//! In-memory router for the app and for tests that must not touch a real device.
//!
//! Timing uses `tokio::time::Instant`, so tests under a paused tokio clock are deterministic.
//!
//! Serving rule: the *allowed* cells are all cells when the configured LTE mask is
//! `LteBandSet::AUTO`, otherwise the cells whose band is in the mask. The serving cell is the
//! allowed cell with the best SINR. NR is attached when that cell is an `nr_anchor`, the mode is
//! Auto or NrOnly and `nr_option != Some(Sa)`. Mode `NrOnly` with option `Sa` has no LTE serving
//! cell: serving is NR (best-SINR allowed anchor) if any anchor is allowed, else none.
//! While re-attaching (after an apply or reboot) or with no allowed cell, there is no service.

use std::collections::VecDeque;
use std::net::Ipv4Addr;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::Instant;

use super::huawei::errors::kind;
use super::huawei::session::wrap_request;
use super::huawei::{apply_fields, apply_inner_xml, set_default_fields};
use crate::bands::nr::resolve_nr_band;
use crate::{
    ApplyPreview, Capabilities, ConnState, Credentials, DeviceInfo, DriverInfo, LoginGate,
    LteBandSet, LteSignal, NetConfig, NetworkMode, NrOption, NrSignal, Rat, RouterDriver,
    RouterError, RouterIdentity, SessionState, Signal, Status, WanAddress,
};

const NR_ARFCN: u32 = 528980;

/// The address ranges the mock hands out: two private and one carrier-grade NAT range.
pub const MOCK_WAN_POOLS: [[u8; 2]; 3] = [[10, 45], [10, 62], [100, 70]];

/// A made-up address in pool `pool` (wrapping), with host part `host`.
pub fn mock_wan(pool: usize, host: u16) -> Ipv4Addr {
    let [a, b] = MOCK_WAN_POOLS[pool % MOCK_WAN_POOLS.len()];
    let [c, d] = host.to_be_bytes();
    Ipv4Addr::new(a, b, c, d)
}

/// The demo's devices: made-up names in the documentation MAC range.
fn mock_devices() -> Vec<crate::privacy::LanDevice> {
    use crate::privacy::DeviceLink::{Cable, Wifi};
    let device = |name: &str, mac: &str, host: u8, active: bool, link| crate::privacy::LanDevice {
        name: Some(name.to_string()),
        mac: mac.to_string(),
        ip: Some(format!("192.168.8.{host}")),
        active: Some(active),
        private_mac: crate::privacy::is_private_mac(mac),
        link: Some(link),
        this_device: host == 100,
    };
    vec![
        device("Office PC", "00:00:5E:00:53:10", 100, true, Cable),
        device("Living-room TV", "00:00:5E:00:53:11", 101, true, Wifi),
        device("iPhone", "DA:A1:19:00:53:12", 102, true, Wifi),
        device("Galaxy Tab", "F2:3B:07:00:53:13", 103, false, Wifi),
        device("Printer", "00:00:5E:00:53:14", 104, false, Cable),
    ]
}

#[derive(Debug, Clone, PartialEq)]
pub struct MockCell {
    pub band: u8,
    pub pci: u16,
    pub cell_id: u64,
    pub rsrp: f32,
    pub rsrq: f32,
    pub sinr: f32,
    pub nr_anchor: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MockScenario {
    pub supported: LteBandSet,
    pub cells: Vec<MockCell>,
    pub reattach: Duration,
    pub initial: NetConfig,
}

impl MockScenario {
    pub fn zain_like() -> Self {
        let cell = |band, pci, rsrp, rsrq, sinr, nr_anchor| MockCell {
            band,
            pci,
            cell_id: 1_000_000 + u64::from(band),
            rsrp,
            rsrq,
            sinr,
            nr_anchor,
        };
        Self {
            supported: LteBandSet::from_huawei_hex("7E2880800D5").expect("valid mask"),
            cells: vec![
                cell(1, 101, -95.0, -11.0, 8.0, true),
                cell(3, 103, -100.0, -8.0, 0.0, true),
                cell(8, 108, -80.0, -9.0, 12.0, false),
                cell(20, 120, -85.0, -10.0, 15.0, false),
                cell(28, 128, -82.0, -9.0, 18.0, false),
                cell(40, 140, -100.0, -14.0, 10.0, false),
            ],
            reattach: Duration::from_secs(15),
            initial: NetConfig {
                network_mode: NetworkMode::Auto,
                lte_bands: LteBandSet::AUTO,
                legacy_band_raw: "0".to_string(),
                nr_option: Some(NrOption::Nsa),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    ApplyError(u32),
    /// Refuses the next band change only, as a router does when its session just expired.
    RefuseOneApply,
    VerifyMismatch,
    Unreachable,
    SessionTakeover,
    NoServiceForever,
}

struct State {
    config: NetConfig,
    applied: Vec<NetConfig>,
    fault: Option<Fault>,
    password: String,
    logged_in: bool,
    /// The router code a call answers with while logged out (`drop_session_with_code`).
    session_lost_code: Option<u32>,
    login_attempts: u32,
    reattach_until: Option<Instant>,
    /// `None` is the demo driver, which has no identity to check.
    identity: Option<RouterIdentity>,
    logouts: u32,
    logout_error: Option<RouterError>,
    logout_delay: Duration,
    wan: Option<Ipv4Addr>,
    wan_reads: u32,
    /// `wan_address()` fails with this while set.
    wan_error: Option<RouterError>,
    /// Addresses handed out at the next band re-attaches, in order; when empty the address
    /// stays (`queue_reattach_wan`).
    reattach_wan: VecDeque<Ipv4Addr>,
    apn_error: Option<RouterError>,
    /// `remote_management()` fails with this while set.
    remote_management_error: Option<RouterError>,
    /// `lan_devices()` fails with this while set.
    devices_error: Option<RouterError>,
    /// `wifi_devices()` fails with this while set.
    wifi_devices_error: Option<RouterError>,
    /// Every `wifi_devices()` call, whether it failed or not.
    wifi_device_reads: u32,
    data: DataSwitch,
    search: MockSearch,
    apn: MockApn,
    /// Every `web_page` path asked for, in order.
    web_gets: Vec<String>,
    /// Reboots and factory resets sent.
    controls: u32,
}

/// The mock's manual network search (`network_search`).
struct MockSearch {
    supported: bool,
    /// How long a search takes. No service and no internet address while it runs.
    duration: Duration,
    /// Searches are refused with this error, at once.
    refusal: Option<RouterError>,
    /// A search never answers, so the caller's timeout ends it.
    never_answers: bool,
    /// Service never comes back after a search.
    breaks_service: bool,
    /// A search also changes the band config to this, as a router might.
    sets_config: Option<NetConfig>,
    /// A search runs until then.
    until: Option<Instant>,
    /// Searches started so far.
    count: u32,
}

impl Default for MockSearch {
    fn default() -> Self {
        Self {
            supported: true,
            duration: Duration::from_secs(30),
            refusal: None,
            never_answers: false,
            breaks_service: false,
            sets_config: None,
            until: None,
            count: 0,
        }
    }
}

impl MockSearch {
    fn running(&self) -> bool {
        self.until.is_some_and(|t| Instant::now() < t) || (self.never_answers && self.count > 0)
    }
}

/// The mock's APN profiles, stored as the router would hold them.
struct MockApn {
    supported: bool,
    current: u32,
    auto_apn: bool,
    profiles: Vec<crate::apn::ApnProfile>,
    reads: u32,
    /// Switching to this profile leaves the router without data (a wrong APN).
    breaks_data: Option<u32>,
    /// The next switch POSTs that fail.
    post_failures: u32,
    /// Every accepted switch, in order.
    posts: Vec<u32>,
    /// Saved passwords by index. Only the tests read them back.
    passwords: std::collections::HashMap<u32, String>,
    /// The router shows saved passwords as stars, so "keep" can't work.
    masked_passwords: bool,
    /// The next profile write also makes this profile the one in use, as a router might.
    write_switches_to: Option<u32>,
    /// Profile writes are refused, as a router that wants an encrypted request would.
    writes_refused: bool,
    /// Every accepted add, edit and delete, as (kind, index).
    writes: Vec<(&'static str, u32)>,
}

impl Default for MockApn {
    fn default() -> Self {
        Self {
            supported: true,
            current: 1,
            auto_apn: false,
            profiles: vec![
                mock_profile(1, "Operator internet", Some("internet.example"), true),
                mock_profile(2, "Work", Some("work.example"), false),
                mock_profile(3, "Network default", None, false),
            ],
            reads: 0,
            breaks_data: None,
            post_failures: 0,
            posts: Vec::new(),
            passwords: std::collections::HashMap::new(),
            masked_passwords: false,
            write_switches_to: None,
            writes_refused: false,
            writes: Vec::new(),
        }
    }
}

impl State {
    fn apn_write_allowed(&self) -> Result<(), RouterError> {
        if self.apn.writes_refused {
            return Err(RouterError::Validation {
                message: crate::apn::APN_NEEDS_ENCRYPTION.to_string(),
            });
        }
        Ok(())
    }

    /// A router that switches its profile on a write (`write_switches_to`) does it now.
    fn apn_after_write(&mut self) {
        if let Some(i) = self.apn.write_switches_to.take() {
            self.apn.current = i;
        }
    }

    /// Data flows: the switch is on, any reconnect is over, and the APN in use works.
    fn data_up(&self) -> bool {
        self.data.up() && self.apn.breaks_data != Some(self.apn.current) && !self.search.running()
    }
}

/// A made-up profile, built the way the Huawei parser builds one.
pub fn mock_profile(
    index: u32,
    name: &str,
    apn: Option<&str>,
    read_only: bool,
) -> crate::apn::ApnProfile {
    crate::apn::RawProfile {
        index: index.to_string(),
        is_valid: "1".into(),
        name: name.into(),
        apn_is_static: if apn.is_some() { "1" } else { "0" }.into(),
        apn_name: apn.unwrap_or_default().into(),
        username: String::new(),
        has_password: false,
        auth_mode: "0".into(),
        read_only: if read_only { "2" } else { "0" }.into(),
        ip_type: "2".into(),
    }
    .build()
    .expect("a numeric index")
}

/// A profile as the router would hold it after saving `draft`.
fn draft_profile(
    index: u32,
    draft: &crate::apn::ApnDraft,
    has_password: bool,
) -> crate::apn::ApnProfile {
    crate::apn::RawProfile {
        index: index.to_string(),
        is_valid: "1".into(),
        name: draft.name.clone(),
        apn_is_static: if draft.apn.is_empty() { "0" } else { "1" }.into(),
        apn_name: draft.apn.clone(),
        username: draft.username.clone(),
        has_password,
        auth_mode: draft.auth.code().unwrap_or("0").into(),
        read_only: "0".into(),
        ip_type: draft.ip_type.code().unwrap_or("2").into(),
    }
    .build()
    .expect("a numeric index")
}

/// The mock's mobile data switch.
struct DataSwitch {
    supported: bool,
    on: bool,
    /// Data is back (and the address readable) from then on, after an "on".
    back_at: Option<Instant>,
    /// How long the mock takes to reconnect after an "on".
    reconnect: Duration,
    /// The next "on" POSTs that fail.
    on_failures: u32,
    /// Addresses handed out at the next reconnects, in order; when empty the address stays.
    next_wan: VecDeque<Ipv4Addr>,
    /// Every accepted POST, in order.
    posts: Vec<bool>,
    /// "Off" POSTs are refused with this error while set.
    off_refusal: Option<RouterError>,
}

impl Default for DataSwitch {
    fn default() -> Self {
        Self {
            supported: true,
            on: true,
            back_at: None,
            reconnect: Duration::from_secs(3),
            on_failures: 0,
            next_wan: VecDeque::new(),
            posts: Vec::new(),
            off_refusal: None,
        }
    }
}

impl DataSwitch {
    /// Data flows: the switch is on and the reconnect is over.
    fn up(&self) -> bool {
        self.on && self.back_at.is_none_or(|t| Instant::now() >= t)
    }
}

pub struct MockDriver {
    scenario: MockScenario,
    state: Mutex<State>,
}

impl MockDriver {
    pub fn new(scenario: MockScenario) -> Self {
        let state = State {
            config: scenario.initial.clone(),
            applied: Vec::new(),
            fault: None,
            password: "mock".to_string(),
            logged_in: false,
            session_lost_code: None,
            login_attempts: 0,
            reattach_until: None,
            identity: None,
            logouts: 0,
            logout_error: None,
            logout_delay: Duration::ZERO,
            wan: Some(mock_wan(0, 0x25_8E)),
            wan_reads: 0,
            wan_error: None,
            reattach_wan: VecDeque::new(),
            apn_error: None,
            remote_management_error: None,
            devices_error: None,
            wifi_devices_error: None,
            wifi_device_reads: 0,
            data: DataSwitch::default(),
            search: MockSearch::default(),
            apn: MockApn::default(),
            web_gets: Vec::new(),
            controls: 0,
        };
        Self {
            scenario,
            state: Mutex::new(state),
        }
    }

    /// Setting `SessionTakeover` also logs the mock out; clearing the fault does not log back in.
    pub fn set_fault(&self, f: Option<Fault>) {
        let mut st = self.lock();
        if f == Some(Fault::SessionTakeover) {
            st.logged_in = false;
        }
        st.fault = f;
    }

    /// Makes logins go through `LoginGate` checks, as a real router's do.
    pub fn set_identity(&self, identity: Option<RouterIdentity>) {
        self.lock().identity = identity;
    }

    /// Logs the mock out, as a router does when its session expires.
    pub fn drop_session(&self) {
        self.lock().logged_in = false;
    }

    /// Logs the mock out, and makes calls answer `SessionLost` with this router code (100003,
    /// 125002 or 125003) until the next login, as the Huawei driver reports them.
    pub fn drop_session_with_code(&self, code: u32) {
        let mut st = self.lock();
        st.logged_in = false;
        st.session_lost_code = Some(code);
    }

    /// Number of `logout()` calls so far, including failed ones.
    pub fn logouts(&self) -> u32 {
        self.lock().logouts
    }

    /// Whether the mock holds a logged-in session.
    pub fn is_logged_in(&self) -> bool {
        self.lock().logged_in
    }

    /// Makes `logout()` fail with this error (the session stays), or succeed again with `None`.
    pub fn set_logout_error(&self, e: Option<RouterError>) {
        self.lock().logout_error = e;
    }

    /// Makes `logout()` take this long before it answers.
    pub fn set_logout_delay(&self, d: Duration) {
        self.lock().logout_delay = d;
    }

    /// Number of `login()` calls so far, including failed ones.
    pub fn login_attempts(&self) -> u32 {
        self.lock().login_attempts
    }

    /// Configs that were actually stored by a successful apply (not errors, not
    /// `VerifyMismatch` applies, which leave the stored config untouched).
    pub fn applied(&self) -> Vec<NetConfig> {
        self.lock().applied.clone()
    }

    /// The WAN address the mock reports; `None` reads as no data connection.
    pub fn set_wan(&self, ip: Option<Ipv4Addr>) {
        self.lock().wan = ip;
    }

    /// Makes `wan_address()` fail with `e` until cleared.
    pub fn set_wan_error(&self, e: Option<RouterError>) {
        self.lock().wan_error = e;
    }

    /// Makes `apn_profiles()` fail with `e` until cleared.
    pub fn set_apn_error(&self, e: Option<RouterError>) {
        self.lock().apn_error = e;
    }

    /// Makes `remote_management()` fail with `e` until cleared.
    pub fn set_remote_management_error(&self, e: Option<RouterError>) {
        self.lock().remote_management_error = e;
    }

    /// Makes `lan_devices()` fail with `e` until cleared.
    pub fn set_devices_error(&self, e: Option<RouterError>) {
        self.lock().devices_error = e;
    }

    /// Makes `wifi_devices()` fail with `e` until cleared.
    pub fn set_wifi_devices_error(&self, e: Option<RouterError>) {
        self.lock().wifi_devices_error = e;
    }

    /// How often `wifi_devices()` was asked for, failures included.
    pub fn wifi_device_reads(&self) -> u32 {
        self.lock().wifi_device_reads
    }

    /// Number of successful `wan_address()` calls so far.
    pub fn wan_reads(&self) -> u32 {
        self.lock().wan_reads
    }

    /// Whether the mock has a mobile data switch (on by default).
    pub fn set_data_switch_supported(&self, on: bool) {
        self.lock().data.supported = on;
    }

    /// Sets the switch directly, as the router's own web page would.
    pub fn set_data_on(&self, on: bool) {
        let mut st = self.lock();
        st.data.on = on;
        st.data.back_at = None;
    }

    pub fn data_on(&self) -> bool {
        self.lock().data.on
    }

    /// Makes the next `n` "on" POSTs fail.
    pub fn fail_data_on(&self, n: u32) {
        self.lock().data.on_failures = n;
    }

    /// How long data stays down after an "on".
    pub fn set_reconnect(&self, d: Duration) {
        self.lock().data.reconnect = d;
    }

    /// Addresses for the next reconnects, in order. Once they run out the address stays.
    pub fn queue_wan(&self, ips: &[Ipv4Addr]) {
        self.lock().data.next_wan.extend(ips.iter().copied());
    }

    /// Addresses for the next band changes the mock applies, in order, as a real network gives
    /// a new address when the router re-attaches. Once they run out the address stays.
    pub fn queue_reattach_wan(&self, ips: &[Ipv4Addr]) {
        self.lock().reattach_wan.extend(ips.iter().copied());
    }

    /// Every accepted data switch POST so far (`false` is off).
    pub fn switch_posts(&self) -> Vec<bool> {
        self.lock().data.posts.clone()
    }

    /// Makes "off" POSTs fail with `e` (the switch stays on) until cleared.
    pub fn refuse_data_off(&self, e: Option<RouterError>) {
        self.lock().data.off_refusal = e;
    }

    /// Whether the mock has a manual network search (on by default).
    pub fn set_network_search_supported(&self, on: bool) {
        self.lock().search.supported = on;
    }

    /// How long a network search takes (30 s by default).
    pub fn set_search_duration(&self, d: Duration) {
        self.lock().search.duration = d;
    }

    /// Makes network searches fail at once with `e` until cleared.
    pub fn refuse_search(&self, e: Option<RouterError>) {
        self.lock().search.refusal = e;
    }

    /// Network searches never answer, as a router that hangs on one.
    pub fn search_never_answers(&self, on: bool) {
        self.lock().search.never_answers = on;
    }

    /// Service never comes back after the next search.
    pub fn search_breaks_service(&self, on: bool) {
        self.lock().search.breaks_service = on;
    }

    /// The next search also sets the band config to `cfg`.
    pub fn search_sets_config(&self, cfg: Option<NetConfig>) {
        self.lock().search.sets_config = cfg;
    }

    /// Network searches started so far.
    pub fn searches(&self) -> u32 {
        self.lock().search.count
    }

    /// Every `web_page` path asked for so far.
    pub fn web_gets(&self) -> Vec<String> {
        self.lock().web_gets.clone()
    }

    /// Every write the mock has taken so far: band and APN changes, data-switch POSTs, profile
    /// writes, reboots and factory resets. Reads don't count.
    pub fn writes(&self) -> usize {
        let st = self.lock();
        st.applied.len()
            + st.data.posts.len()
            + st.apn.posts.len()
            + st.apn.writes.len()
            + st.controls as usize
    }

    /// Whether the mock has APN profiles (on by default).
    pub fn set_apn_supported(&self, on: bool) {
        self.lock().apn.supported = on;
    }

    /// Turns the router's own Auto APN on or off.
    pub fn set_auto_apn(&self, on: bool) {
        self.lock().apn.auto_apn = on;
    }

    /// Replaces the profiles and the one in use, as the router's own web page would.
    pub fn set_apn_profiles(&self, current: u32, profiles: Vec<crate::apn::ApnProfile>) {
        let mut st = self.lock();
        st.apn.current = current;
        st.apn.profiles = profiles;
    }

    /// The index of the profile in use.
    pub fn current_apn(&self) -> u32 {
        self.lock().apn.current
    }

    /// Switching to profile `index` leaves the router without data, as a wrong APN does.
    pub fn apn_breaks_data(&self, index: Option<u32>) {
        self.lock().apn.breaks_data = index;
    }

    /// Makes the next `n` switch POSTs fail.
    pub fn apn_post_fails(&self, n: u32) {
        self.lock().apn.post_failures = n;
    }

    /// Every accepted switch so far, by index.
    pub fn apn_posts(&self) -> Vec<u32> {
        self.lock().apn.posts.clone()
    }

    /// Sets a saved password, as the router's own web page would.
    pub fn set_apn_password(&self, index: u32, password: &str) {
        let mut st = self.lock();
        st.apn.passwords.insert(index, password.to_string());
        if let Some(p) = st.apn.profiles.iter_mut().find(|p| p.index == index) {
            p.has_password = !password.is_empty();
        }
    }

    /// The saved password of profile `index` (tests only).
    pub fn apn_password(&self, index: u32) -> Option<String> {
        self.lock().apn.passwords.get(&index).cloned()
    }

    /// The router shows saved passwords as stars.
    pub fn apn_masks_passwords(&self, on: bool) {
        self.lock().apn.masked_passwords = on;
    }

    /// The next profile write also switches the profile in use to `index`.
    pub fn apn_write_switches_to(&self, index: Option<u32>) {
        self.lock().apn.write_switches_to = index;
    }

    /// Profile writes are refused, as on a router that wants an encrypted request.
    pub fn apn_refuses_writes(&self, on: bool) {
        self.lock().apn.writes_refused = on;
    }

    /// Every accepted add, edit and delete so far, as (kind, index).
    pub fn apn_writes(&self) -> Vec<(&'static str, u32)> {
        self.lock().apn.writes.clone()
    }

    /// Number of successful `apn_profiles()` calls so far.
    pub fn apn_reads(&self) -> u32 {
        self.lock().apn.reads
    }

    pub fn set_password(&self, p: &str) {
        self.lock().password = p.to_string();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Faults that make (almost) every call fail.
    fn gate(&self) -> Result<(), RouterError> {
        match self.lock().fault {
            Some(Fault::Unreachable) => Err(RouterError::Unreachable),
            Some(Fault::SessionTakeover) => Err(RouterError::SessionTakenOver),
            _ => Ok(()),
        }
    }

    /// Fault gate, then require a logged-in session (`SessionLost` otherwise).
    fn authed(&self) -> Result<(), RouterError> {
        self.gate()?;
        let st = self.lock();
        if st.logged_in {
            Ok(())
        } else {
            Err(RouterError::SessionLost {
                code: st.session_lost_code,
            })
        }
    }

    fn start_reattach(&self, st: &mut State) {
        st.reattach_until = Some(Instant::now() + self.scenario.reattach);
    }

    /// Current signal, per the rule in the module docs.
    fn compute_signal(&self) -> Signal {
        let st = self.lock();
        let none = Signal {
            serving: None,
            lte: None,
            nr: None,
        };
        if st.fault == Some(Fault::NoServiceForever) || st.search.running() {
            return none;
        }
        if st.reattach_until.is_some_and(|t| Instant::now() < t) {
            return none;
        }
        let cfg = &st.config;
        let best = |anchor_only: bool| {
            self.scenario
                .cells
                .iter()
                .filter(|c| cfg.lte_bands == LteBandSet::AUTO || cfg.lte_bands.contains(c.band))
                .filter(|c| !anchor_only || c.nr_anchor)
                .max_by(|a, b| a.sinr.total_cmp(&b.sinr))
        };
        let nr_signal = |c: &MockCell| NrSignal {
            arfcn_dl: Some(NR_ARFCN),
            arfcn_ul: Some(NR_ARFCN),
            band: resolve_nr_band(NR_ARFCN, Some(NR_ARFCN)),
            pci: Some(c.pci),
            rsrp: Some(c.rsrp),
            rsrq: Some(c.rsrq),
            sinr: Some(c.sinr),
            tx_power: None,
            dl_bw_mhz: None,
            ul_bw_mhz: None,
            dl_mcs: [None; 2],
            ul_mcs: None,
            cqi: [None; 2],
        };
        let sa = cfg.nr_option == Some(NrOption::Sa);
        if cfg.network_mode == NetworkMode::NrOnly && sa {
            return match best(true) {
                Some(c) => Signal {
                    serving: Some(Rat::Nr),
                    lte: None,
                    nr: Some(nr_signal(c)),
                },
                None => none,
            };
        }
        let Some(c) = best(false) else { return none };
        let nr_ok = c.nr_anchor
            && matches!(cfg.network_mode, NetworkMode::Auto | NetworkMode::NrOnly)
            && !sa;
        Signal {
            serving: Some(Rat::Lte),
            lte: Some(LteSignal {
                band: Some(u16::from(c.band)),
                earfcn_dl: None,
                earfcn_ul: None,
                pci: Some(c.pci),
                cell_id: Some(c.cell_id),
                enb_id: None,
                sector: None,
                rsrp: Some(c.rsrp),
                rsrq: Some(c.rsrq),
                sinr: Some(c.sinr),
                rssi: None,
                tx_power: None,
                dl_bw_mhz: None,
                ul_bw_mhz: None,
                dl_mcs: [None; 2],
                ul_mcs: None,
                cqi: [None; 2],
                scc_pci: None,
            }),
            nr: nr_ok.then(|| nr_signal(c)),
        }
    }
}

#[async_trait]
impl RouterDriver for MockDriver {
    fn info(&self) -> DriverInfo {
        DriverInfo {
            id: "mock",
            name: "Mock router",
        }
    }

    fn apply_preview(&self, cfg: &NetConfig) -> ApplyPreview {
        let fields = apply_fields(cfg);
        let body = wrap_request(&apply_inner_xml(&fields));
        ApplyPreview { fields, body }
    }

    /// Both `Unreachable` and `SessionTakeover` faults also fail login.
    async fn login(&self, creds: &Credentials) -> Result<(), RouterError> {
        self.lock().login_attempts += 1;
        self.gate()?;
        let mut st = self.lock();
        if creds.password() != st.password {
            return Err(RouterError::BadCredentials);
        }
        st.logged_in = true;
        st.session_lost_code = None;
        Ok(())
    }

    /// With an identity set, a refused gate stops before the attempt, as the proof never goes out.
    async fn login_gated(
        &self,
        creds: &Credentials,
        gate: &LoginGate,
    ) -> Result<Option<RouterIdentity>, RouterError> {
        let identity = self.lock().identity.clone();
        if let Some(seen) = &identity {
            gate.check(seen)?;
        }
        self.login(creds).await.map(|()| identity)
    }

    async fn logout(&self) -> Result<(), RouterError> {
        let delay = {
            let mut st = self.lock();
            st.logouts += 1;
            st.logout_delay
        };
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        self.gate()?;
        let mut st = self.lock();
        if let Some(e) = st.logout_error.clone() {
            return Err(e);
        }
        st.logged_in = false;
        Ok(())
    }

    async fn session_state(&self) -> Result<SessionState, RouterError> {
        self.gate()?;
        Ok(if self.lock().logged_in {
            SessionState::LoggedIn
        } else {
            SessionState::LoggedOut
        })
    }

    async fn heartbeat(&self) -> Result<(), RouterError> {
        self.authed()
    }

    async fn capabilities(&self) -> Result<Capabilities, RouterError> {
        self.gate()?;
        let (data_switch, search, apn_read) = {
            let st = self.lock();
            (st.data.supported, st.search.supported, st.apn.supported)
        };
        let mut ip_methods = Vec::new();
        if data_switch {
            ip_methods.push(crate::IpMethod::DataSwitch);
        }
        if search {
            ip_methods.push(crate::IpMethod::NetworkSearch);
        }
        Ok(Capabilities {
            supported_lte: self.scenario.supported,
            modes: vec![NetworkMode::Auto, NetworkMode::NrOnly, NetworkMode::LteOnly],
            nr_options: vec![NrOption::Nsa, NrOption::Sa, NrOption::SaNsa],
            band_edit_allowed: true,
            factory_reset_verified: true,
            data_switch,
            ip_methods,
            apn: crate::apn::ApnSupport {
                read: apn_read,
                switch: apn_read,
                edit: apn_read,
                confirmed: true,
            },
        })
    }

    async fn device_info(&self) -> Result<DeviceInfo, RouterError> {
        self.authed()?;
        Ok(DeviceInfo {
            model: "Mock H122-373".to_string(),
            firmware: "mock-11.0.2.21".to_string(),
            web_ui: "mock".to_string(),
            home_plmn: None,
        })
    }

    async fn status(&self) -> Result<Status, RouterError> {
        self.authed()?;
        let sig = self.compute_signal();
        let up = sig.serving.is_some();
        let data_up = self.lock().data_up();
        Ok(Status {
            connection: match (up, data_up) {
                (true, true) => ConnState::Connected,
                (true, false) => ConnState::Disconnected,
                (false, _) => ConnState::NoService,
            },
            network_type: if sig.nr.is_some() { 20 } else { 19 },
            operator: None,
            bars: up.then_some(4),
            nr_bars: sig.nr.is_some().then_some(4),
            endc: Some(sig.nr.is_some()),
            endc_restricted: Some(false),
        })
    }

    async fn signal(&self) -> Result<Signal, RouterError> {
        self.authed()?;
        Ok(self.compute_signal())
    }

    async fn net_config(&self) -> Result<NetConfig, RouterError> {
        self.authed()?;
        Ok(self.lock().config.clone())
    }

    async fn wan_address(&self) -> Result<WanAddress, RouterError> {
        self.authed()?;
        let mut st = self.lock();
        if let Some(e) = st.wan_error.clone() {
            return Err(e);
        }
        st.wan_reads += 1;
        Ok(WanAddress {
            ipv4: st.wan.filter(|_| st.data_up()),
            ipv6_present: false,
        })
    }

    /// A made-up provider server, as an operator-supplied router would show.
    async fn remote_management(
        &self,
    ) -> Result<crate::privacy::Seen<crate::privacy::RemoteManagement>, RouterError> {
        self.authed()?;
        if let Some(e) = self.lock().remote_management_error.clone() {
            return Err(e);
        }
        Ok(crate::privacy::Seen {
            value: crate::privacy::RemoteManagement {
                server_host: Some("acs.example.net".to_string()),
                server_set: Some(true),
                enabled: Some(true),
            },
            tags: ["Enable", "AcsUrl", "AcsUserName", "PeriodicInformInterval"]
                .map(String::from)
                .to_vec(),
        })
    }

    /// Five made-up devices (documentation MAC range): two offline, two with a private address.
    async fn lan_devices(
        &self,
    ) -> Result<crate::privacy::Seen<Vec<crate::privacy::LanDevice>>, RouterError> {
        self.authed()?;
        if let Some(e) = self.lock().devices_error.clone() {
            return Err(e);
        }
        Ok(crate::privacy::Seen {
            value: mock_devices(),
            tags: [
                "Hosts",
                "Host",
                "ID",
                "MacAddress",
                "IpAddress",
                "HostName",
                "ActualName",
                "Active",
            ]
            .map(String::from)
            .to_vec(),
        })
    }

    /// The same devices that are online, as the Wi-Fi list gives them: without an online state.
    async fn wifi_devices(
        &self,
    ) -> Result<crate::privacy::Seen<Vec<crate::privacy::LanDevice>>, RouterError> {
        self.authed()?;
        let mut st = self.lock();
        st.wifi_device_reads += 1;
        if let Some(e) = st.wifi_devices_error.clone() {
            return Err(e);
        }
        Ok(crate::privacy::Seen {
            value: mock_devices()
                .into_iter()
                .filter(|d| d.active == Some(true))
                .map(|d| crate::privacy::LanDevice { active: None, ..d })
                .collect(),
            tags: [
                "Hosts",
                "Host",
                "ID",
                "MacAddress",
                "IpAddress",
                "HostName",
                "AssociatedSsid",
            ]
            .map(String::from)
            .to_vec(),
        })
    }

    async fn data_switch(&self) -> Result<bool, RouterError> {
        self.authed()?;
        let st = self.lock();
        if !st.data.supported {
            return Err(RouterError::Api {
                code: 100002,
                kind: kind(100002),
            });
        }
        Ok(st.data.on)
    }

    async fn set_data_switch(&self, on: bool) -> Result<(), RouterError> {
        self.authed()?;
        let mut st = self.lock();
        if !st.data.supported {
            return Err(RouterError::Api {
                code: 100002,
                kind: kind(100002),
            });
        }
        if !on {
            if let Some(e) = st.data.off_refusal.clone() {
                return Err(e);
            }
        }
        if on && st.data.on_failures > 0 {
            st.data.on_failures -= 1;
            return Err(RouterError::Api {
                code: 100004,
                kind: kind(100004),
            });
        }
        st.data.posts.push(on);
        if on && !st.data.on {
            st.data.back_at = Some(Instant::now() + st.data.reconnect);
            if let Some(ip) = st.data.next_wan.pop_front() {
                st.wan = Some(ip);
            }
        }
        st.data.on = on;
        Ok(())
    }

    /// No service and no address for the search's duration, then the next queued address.
    async fn network_search(&self) -> Result<crate::NetworkSearch, RouterError> {
        self.authed()?;
        let (duration, never) = {
            let mut st = self.lock();
            if !st.search.supported {
                return Err(RouterError::Api {
                    code: 100002,
                    kind: kind(100002),
                });
            }
            if let Some(e) = st.search.refusal.clone() {
                return Err(e);
            }
            st.search.count += 1;
            st.search.until = Some(Instant::now() + st.search.duration);
            if let Some(cfg) = st.search.sets_config.take() {
                st.config = cfg;
            }
            (st.search.duration, st.search.never_answers)
        };
        if never {
            std::future::pending::<()>().await;
        }
        tokio::time::sleep(duration).await;
        let mut st = self.lock();
        st.search.until = None;
        if let Some(ip) = st.data.next_wan.pop_front() {
            st.wan = Some(ip);
        }
        if st.search.breaks_service {
            st.fault = Some(Fault::NoServiceForever);
        }
        Ok(crate::NetworkSearch { networks: Some(3) })
    }

    async fn apn_profiles(&self) -> Result<crate::apn::ApnProfiles, RouterError> {
        self.authed()?;
        let mut st = self.lock();
        if let Some(e) = st.apn_error.clone() {
            return Err(e);
        }
        if !st.apn.supported {
            return Err(RouterError::Api {
                code: 100002,
                kind: kind(100002),
            });
        }
        st.apn.reads += 1;
        Ok(crate::apn::ApnProfiles {
            current: Some(st.apn.current),
            profiles: st.apn.profiles.clone(),
            auto_apn: Some(st.apn.auto_apn),
        })
    }

    fn apn_preview(&self, target: &crate::apn::ApnTarget) -> ApplyPreview {
        let fields = set_default_fields(target);
        let body = wrap_request(&apply_inner_xml(&fields));
        ApplyPreview { fields, body }
    }

    /// Checks the target against its own profiles first, as the Huawei driver does. An accepted
    /// switch drops data for the reconnect time.
    async fn set_default_apn(&self, target: &crate::apn::ApnTarget) -> Result<(), RouterError> {
        let profiles = self.apn_profiles().await?;
        crate::apn::check_target(&profiles, target)?;
        let mut st = self.lock();
        if st.apn.post_failures > 0 {
            st.apn.post_failures -= 1;
            return Err(RouterError::Api {
                code: 100004,
                kind: kind(100004),
            });
        }
        st.apn.posts.push(target.index);
        st.apn.current = target.index;
        st.data.back_at = Some(Instant::now() + st.data.reconnect);
        Ok(())
    }

    /// The same checks as the Huawei driver, before and after the write.
    async fn add_apn_profile(
        &self,
        expect_current: &crate::apn::ApnTarget,
        draft: &crate::apn::ApnDraft,
    ) -> Result<u32, RouterError> {
        let draft = draft.checked(true)?;
        let before = self.apn_profiles().await?;
        crate::apn::check_write(&before, expect_current, None)?;
        let password = draft.password_text(None)?;
        {
            let mut st = self.lock();
            st.apn_write_allowed()?;
            let index = st.apn.profiles.iter().map(|p| p.index).max().unwrap_or(0) + 1;
            let profile = draft_profile(index, &draft, !password.is_empty());
            st.apn.profiles.push(profile);
            st.apn.passwords.insert(index, password);
            st.apn.writes.push(("add", index));
            st.apn_after_write();
        }
        let after = self.apn_profiles().await?;
        crate::apn::check_after_write(&after, expect_current)?;
        crate::apn::added_index(&before, &after, &draft)
    }

    async fn edit_apn_profile(
        &self,
        expect_current: &crate::apn::ApnTarget,
        target: &crate::apn::ApnTarget,
        draft: &crate::apn::ApnDraft,
    ) -> Result<(), RouterError> {
        let draft = draft.checked(false)?;
        let before = self.apn_profiles().await?;
        crate::apn::check_write(&before, expect_current, Some(target))?;
        {
            let mut st = self.lock();
            let saved = if st.apn.masked_passwords {
                "******".to_string()
            } else {
                st.apn
                    .passwords
                    .get(&target.index)
                    .cloned()
                    .unwrap_or_default()
            };
            let password = draft.password_text(Some(&saved))?;
            st.apn_write_allowed()?;
            let profile = draft_profile(target.index, &draft, !password.is_empty());
            if let Some(p) = st.apn.profiles.iter_mut().find(|p| p.index == target.index) {
                *p = profile;
            }
            st.apn.passwords.insert(target.index, password);
            st.apn.writes.push(("edit", target.index));
            st.apn_after_write();
        }
        let after = self.apn_profiles().await?;
        crate::apn::check_after_write(&after, expect_current)?;
        crate::apn::check_edited(&after, target.index, &draft)
    }

    async fn delete_apn_profile(
        &self,
        expect_current: &crate::apn::ApnTarget,
        target: &crate::apn::ApnTarget,
    ) -> Result<(), RouterError> {
        let before = self.apn_profiles().await?;
        crate::apn::check_write(&before, expect_current, Some(target))?;
        {
            let mut st = self.lock();
            st.apn_write_allowed()?;
            st.apn.profiles.retain(|p| p.index != target.index);
            st.apn.passwords.remove(&target.index);
            st.apn.writes.push(("delete", target.index));
            st.apn_after_write();
        }
        let after = self.apn_profiles().await?;
        crate::apn::check_after_write(&after, expect_current)?;
        crate::apn::check_deleted(&after, target.index)
    }

    async fn apply_net_config(&self, cfg: &NetConfig) -> Result<(), RouterError> {
        self.authed()?;
        let mut st = self.lock();
        match st.fault {
            Some(Fault::ApplyError(code)) => Err(RouterError::Api {
                code,
                kind: kind(code),
            }),
            Some(Fault::RefuseOneApply) => {
                st.fault = None;
                Err(RouterError::Api {
                    code: 100005,
                    kind: kind(100005),
                })
            }
            Some(Fault::VerifyMismatch) => Ok(()),
            _ => {
                st.config = cfg.clone();
                st.applied.push(cfg.clone());
                self.start_reattach(&mut st);
                if let Some(ip) = st.reattach_wan.pop_front() {
                    st.wan = Some(ip);
                }
                Ok(())
            }
        }
    }

    async fn reboot(&self) -> Result<(), RouterError> {
        self.authed()?;
        let mut st = self.lock();
        st.controls += 1;
        self.start_reattach(&mut st);
        st.logged_in = false;
        Ok(())
    }

    async fn factory_reset(&self) -> Result<(), RouterError> {
        self.authed()?;
        let mut st = self.lock();
        st.controls += 1;
        self.start_reattach(&mut st);
        st.logged_in = false;
        Ok(())
    }

    /// The synthetic pages in `mock_web_page`; an unknown name gets the router's short error
    /// page, as the H122 answers. Logged out is the redirect to the login page.
    async fn web_page(&self, path: &str) -> Result<Vec<u8>, RouterError> {
        if !crate::page_scripts::is_allowed_path(path) {
            return Err(RouterError::Validation {
                message: crate::page_scripts::PAGE_PATH_REFUSED.to_string(),
            });
        }
        self.gate()?;
        let mut st = self.lock();
        if !st.logged_in {
            return Err(RouterError::Validation {
                message: crate::page_scripts::PAGE_NOT_LOGGED_IN.to_string(),
            });
        }
        st.web_gets.push(path.to_string());
        Ok(mock_web_page(path).unwrap_or_else(mock_error_page))
    }

    /// Router-shaped bodies with made-up identifiers, so the demo shows what redaction does.
    async fn diagnostic_reads(&self) -> Vec<crate::diagnostics::EndpointRead> {
        let ok = |path: &str, body: &str| crate::diagnostics::EndpointRead {
            path: path.to_string(),
            result: Ok(body.to_string()),
        };
        vec![
            ok(
                "/api/device/information",
                "<response><DeviceName>Mock H122-373</DeviceName><SerialNumber>MOCK0000SERIAL01</SerialNumber><Imei>000000000000000</Imei><MacAddress1>00:11:22:33:44:55</MacAddress1><SoftwareVersion>0.0.0.0(mock)</SoftwareVersion></response>",
            ),
            ok(
                "/api/device/signal",
                "<response><pci>103</pci><cell_id>11111111</cell_id><enodeb_id>222222</enodeb_id><band>3</band><rsrp>-92dBm</rsrp><sinr>-1dB</sinr></response>",
            ),
            crate::diagnostics::EndpointRead {
                path: "/api/net/current-plmn".to_string(),
                result: Err(RouterError::SessionLost { code: None }),
            },
        ]
    }
}

/// The router's 88-byte answer for a file it doesn't have.
pub fn mock_error_page() -> Vec<u8> {
    let mut page =
        b"<html><head><title>error</title></head><body>Page not found</body></html>".to_vec();
    page.resize(88, b'\n');
    page
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    // Writing to a Vec can't fail.
    let _ = enc.write_all(bytes);
    enc.finish().unwrap_or_default()
}

/// A small made-up web UI for the page-script saver: written for the app's tests, not taken from
/// any router. It has a CSRF meta, a fake IMEI-length number, a gzip body, a missing script, a
/// `../` and an encoded `%2e%2e` link, links to other hosts, a file that only the stricter last
/// scan catches, and a chain deeper than the crawler goes.
pub fn mock_web_page(path: &str) -> Option<Vec<u8>> {
    let text: &str = match path {
        "/html/content.html" => concat!(
            "<!DOCTYPE html>\n<html><head>\n",
            "<meta name=\"csrf_token\" content=\"MockCsrfToken0123456789abcdefABCD\">\n",
            "<meta name=\"csrf_token\" content=\"MockCsrfToken9876543210fedcbaDCBA\">\n",
            "<script type=\"text/javascript\" src=\"/js/main.js\"></script>\n",
            "<script src=\"/lib/mock-lib.js?v=2#top\"></script>\n",
            "<script src=\"../js/outside.js\"></script>\n",
            "<script src=\"/js/%2e%2e/escape.js\"></script>\n",
            "<script src=\"http://203.0.113.9/js/elsewhere.js\"></script>\n",
            "<script src=\"//mock.example/js/elsewhere.js\"></script>\n",
            "</head><body><a href=\"/html/settings.html\">Settings</a></body></html>\n",
        ),
        "/js/main.js" => concat!(
            "// Mock web UI script, made up for the app's tests.\n",
            "var demoImei = \"000000000000000\";\n",
            "function load(p) { return p; }\n",
            "load('/js/missing.js');\n",
            "load(\"/js/hidden.js\");\n",
        ),
        "/js/hidden.js" => "var note = \"line\\n000000000000001\";\n",
        "/html/settings.html" => {
            "<html><body><script src=\"/js/settings.js\"></script></body></html>\n"
        }
        "/js/settings.js" => "var next = '/js/deep.js';\n",
        "/js/deep.js" => "var next = '/js/too-deep.js';\n",
        "/js/too-deep.js" => "var end = true;\n",
        "/lib/mock-lib.js" => {
            return Some(gzip(
                b"/* Mock library, made up for the app's tests. */\nfunction mockLib() { return 'lib'; }\n",
            ))
        }
        _ => return None,
    };
    Some(text.as_bytes().to_vec())
}
