//! RouterActor end-to-end tests against the mock router, under a paused tokio clock.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{broadcast, oneshot, watch};
use tokio::time::{sleep, Instant};

use router_core::actor::{
    spawn_actor, ActorConfig, ActorHandle, Alert, AppSnapshot, AutomationConfig, AutomationEvent,
    CellKey, Command, IpRefreshAlert, LogoutOutcome, RebootTicket, SessionView, WidenFallback,
    DATA_JOURNAL_BLOCKED, IP_NOT_IN_SERVICE, IP_REFRESH_RUNNING, NOT_CONNECTED, REBOOT_PENDING,
};
use router_core::drivers::mock::{mock_wan, Fault, MockDriver, MockScenario};
use router_core::identity_file;
use router_core::ip_refresh_file::{
    DataSwitchPhase, DataSwitchRecord, DataSwitchStore, DATA_SWITCH_VERSION,
};
use router_core::journal::{Journal, JournalPhase, JournalRecord, JOURNAL_VERSION};
use router_core::logout_marker;
use router_core::LoginGate;
use router_core::{
    ApiErrorKind, ApplyPreview, Capabilities, Credentials, DeviceInfo, DriverInfo, LteBandSet,
    NetConfig, NetworkMode, NrOption, Phase, Rat, RevertReason, RouterDriver, RouterError,
    RouterIdentity, SafeApplyEvent, SafeApplyTimers, SequenceOrigin, SequenceRecord, SequenceStore,
    SessionState, Signal, Status, WanAddress, WanKind, CHANGE_IN_PROGRESS, SEQUENCE_VERSION,
    SPEED_TEST_RUNNING,
};
use router_core::{
    IpReattachOutcome, IpReattachResult, IpRefreshOutcome, IpRefreshPhase, IP_REATTACH_COOLDOWN,
    IP_REATTACH_UNDO_OFF, PAGE_SCRIPTS_RUNNING,
};

const HOST: &str = "192.168.8.1";

// ---------------------------------------------------------------------------------------------
// Counting decorator around the mock (test-only; the production mock has no counters).

#[derive(Default)]
struct Counts {
    login: AtomicU32,
    heartbeat: AtomicU32,
    status: AtomicU32,
    signal: AtomicU32,
    net_config: AtomicU32,
    apply: AtomicU32,
    reboot: AtomicU32,
    factory_reset: AtomicU32,
    logout: AtomicU32,
    wan: AtomicU32,
    switch: AtomicU32,
    apn: AtomicU32,
    apn_switch: AtomicU32,
    apn_write: AtomicU32,
    search: AtomicU32,
    other: AtomicU32,
}

impl Counts {
    fn total(&self) -> u32 {
        [
            &self.login,
            &self.heartbeat,
            &self.status,
            &self.signal,
            &self.net_config,
            &self.apply,
            &self.reboot,
            &self.factory_reset,
            &self.logout,
            &self.wan,
            &self.switch,
            &self.apn,
            &self.apn_switch,
            &self.apn_write,
            &self.search,
            &self.other,
        ]
        .iter()
        .map(|c| c.load(Ordering::SeqCst))
        .sum()
    }

    fn reset(&self) {
        for c in [
            &self.login,
            &self.heartbeat,
            &self.status,
            &self.signal,
            &self.net_config,
            &self.apply,
            &self.reboot,
            &self.factory_reset,
            &self.logout,
            &self.wan,
            &self.switch,
            &self.apn,
            &self.apn_switch,
            &self.apn_write,
            &self.search,
            &self.other,
        ] {
            c.store(0, Ordering::SeqCst);
        }
    }
}

fn get(c: &AtomicU32) -> u32 {
    c.load(Ordering::SeqCst)
}

fn bump(c: &AtomicU32) {
    c.fetch_add(1, Ordering::SeqCst);
}

/// Replaces the serving LTE cell's (pci, cell_id) for the next `remaining` signal polls.
struct CellOverride {
    pci: u16,
    cell_id: u64,
    remaining: u32,
}

struct Probe {
    mock: Arc<MockDriver>,
    counts: Counts,
    cell: Mutex<Option<CellOverride>>,
    /// Huawei-style session loss: every data call answers `<error><code>100003`.
    no_rights: AtomicBool,
    /// `signal` answers `<error><code>100004` (busy).
    busy: AtomicBool,
    /// `capabilities` answers `<error><code>100004` (busy).
    caps_busy: AtomicBool,
    /// `login` fails with this error instead of reaching the mock.
    login_error: Mutex<Option<RouterError>>,
    /// `signal` hangs for 30 s, like the H122 web server during a re-attach.
    slow_signal: AtomicBool,
    /// Reports this serving LTE band instead of the mock's.
    band_override: Mutex<Option<u16>>,
    /// `apply_net_config` of this config answers OK but changes nothing.
    swallow_apply: Mutex<Option<NetConfig>>,
    /// `apply_net_config` of this config is refused (API error).
    refuse_apply: Mutex<Option<NetConfig>>,
}

fn api(code: u32, kind: ApiErrorKind) -> RouterError {
    RouterError::Api { code, kind }
}

impl Probe {
    fn new(mock: Arc<MockDriver>) -> Arc<Self> {
        Arc::new(Self {
            mock,
            counts: Counts::default(),
            cell: Mutex::new(None),
            no_rights: AtomicBool::new(false),
            busy: AtomicBool::new(false),
            caps_busy: AtomicBool::new(false),
            login_error: Mutex::new(None),
            slow_signal: AtomicBool::new(false),
            band_override: Mutex::new(None),
            swallow_apply: Mutex::new(None),
            refuse_apply: Mutex::new(None),
        })
    }

    fn data_gate(&self) -> Result<(), RouterError> {
        if self.no_rights.load(Ordering::SeqCst) {
            Err(api(100003, ApiErrorKind::NoRights))
        } else {
            Ok(())
        }
    }

    fn override_cell(&self, pci: u16, cell_id: u64, polls: u32) {
        *self.cell.lock().unwrap() = Some(CellOverride {
            pci,
            cell_id,
            remaining: polls,
        });
    }
}

#[async_trait]
impl RouterDriver for Probe {
    fn info(&self) -> DriverInfo {
        self.mock.info()
    }
    fn apply_preview(&self, cfg: &NetConfig) -> ApplyPreview {
        self.mock.apply_preview(cfg)
    }
    async fn login(&self, creds: &Credentials) -> Result<(), RouterError> {
        bump(&self.counts.login);
        let forced = self.login_error.lock().unwrap().clone();
        if let Some(e) = forced {
            return Err(e);
        }
        self.mock.login(creds).await
    }
    async fn login_gated(
        &self,
        creds: &Credentials,
        gate: &LoginGate,
    ) -> Result<Option<RouterIdentity>, RouterError> {
        bump(&self.counts.login);
        let forced = self.login_error.lock().unwrap().clone();
        if let Some(e) = forced {
            return Err(e);
        }
        self.mock.login_gated(creds, gate).await
    }
    async fn logout(&self) -> Result<(), RouterError> {
        bump(&self.counts.logout);
        self.mock.logout().await
    }
    fn forget_session(&self) {
        self.mock.forget_session();
    }
    async fn session_state(&self) -> Result<SessionState, RouterError> {
        bump(&self.counts.other);
        self.mock.session_state().await
    }
    async fn heartbeat(&self) -> Result<(), RouterError> {
        bump(&self.counts.heartbeat);
        self.data_gate()?;
        self.mock.heartbeat().await
    }
    async fn capabilities(&self) -> Result<Capabilities, RouterError> {
        bump(&self.counts.other);
        self.data_gate()?;
        if self.caps_busy.load(Ordering::SeqCst) {
            return Err(api(100004, ApiErrorKind::Busy));
        }
        self.mock.capabilities().await
    }
    async fn device_info(&self) -> Result<DeviceInfo, RouterError> {
        bump(&self.counts.other);
        self.data_gate()?;
        self.mock.device_info().await
    }
    async fn status(&self) -> Result<Status, RouterError> {
        bump(&self.counts.status);
        self.data_gate()?;
        self.mock.status().await
    }
    async fn signal(&self) -> Result<Signal, RouterError> {
        bump(&self.counts.signal);
        self.data_gate()?;
        if self.busy.load(Ordering::SeqCst) {
            return Err(api(100004, ApiErrorKind::Busy));
        }
        if self.slow_signal.load(Ordering::SeqCst) {
            sleep(Duration::from_secs(30)).await;
        }
        let mut s = self.mock.signal().await?;
        if let Some(band) = *self.band_override.lock().unwrap() {
            if let Some(lte) = s.lte.as_mut() {
                lte.band = Some(band);
            }
        }
        let mut guard = self.cell.lock().unwrap();
        if let Some(o) = guard.as_mut() {
            if o.remaining > 0 {
                o.remaining -= 1;
                if let Some(lte) = s.lte.as_mut() {
                    lte.pci = Some(o.pci);
                    lte.cell_id = Some(o.cell_id);
                }
            }
        }
        Ok(s)
    }
    async fn net_config(&self) -> Result<NetConfig, RouterError> {
        bump(&self.counts.net_config);
        self.data_gate()?;
        self.mock.net_config().await
    }
    async fn wan_address(&self) -> Result<WanAddress, RouterError> {
        bump(&self.counts.wan);
        self.data_gate()?;
        self.mock.wan_address().await
    }
    async fn data_switch(&self) -> Result<bool, RouterError> {
        bump(&self.counts.switch);
        self.data_gate()?;
        self.mock.data_switch().await
    }
    async fn set_data_switch(&self, on: bool) -> Result<(), RouterError> {
        bump(&self.counts.switch);
        self.data_gate()?;
        self.mock.set_data_switch(on).await
    }
    async fn network_search(&self) -> Result<router_core::NetworkSearch, RouterError> {
        bump(&self.counts.search);
        self.data_gate()?;
        self.mock.network_search().await
    }
    async fn apn_profiles(&self) -> Result<router_core::ApnProfiles, RouterError> {
        bump(&self.counts.apn);
        self.data_gate()?;
        self.mock.apn_profiles().await
    }
    async fn set_default_apn(&self, target: &router_core::ApnTarget) -> Result<(), RouterError> {
        bump(&self.counts.apn_switch);
        self.data_gate()?;
        self.mock.set_default_apn(target).await
    }
    async fn add_apn_profile(
        &self,
        expect: &router_core::ApnTarget,
        draft: &router_core::ApnDraft,
    ) -> Result<u32, RouterError> {
        bump(&self.counts.apn_write);
        self.data_gate()?;
        self.mock.add_apn_profile(expect, draft).await
    }
    async fn edit_apn_profile(
        &self,
        expect: &router_core::ApnTarget,
        target: &router_core::ApnTarget,
        draft: &router_core::ApnDraft,
    ) -> Result<(), RouterError> {
        bump(&self.counts.apn_write);
        self.data_gate()?;
        self.mock.edit_apn_profile(expect, target, draft).await
    }
    async fn delete_apn_profile(
        &self,
        expect: &router_core::ApnTarget,
        target: &router_core::ApnTarget,
    ) -> Result<(), RouterError> {
        bump(&self.counts.apn_write);
        self.data_gate()?;
        self.mock.delete_apn_profile(expect, target).await
    }
    async fn apply_net_config(&self, cfg: &NetConfig) -> Result<(), RouterError> {
        bump(&self.counts.apply);
        self.data_gate()?;
        if self.refuse_apply.lock().unwrap().as_ref() == Some(cfg) {
            return Err(api(112003, ApiErrorKind::BadParam));
        }
        if self.swallow_apply.lock().unwrap().as_ref() == Some(cfg) {
            return Ok(());
        }
        self.mock.apply_net_config(cfg).await
    }
    async fn reboot(&self) -> Result<(), RouterError> {
        bump(&self.counts.reboot);
        self.mock.reboot().await
    }

    async fn factory_reset(&self) -> Result<(), RouterError> {
        bump(&self.counts.factory_reset);
        self.mock.factory_reset().await
    }
}

// ---------------------------------------------------------------------------------------------
// Helpers

struct Rig {
    mock: Arc<MockDriver>,
    probe: Arc<Probe>,
    h: ActorHandle,
    alerts: broadcast::Receiver<Alert>,
}

fn actor_cfg(dir: &Path) -> ActorConfig {
    ActorConfig {
        host: HOST.to_string(),
        timers: SafeApplyTimers::default(),
        data_dir: dir.to_path_buf(),
        cell_alerts: true,
        automation: AutomationConfig::default(),
    }
}

fn spawn_on(mock: Arc<MockDriver>, dir: &Path) -> Rig {
    spawn_with(mock, dir, AutomationConfig::default())
}

fn spawn_with(mock: Arc<MockDriver>, dir: &Path, automation: AutomationConfig) -> Rig {
    let probe = Probe::new(mock.clone());
    let h = spawn_actor(
        probe.clone(),
        ActorConfig {
            automation,
            ..actor_cfg(dir)
        },
    );
    let alerts = h.alerts.subscribe();
    Rig {
        mock,
        probe,
        h,
        alerts,
    }
}

fn new_mock() -> Arc<MockDriver> {
    Arc::new(MockDriver::new(MockScenario::zain_like()))
}

fn creds() -> Credentials {
    Credentials::new("admin", "mock")
}

fn cfg(bands: &[u8]) -> NetConfig {
    NetConfig {
        network_mode: NetworkMode::Auto,
        lte_bands: LteBandSet::from_bands(bands).unwrap(),
        legacy_band_raw: "0".into(),
        nr_option: Some(NrOption::Nsa),
    }
}

fn initial() -> NetConfig {
    MockScenario::zain_like().initial
}

async fn send(h: &ActorHandle, c: Command) {
    h.commands.send(c).await.expect("actor is running");
}

/// Waits (in paused, auto-advancing time) until `pred` holds for the latest snapshot.
async fn wait_for(
    rx: &mut watch::Receiver<AppSnapshot>,
    within: Duration,
    what: &str,
    pred: impl Fn(&AppSnapshot) -> bool,
) -> AppSnapshot {
    let deadline = Instant::now() + within;
    loop {
        {
            let s = rx.borrow_and_update();
            if pred(&s) {
                return s.clone();
            }
        }
        match tokio::time::timeout_at(deadline, rx.changed()).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => panic!("actor stopped while waiting for {what}"),
            Err(_) => panic!(
                "timed out waiting for {what}; last snapshot: session {:?}, phase {:?}",
                rx.borrow().session,
                rx.borrow().safe_apply.phase
            ),
        }
    }
}

async fn connect(r: &mut Rig) {
    send(&r.h, Command::Connect(creds())).await;
    let mut rx = r.h.snapshots.clone();
    wait_for(&mut rx, Duration::from_secs(10), "login", |s| {
        s.session == SessionView::LoggedIn && s.config.is_some() && s.capabilities.is_some()
    })
    .await;
}

fn drain(rx: &mut broadcast::Receiver<Alert>) -> Vec<Alert> {
    let mut out = Vec::new();
    while let Ok(a) = rx.try_recv() {
        out.push(a);
    }
    out
}

fn phase(r: &Rig) -> Phase {
    r.h.snapshots.borrow().safe_apply.phase.clone()
}

async fn wait_phase(r: &Rig, within: Duration, what: &str, pred: impl Fn(&Phase) -> bool) {
    let mut rx = r.h.snapshots.clone();
    wait_for(&mut rx, within, what, |s| pred(&s.safe_apply.phase)).await;
}

async fn request_reboot(h: &ActorHandle) -> Result<RebootTicket, RouterError> {
    let (tx, rx) = oneshot::channel();
    send(h, Command::RequestReboot(tx)).await;
    rx.await.unwrap()
}

async fn confirm_reboot(h: &ActorHandle, id: &str) -> Result<(), RouterError> {
    let (tx, rx) = oneshot::channel();
    send(h, Command::ConfirmReboot(id.to_string(), tx)).await;
    rx.await.unwrap()
}

// ---------------------------------------------------------------------------------------------
// Tests

#[tokio::test(start_paused = true)]
async fn polls_politely() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect(&mut r).await;
    sleep(Duration::from_millis(2500)).await;

    // Visible: 1 s cadence.
    let c = &r.probe.counts;
    c.reset();
    sleep(Duration::from_secs(60)).await;
    let (sig, st, nc, hb) = (
        get(&c.signal),
        get(&c.status),
        get(&c.net_config),
        get(&c.heartbeat),
    );
    assert!((55..=61).contains(&sig), "signal polls {sig}");
    assert!(st <= 61, "status polls {st}");
    assert!(nc <= 7, "net_config polls {nc}");
    assert!(hb <= 13, "heartbeats {hb}");
    assert_eq!(get(&c.login), 0);

    // Hidden: 5 s cadence.
    send(&r.h, Command::SetVisible(false)).await;
    sleep(Duration::from_millis(5500)).await;
    c.reset();
    sleep(Duration::from_secs(60)).await;
    let (sig, st) = (get(&c.signal), get(&c.status));
    assert!((10..=13).contains(&sig), "hidden signal polls {sig}");
    assert!(sig + st <= 26, "hidden signal+status {}", sig + st);
    assert!(get(&c.heartbeat) <= 13);
    assert_eq!(get(&c.apply), 0);
}

/// The H122 drops a session 300 s after the last connect; the actor repeats the connect reads
/// at 270 s, silently (no session alert, no "connecting").
#[tokio::test(start_paused = true)]
async fn refreshes_session_before_router_expiry() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect(&mut r).await;
    drain(&mut r.alerts);
    let c = &r.probe.counts;
    c.reset();

    sleep(Duration::from_secs(260)).await;
    assert_eq!(get(&c.login), 0, "not before 270 s");
    sleep(Duration::from_secs(20)).await;
    assert_eq!(get(&c.login), 1, "one refresh by 280 s");
    sleep(Duration::from_secs(270)).await;
    assert_eq!(get(&c.login), 2, "and again 270 s later");

    assert_eq!(r.h.snapshots.borrow().session, SessionView::LoggedIn);
    let session_alerts = drain(&mut r.alerts)
        .into_iter()
        .filter(|a| matches!(a, Alert::Session { .. }))
        .count();
    assert_eq!(session_alerts, 0, "the refresh is invisible");
    let log = std::fs::read_to_string(dir.path().join("events.jsonl")).unwrap();
    assert_eq!(log.matches("session.refreshed").count(), 2);
}

/// Reaches probation, then makes the router hang on reads and waits until a poll is stuck.
async fn probation_with_hung_router(r: &mut Rig) {
    connect(r).await;
    send(&r.h, Command::Apply(cfg(&[28]))).await;
    wait_phase(r, Duration::from_secs(30), "probation", |p| {
        *p == Phase::Probation
    })
    .await;
    r.probe.slow_signal.store(true, Ordering::SeqCst);
    let before = get(&r.probe.counts.signal);
    while get(&r.probe.counts.signal) == before {
        sleep(Duration::from_millis(100)).await;
    }
}

/// Keep/Revert are answered within a second even while a poll hangs on a slow router
/// (device test 2026-10-29: both buttons seemed dead during a re-attach).
#[tokio::test(start_paused = true)]
async fn keep_is_not_blocked_by_a_hung_poll() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    probation_with_hung_router(&mut r).await;
    send(&r.h, Command::KeepNow).await;
    wait_phase(&r, Duration::from_secs(1), "committed", |p| {
        *p == Phase::Committed
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn revert_is_not_blocked_by_a_hung_poll() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    probation_with_hung_router(&mut r).await;
    send(&r.h, Command::RevertNow).await;
    wait_phase(&r, Duration::from_secs(1), "reverting", |p| {
        matches!(p, Phase::Reverting { .. } | Phase::Reverted { .. })
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn backoff_on_unreachable() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect(&mut r).await;
    sleep(Duration::from_millis(1500)).await;

    r.mock.set_fault(Some(Fault::Unreachable));
    let c = &r.probe.counts;
    c.reset();
    sleep(Duration::from_secs(60)).await;
    let attempts = get(&c.signal);
    assert!(
        (3..=8).contains(&attempts),
        "poll attempts while unreachable {attempts}"
    );
    // Unreachable isn't a session loss: no login storm.
    assert_eq!(get(&c.login), 0);

    r.mock.set_fault(None);
    sleep(Duration::from_secs(30)).await;
    c.reset();
    sleep(Duration::from_secs(10)).await;
    assert!(get(&c.signal) >= 9, "back to 1 s: {}", get(&c.signal));
}

#[tokio::test(start_paused = true)]
async fn end_to_end_bad_band_reverts() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect(&mut r).await;

    send(&r.h, Command::Apply(cfg(&[7]))).await;
    wait_phase(&r, Duration::from_secs(10), "grace", |p| {
        *p == Phase::ReattachGrace
    })
    .await;
    sleep(Duration::from_secs(80)).await;
    assert_eq!(r.mock.applied(), vec![cfg(&[7])], "no revert before grace");

    wait_phase(&r, Duration::from_secs(60), "reverted", |p| {
        *p == Phase::Reverted {
            reason: RevertReason::NoReattach,
            fallback: false,
        }
    })
    .await;
    assert_eq!(r.mock.applied().last(), Some(&initial()));
    assert_eq!(r.mock.applied().len(), 2);

    let alerts = drain(&mut r.alerts);
    let events: Vec<SafeApplyEvent> = alerts
        .into_iter()
        .filter_map(|a| match a {
            Alert::SafeApply { event } => Some(event),
            _ => None,
        })
        .collect();
    for e in [
        SafeApplyEvent::Started,
        SafeApplyEvent::Applied,
        SafeApplyEvent::Verified,
    ] {
        assert!(events.contains(&e), "missing {e:?} in {events:?}");
    }
    assert!(events.contains(&SafeApplyEvent::Reverted {
        reason: RevertReason::NoReattach,
        fallback: false,
    }));
    assert!(!dir.path().join("pending-change.json").exists());

    // Started/Applied/Verified and the revert are in the event log.
    let log = std::fs::read_to_string(dir.path().join("events.jsonl")).unwrap();
    for kind in [
        "safe_apply.started",
        "safe_apply.applied",
        "safe_apply.verified",
        "safe_apply.reverted",
    ] {
        assert!(log.contains(kind), "event log lacks {kind}");
    }
}

#[tokio::test(start_paused = true)]
async fn end_to_end_good_band_commits() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect(&mut r).await;

    send(&r.h, Command::Apply(cfg(&[28]))).await;
    wait_phase(&r, Duration::from_secs(30), "probation", |p| {
        *p == Phase::Probation
    })
    .await;
    assert!(dir.path().join("pending-change.json").exists());
    let rec: JournalRecord =
        serde_json::from_slice(&std::fs::read(dir.path().join("pending-change.json")).unwrap())
            .unwrap();
    assert_eq!(rec.phase, JournalPhase::Probation);
    assert!(rec.created_at_ms > 0 && rec.phase_changed_at_ms >= rec.created_at_ms);

    sleep(Duration::from_secs(170)).await;
    assert_eq!(phase(&r), Phase::Probation, "commits only after 180 s");
    wait_phase(&r, Duration::from_secs(20), "committed", |p| {
        *p == Phase::Committed
    })
    .await;
    assert!(!dir.path().join("pending-change.json").exists());
    let archived: Vec<_> = std::fs::read_dir(dir.path().join("journal-archive"))
        .unwrap()
        .collect();
    assert_eq!(archived.len(), 1);
    assert_eq!(r.mock.applied(), vec![cfg(&[28])]);
    assert!(drain(&mut r.alerts).contains(&Alert::SafeApply {
        event: SafeApplyEvent::Committed
    }));
    // Each POST and read-back is in the event log with the band mask it carried.
    let log = std::fs::read_to_string(dir.path().join("events.jsonl")).unwrap();
    assert_eq!(log.matches("\"safe_apply.post\"").count(), 1);
    assert!(
        log.matches("\"safe_apply.read\"").count() >= 2,
        "snapshot + verify"
    );
    let mask = serde_json::to_string(&cfg(&[28]).lte_bands).unwrap();
    assert!(log.contains(&format!("\"lteBands\":{mask}")));
}

/// The fault model holds one fault at a time, so "no service" is set first and observed, then
/// replaced by the takeover. That way the loss timer is running when the session disappears.
/// "Clearing the takeover" switches back to `NoServiceForever`, so the router is reachable with
/// a fresh login but still out of service.
#[tokio::test(start_paused = true)]
async fn takeover_during_probation_still_reverts() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect(&mut r).await;
    let t0 = Instant::now();

    send(&r.h, Command::Apply(cfg(&[3]))).await;
    wait_phase(&r, Duration::from_secs(30), "probation", |p| {
        *p == Phase::Probation
    })
    .await;

    tokio::time::sleep_until(t0 + Duration::from_secs(20)).await;
    r.mock.set_fault(Some(Fault::NoServiceForever));
    sleep(Duration::from_secs(2)).await;
    r.mock.set_fault(Some(Fault::SessionTakeover));
    let mut rx = r.h.snapshots.clone();
    wait_for(&mut rx, Duration::from_secs(5), "taken over", |s| {
        s.session == SessionView::TakenOver
    })
    .await;
    assert!(drain(&mut r.alerts).contains(&Alert::Session {
        view: SessionView::TakenOver
    }));

    // Re-login every 5 s while the change is active.
    tokio::time::sleep_until(t0 + Duration::from_secs(100)).await;
    let logins = get(&r.probe.counts.login);
    tokio::time::sleep_until(t0 + Duration::from_secs(200)).await;
    let relogins = get(&r.probe.counts.login) - logins;
    assert!(
        (18..=21).contains(&relogins),
        "re-logins in 100 s: {relogins}"
    );
    assert_eq!(r.mock.applied().len(), 1, "no POST without a session");
    assert!(
        matches!(phase(&r), Phase::RevertPending { .. }),
        "phase {:?}",
        phase(&r)
    );
    assert_eq!(r.h.snapshots.borrow().session, SessionView::TakenOver);

    r.mock.set_fault(Some(Fault::NoServiceForever));
    sleep(Duration::from_secs(10)).await;
    let applied = r.mock.applied();
    assert_eq!(applied.len(), 2, "revert POST within 10 s");
    assert_eq!(applied[1], initial());
}

#[tokio::test(start_paused = true)]
async fn crash_mid_probation_resumes() {
    let dir = tempfile::tempdir().unwrap();
    let mock = new_mock();
    let mut r = spawn_on(mock.clone(), dir.path());
    connect(&mut r).await;
    send(&r.h, Command::Apply(cfg(&[3]))).await;
    wait_phase(&r, Duration::from_secs(30), "probation", |p| {
        *p == Phase::Probation
    })
    .await;

    send(&r.h, Command::Shutdown).await;
    let mut rx = r.h.snapshots.clone();
    tokio::time::timeout(Duration::from_secs(5), async {
        while rx.changed().await.is_ok() {}
    })
    .await
    .expect("actor exits on Shutdown");
    drop(r);

    sleep(Duration::from_secs(30)).await;
    let mut r2 = spawn_on(mock.clone(), dir.path());
    let mut rx = r2.h.snapshots.clone();
    let s = wait_for(&mut rx, Duration::from_secs(2), "resumed grace", |s| {
        s.safe_apply.phase == Phase::ReattachGrace
    })
    .await;
    assert!(
        s.safe_apply.seconds_left.unwrap_or(0) >= 89,
        "fresh grace timer: {:?}",
        s.safe_apply.seconds_left
    );
    assert_eq!(s.foreign_journal, None);

    connect(&mut r2).await;
    wait_phase(&r2, Duration::from_secs(300), "committed", |p| {
        *p == Phase::Committed
    })
    .await;
    assert_eq!(mock.applied(), vec![cfg(&[3])], "no revert POST");
    assert!(!dir.path().join("pending-change.json").exists());
}

#[tokio::test(start_paused = true)]
async fn unreachable_router_with_journal_waits() {
    let dir = tempfile::tempdir().unwrap();
    let previous = cfg(&[1, 3]);
    Journal::new(dir.path().to_path_buf())
        .write(&JournalRecord {
            version: JOURNAL_VERSION,
            id: "11111111-2222-4333-8444-555555555555".into(),
            driver: "mock".into(),
            host: HOST.into(),
            previous: previous.clone(),
            requested: cfg(&[3]),
            phase: JournalPhase::Reverting,
            phase_changed_at_ms: 1,
            created_at_ms: 1,
        })
        .unwrap();

    let mock = new_mock();
    mock.set_fault(Some(Fault::Unreachable));
    let r = spawn_on(mock.clone(), dir.path());
    send(&r.h, Command::Connect(creds())).await;
    sleep(Duration::from_secs(60)).await;

    let total = r.probe.counts.total();
    assert!(total <= 8, "driver calls while unreachable: {total}");
    assert!(get(&r.probe.counts.login) >= 3, "keeps trying");
    assert_eq!(get(&r.probe.counts.apply), 0, "no POST");
    assert!(
        matches!(
            phase(&r),
            Phase::RevertPending { .. } | Phase::Reverting { .. }
        ),
        "phase {:?}",
        phase(&r)
    );
    assert!(dir.path().join("pending-change.json").exists());

    mock.set_fault(None);
    sleep(Duration::from_secs(40)).await;
    assert_eq!(mock.applied().first(), Some(&previous), "revert posted");
}

#[tokio::test(start_paused = true)]
async fn bad_credentials_stop_retrying() {
    let dir = tempfile::tempdir().unwrap();
    let mock = new_mock();
    let r = spawn_on(mock.clone(), dir.path());
    send(&r.h, Command::Connect(Credentials::new("admin", "wrong"))).await;
    sleep(Duration::from_secs(120)).await;
    assert_eq!(mock.login_attempts(), 1);
    assert_eq!(r.h.snapshots.borrow().session, SessionView::BadCredentials);
    assert_eq!(get(&r.probe.counts.signal), 0);
    assert!(dir.path().join("password-rejected").exists());
}

fn marker(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join("password-rejected")).ok()
}

/// The actor writes the marker itself, before it publishes the refusal: no alert reader needed.
#[tokio::test(start_paused = true)]
async fn bad_password_writes_marker_with_no_alert_reader() {
    let dir = tempfile::tempdir().unwrap();
    let mock = new_mock();
    let Rig { h, alerts, .. } = spawn_on(mock.clone(), dir.path());
    drop(alerts);
    send(&h, Command::Connect(Credentials::new("admin", "wrong"))).await;
    let mut rx = h.snapshots.clone();
    let d = dir.path().to_path_buf();
    wait_for(&mut rx, Duration::from_secs(10), "refusal", |s| {
        if s.session == SessionView::BadCredentials {
            assert_eq!(marker(&d), Some(format!("{HOST}/admin")), "marker first");
            true
        } else {
            false
        }
    })
    .await;
    assert_eq!(mock.login_attempts(), 1);
}

/// A reader that fell behind the alert buffer can't make the marker go missing.
#[tokio::test(start_paused = true)]
async fn bad_password_marker_survives_a_lagged_reader() {
    use tokio::sync::broadcast::error::TryRecvError;
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect(&mut r).await;
    // More cell changes than the 64-alert buffer holds, none of them read.
    for i in 0..70u16 {
        r.probe
            .override_cell(500 + i, 1000 + u64::from(i), u32::MAX);
        sleep(Duration::from_secs(3)).await;
    }
    send(&r.h, Command::Connect(Credentials::new("admin", "wrong"))).await;
    let mut rx = r.h.snapshots.clone();
    wait_for(&mut rx, Duration::from_secs(10), "refusal", |s| {
        s.session == SessionView::BadCredentials
    })
    .await;
    assert_eq!(marker(dir.path()), Some(format!("{HOST}/admin")));
    assert!(matches!(r.alerts.try_recv(), Err(TryRecvError::Lagged(_))));
}

#[tokio::test(start_paused = true)]
async fn good_login_writes_no_marker() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect(&mut r).await;
    assert!(!dir.path().join("password-rejected").exists());
}

async fn request_factory_reset(h: &ActorHandle) -> Result<RebootTicket, RouterError> {
    let (tx, rx) = oneshot::channel();
    send(h, Command::RequestFactoryReset(tx)).await;
    rx.await.unwrap()
}

async fn confirm_factory_reset(h: &ActorHandle, id: &str) -> Result<(), RouterError> {
    let (tx, rx) = oneshot::channel();
    send(h, Command::ConfirmFactoryReset(id.to_string(), tx)).await;
    rx.await.unwrap()
}

#[tokio::test(start_paused = true)]
async fn factory_reset_needs_a_fresh_ticket_and_then_forgets_the_password() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect(&mut r).await;

    assert!(confirm_factory_reset(&r.h, "not-a-ticket").await.is_err());

    let t = request_factory_reset(&r.h).await.unwrap();
    assert_eq!(t.expires_in_s, 30);
    sleep(Duration::from_secs(31)).await;
    assert!(confirm_factory_reset(&r.h, &t.id).await.is_err());
    assert_eq!(get(&r.probe.counts.factory_reset), 0);

    // A wrong id uses the ticket up as well.
    let t = request_factory_reset(&r.h).await.unwrap();
    assert!(confirm_factory_reset(&r.h, "wrong-id").await.is_err());
    assert!(confirm_factory_reset(&r.h, &t.id).await.is_err());
    assert_eq!(get(&r.probe.counts.factory_reset), 0);

    let t = request_factory_reset(&r.h).await.unwrap();
    confirm_factory_reset(&r.h, &t.id).await.unwrap();
    assert_eq!(get(&r.probe.counts.factory_reset), 1);
    assert!(confirm_factory_reset(&r.h, &t.id).await.is_err());
    assert_eq!(get(&r.probe.counts.factory_reset), 1);

    // The old password would be wrong now: it's never tried on the reset router.
    let logins = get(&r.probe.counts.login);
    let mut rx = r.h.snapshots.clone();
    wait_for(&mut rx, Duration::from_secs(5), "not configured", |s| {
        s.session == SessionView::NotConfigured
    })
    .await;
    sleep(Duration::from_secs(300)).await;
    assert_eq!(get(&r.probe.counts.login), logins);
}

#[tokio::test(start_paused = true)]
async fn factory_reset_is_blocked_during_a_band_change_and_a_speed_test() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect(&mut r).await;

    send(&r.h, Command::SetSpeedTestActive(true)).await;
    assert!(request_factory_reset(&r.h).await.is_err());
    send(&r.h, Command::SetSpeedTestActive(false)).await;

    // A ticket taken before the change doesn't carry through it.
    let t = request_factory_reset(&r.h).await.unwrap();
    send(&r.h, Command::Apply(cfg(&[3]))).await;
    wait_phase(&r, Duration::from_secs(60), "probation", |p| {
        *p == Phase::Probation
    })
    .await;
    assert!(request_factory_reset(&r.h).await.is_err());
    assert!(confirm_factory_reset(&r.h, &t.id).await.is_err());
    assert_eq!(get(&r.probe.counts.factory_reset), 0);
}

fn refusal_text(r: Result<RebootTicket, RouterError>) -> Option<String> {
    match r {
        Ok(_) => None,
        Err(RouterError::Validation { message }) => Some(message),
        Err(e) => panic!("unexpected refusal {e:?}"),
    }
}

/// Today's refusals of a reboot and a factory reset, by what else holds the router.
#[tokio::test(start_paused = true)]
async fn busy_refusals_by_what_holds_the_router() {
    #[derive(Debug, Clone, Copy)]
    enum Busy {
        Nothing,
        SpeedTest,
        Scan,
        BandChange,
    }
    // (state, reboot refusal, factory reset refusal)
    let table: [(Busy, Option<&str>, Option<&str>); 4] = [
        (Busy::Nothing, None, None),
        (Busy::SpeedTest, None, Some(SPEED_TEST_RUNNING)),
        (
            Busy::Scan,
            Some(CHANGE_IN_PROGRESS),
            Some(CHANGE_IN_PROGRESS),
        ),
        (
            Busy::BandChange,
            Some(CHANGE_IN_PROGRESS),
            Some(CHANGE_IN_PROGRESS),
        ),
    ];
    for (busy, reboot, reset) in table {
        let dir = tempfile::tempdir().unwrap();
        let mut r = spawn_on(new_mock(), dir.path());
        connect(&mut r).await;
        match busy {
            Busy::Nothing => {}
            Busy::SpeedTest => send(&r.h, Command::SetSpeedTestActive(true)).await,
            Busy::Scan => send(&r.h, Command::SetScanActive(true)).await,
            Busy::BandChange => {
                send(&r.h, Command::Apply(cfg(&[3]))).await;
                wait_phase(&r, Duration::from_secs(60), "probation", |p| {
                    *p == Phase::Probation
                })
                .await;
            }
        }
        assert_eq!(
            refusal_text(request_reboot(&r.h).await).as_deref(),
            reboot,
            "reboot while {busy:?}"
        );
        assert_eq!(
            refusal_text(request_factory_reset(&r.h).await).as_deref(),
            reset,
            "factory reset while {busy:?}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn reboot_ticket_rules() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect(&mut r).await;

    assert!(matches!(
        confirm_reboot(&r.h, "not-a-ticket").await,
        Err(RouterError::Validation { .. })
    ));

    let t = request_reboot(&r.h).await.unwrap();
    assert_eq!(t.expires_in_s, 30);
    sleep(Duration::from_secs(31)).await;
    assert!(matches!(
        confirm_reboot(&r.h, &t.id).await,
        Err(RouterError::Validation { .. })
    ));
    assert_eq!(get(&r.probe.counts.reboot), 0);

    let t = request_reboot(&r.h).await.unwrap();
    assert!(matches!(
        confirm_reboot(&r.h, "wrong-id").await,
        Err(RouterError::Validation { .. })
    ));
    sleep(Duration::from_secs(5)).await;
    confirm_reboot(&r.h, &t.id).await.unwrap();
    assert_eq!(get(&r.probe.counts.reboot), 1);
    // A ticket is single-use.
    assert!(confirm_reboot(&r.h, &t.id).await.is_err());
    assert_eq!(get(&r.probe.counts.reboot), 1);

    // Reconnects only after 60 s.
    let logins = get(&r.probe.counts.login);
    sleep(Duration::from_secs(55)).await;
    assert_eq!(get(&r.probe.counts.login), logins, "no login before 60 s");
    let mut rx = r.h.snapshots.clone();
    wait_for(&mut rx, Duration::from_secs(10), "re-login", |s| {
        s.session == SessionView::LoggedIn
    })
    .await;

    send(&r.h, Command::Apply(cfg(&[3]))).await;
    wait_phase(&r, Duration::from_secs(60), "probation", |p| {
        *p == Phase::Probation
    })
    .await;
    assert!(request_reboot(&r.h).await.is_err());
    assert_eq!(get(&r.probe.counts.reboot), 1);
}

#[tokio::test(start_paused = true)]
async fn cell_change_alert_debounced() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect(&mut r).await;
    sleep(Duration::from_secs(3)).await;
    drain(&mut r.alerts);

    // A one-poll flap: no alert.
    r.probe.override_cell(999, 42, 1);
    sleep(Duration::from_secs(5)).await;
    let cell_alerts = |a: &[Alert]| {
        a.iter()
            .filter(|a| matches!(a, Alert::CellChanged { .. }))
            .cloned()
            .collect::<Vec<_>>()
    };
    assert!(cell_alerts(&drain(&mut r.alerts)).is_empty());

    // A real change: exactly one alert once it's stable for 2 polls.
    r.probe.override_cell(999, 42, u32::MAX);
    sleep(Duration::from_secs(5)).await;
    let got = cell_alerts(&drain(&mut r.alerts));
    let b28 = CellKey {
        rat: Rat::Lte,
        pci: Some(128),
        cell_id: Some(1_000_028),
        band: "B28".into(),
    };
    let new = CellKey {
        rat: Rat::Lte,
        pci: Some(999),
        cell_id: Some(42),
        band: "B28".into(),
    };
    assert_eq!(got, vec![Alert::CellChanged { from: b28, to: new }]);

    // The log names the cell by PCI and band only: no cell ID.
    let line = event_log(dir.path())
        .lines()
        .find(|l| l.contains("\"cell.changed\""))
        .map(str::to_string)
        .expect("a cell.changed event");
    let rec: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        rec["detail"],
        serde_json::json!({
            "from": {"rat": "lte", "pci": 128, "band": "B28"},
            "to": {"rat": "lte", "pci": 999, "band": "B28"},
        })
    );
    assert!(!line.contains("cell_id") && !line.contains("cellId"));
}

#[tokio::test(start_paused = true)]
async fn cell_alerts_respect_setting() {
    let dir = tempfile::tempdir().unwrap();
    let probe = Probe::new(new_mock());
    let mut c = actor_cfg(dir.path());
    c.cell_alerts = false;
    let h = spawn_actor(probe.clone(), c);
    let mut alerts = h.alerts.subscribe();
    send(&h, Command::Connect(creds())).await;
    sleep(Duration::from_secs(3)).await;
    probe.override_cell(999, 42, u32::MAX);
    sleep(Duration::from_secs(5)).await;
    assert!(!drain(&mut alerts)
        .iter()
        .any(|a| matches!(a, Alert::CellChanged { .. })));
}

#[tokio::test(start_paused = true)]
async fn foreign_journal_is_surfaced_not_acted_on() {
    let dir = tempfile::tempdir().unwrap();
    let rec = JournalRecord {
        version: JOURNAL_VERSION,
        id: "aaaaaaaa-2222-4333-8444-555555555555".into(),
        driver: "mock".into(),
        host: "10.0.0.1".into(),
        previous: cfg(&[1]),
        requested: cfg(&[3]),
        phase: JournalPhase::Reverting,
        phase_changed_at_ms: 1,
        created_at_ms: 1,
    };
    Journal::new(dir.path().to_path_buf()).write(&rec).unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect(&mut r).await;
    sleep(Duration::from_secs(10)).await;
    let s = r.h.snapshots.borrow().clone();
    assert_eq!(s.foreign_journal, Some(rec));
    assert_eq!(s.safe_apply.phase, Phase::Idle);
    assert!(r.mock.applied().is_empty());

    // A new apply would overwrite the foreign journal: refused.
    send(&r.h, Command::Apply(cfg(&[28]))).await;
    sleep(Duration::from_secs(5)).await;
    assert!(r.mock.applied().is_empty());
    assert!(drain(&mut r.alerts).iter().any(|a| matches!(
        a,
        Alert::SafeApply {
            event: SafeApplyEvent::Rejected { .. }
        }
    )));
}

#[test]
fn alert_and_session_serde_shape() {
    let a = Alert::Session {
        view: SessionView::Reconnecting {
            reason: "router unreachable".into(),
            retry_in_s: 4,
        },
    };
    assert_eq!(
        serde_json::to_value(&a).unwrap(),
        serde_json::json!({"kind": "session", "view": {"kind": "reconnecting", "reason": "router unreachable", "retryInS": 4}})
    );
    let a = Alert::SafeApply {
        event: SafeApplyEvent::Committed,
    };
    assert_eq!(
        serde_json::to_value(&a).unwrap(),
        serde_json::json!({"kind": "safeApply", "event": {"kind": "committed"}})
    );
    let k = CellKey {
        rat: Rat::Lte,
        pci: Some(1),
        cell_id: None,
        band: "B3".into(),
    };
    let a = Alert::CellChanged {
        from: k.clone(),
        to: k,
    };
    assert_eq!(
        serde_json::to_value(&a).unwrap()["from"],
        serde_json::json!({"rat": "lte", "pci": 1, "cellId": null, "band": "B3"})
    );
    assert_eq!(
        serde_json::to_value(SessionView::LockedOut { wait_s: 60 }).unwrap(),
        serde_json::json!({"kind": "lockedOut", "waitS": 60})
    );
    assert_eq!(
        serde_json::to_value(SessionView::LoginRejected { code: 108009 }).unwrap(),
        serde_json::json!({"kind": "loginRejected", "code": 108009})
    );
}

#[tokio::test(start_paused = true)]
async fn shutdown_waits_for_actor_exit() {
    let dir = tempfile::tempdir().unwrap();
    let r = spawn_on(new_mock(), dir.path());
    send(&r.h, Command::Connect(creds())).await;
    sleep(Duration::from_secs(3)).await;
    let tx = r.h.commands.clone();
    assert!(r.h.shutdown(Duration::from_secs(15)).await);
    assert!(tx.send(Command::SetVisible(true)).await.is_err());
}

#[tokio::test(start_paused = true)]
async fn cell_alerts_toggle_applies() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect(&mut r).await;
    sleep(Duration::from_secs(3)).await;
    drain(&mut r.alerts);
    let count = |a: &[Alert]| {
        a.iter()
            .filter(|a| matches!(a, Alert::CellChanged { .. }))
            .count()
    };
    r.probe.override_cell(999, 42, u32::MAX);
    sleep(Duration::from_secs(5)).await;
    assert_eq!(count(&drain(&mut r.alerts)), 1);

    send(&r.h, Command::SetCellAlerts(false)).await;
    r.probe.override_cell(777, 43, u32::MAX);
    sleep(Duration::from_secs(5)).await;
    assert_eq!(count(&drain(&mut r.alerts)), 0);
}

/// C1: the Huawei router answers 100003 ("no rights") once our login is gone. The actor must
/// treat that as a session loss, log in again and still revert a bad band.
#[tokio::test(start_paused = true)]
async fn huawei_style_session_loss_relogs_and_reverts() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect(&mut r).await;

    send(&r.h, Command::Apply(cfg(&[7]))).await;
    wait_phase(&r, Duration::from_secs(10), "grace", |p| {
        *p == Phase::ReattachGrace
    })
    .await;
    let t0 = Instant::now();
    sleep(Duration::from_secs(30)).await;
    let logins = r.mock.login_attempts();
    r.probe.no_rights.store(true, Ordering::SeqCst);

    // Past the grace deadline while the session is gone.
    tokio::time::sleep_until(t0 + Duration::from_secs(100)).await;
    let relogins = r.mock.login_attempts() - logins;
    assert!(relogins >= 2, "re-logins while 100003: {relogins}");
    assert_eq!(r.mock.applied().len(), 1, "no POST without a session");

    r.probe.no_rights.store(false, Ordering::SeqCst);
    sleep(Duration::from_secs(15)).await;
    let applied = r.mock.applied();
    assert_eq!(applied.len(), 2, "revert POST after the re-login");
    assert_eq!(applied[1], initial());
}

/// I1: a poll that fails with a router API error is not evidence of service, so the
/// probation deadline can't commit on Ticks alone.
#[tokio::test(start_paused = true)]
async fn api_errors_during_probation_do_not_commit() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect(&mut r).await;

    send(&r.h, Command::Apply(cfg(&[28]))).await;
    wait_phase(&r, Duration::from_secs(30), "probation", |p| {
        *p == Phase::Probation
    })
    .await;
    r.probe.busy.store(true, Ordering::SeqCst);
    sleep(Duration::from_secs(240)).await;
    assert_eq!(phase(&r), Phase::Probation, "no commit without evidence");

    r.probe.busy.store(false, Ordering::SeqCst);
    wait_phase(&r, Duration::from_secs(40), "committed", |p| {
        *p == Phase::Committed
    })
    .await;
    assert_eq!(r.mock.applied(), vec![cfg(&[28])]);
}

/// I2: an unrecognised login refusal is never retried automatically (lockout guard).
#[tokio::test(start_paused = true)]
async fn login_rejected_stops_retrying() {
    let dir = tempfile::tempdir().unwrap();
    let r = spawn_on(new_mock(), dir.path());
    *r.probe.login_error.lock().unwrap() = Some(RouterError::LoginRejected { code: 108009 });
    send(&r.h, Command::Connect(creds())).await;
    sleep(Duration::from_secs(120)).await;
    assert_eq!(get(&r.probe.counts.login), 1);
    assert_eq!(
        r.h.snapshots.borrow().session,
        SessionView::LoginRejected { code: 108009 }
    );
    assert_eq!(get(&r.probe.counts.signal), 0);
}

/// I4: capabilities that failed at login are fetched again after later successful polls.
#[tokio::test(start_paused = true)]
async fn capabilities_retried_after_failure() {
    let dir = tempfile::tempdir().unwrap();
    let r = spawn_on(new_mock(), dir.path());
    r.probe.caps_busy.store(true, Ordering::SeqCst);
    send(&r.h, Command::Connect(creds())).await;
    let mut rx = r.h.snapshots.clone();
    wait_for(&mut rx, Duration::from_secs(10), "login", |s| {
        s.session == SessionView::LoggedIn && s.signal.is_some()
    })
    .await;
    sleep(Duration::from_secs(5)).await;
    assert!(r.h.snapshots.borrow().capabilities.is_none());

    // Retried at most once every 10 s (device_info is fetched once per login and also counts
    // as "other", but there is no re-login here).
    let before = get(&r.probe.counts.other);
    sleep(Duration::from_secs(30)).await;
    let tries = get(&r.probe.counts.other) - before;
    assert!(
        (2..=4).contains(&tries),
        "capability retries in 30 s: {tries}"
    );

    r.probe.caps_busy.store(false, Ordering::SeqCst);
    wait_for(&mut rx, Duration::from_secs(15), "capabilities", |s| {
        s.capabilities.is_some()
    })
    .await;
}

/// I5 / acceptance criterion 3: with the window closed (5 s polling) a bad band is still
/// reverted on time, and polling stays at the hidden cadence.
#[tokio::test(start_paused = true)]
async fn hidden_window_bad_band_reverts() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect(&mut r).await;
    send(&r.h, Command::SetVisible(false)).await;
    sleep(Duration::from_secs(6)).await;

    send(&r.h, Command::Apply(cfg(&[7]))).await;
    wait_phase(&r, Duration::from_secs(10), "grace", |p| {
        *p == Phase::ReattachGrace
    })
    .await;
    let t0 = Instant::now();
    r.probe.counts.reset();

    sleep(Duration::from_secs(60)).await;
    let polls = get(&r.probe.counts.signal);
    assert!((10..=13).contains(&polls), "hidden polls in 60 s: {polls}");
    assert_eq!(r.mock.applied().len(), 1, "no revert before grace");

    tokio::time::sleep_until(t0 + Duration::from_secs(100)).await;
    let applied = r.mock.applied();
    assert_eq!(applied.len(), 2, "revert POST by grace + 10 s");
    assert_eq!(applied[1], initial());

    r.probe.counts.reset();
    sleep(Duration::from_secs(60)).await;
    let polls = get(&r.probe.counts.signal);
    assert!(
        (10..=13).contains(&polls),
        "hidden polls after revert: {polls}"
    );
}

// ---------------------------------------------------------------------------------------------
// Automation: loss policy, primary-band two-step apply, re-lock watchdog

fn primary(band: u8, relock: bool) -> AutomationConfig {
    AutomationConfig {
        primary_band: Some(band),
        relock_primary: relock,
        revert_on_loss: true,
    }
}

/// Alerts so far, tolerating a lagged receiver (long runs send more than the buffer holds).
fn drain_all(rx: &mut broadcast::Receiver<Alert>) -> Vec<Alert> {
    use tokio::sync::broadcast::error::TryRecvError;
    let mut out = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(a) => out.push(a),
            Err(TryRecvError::Lagged(_)) => continue,
            Err(_) => break,
        }
    }
    out
}

fn automation_events(alerts: &[Alert]) -> Vec<AutomationEvent> {
    alerts
        .iter()
        .filter_map(|a| match a {
            Alert::Automation { event } => Some(event.clone()),
            _ => None,
        })
        .collect()
}

fn event_log(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("events.jsonl")).unwrap_or_default()
}

/// Waits until no change or sequence is open and `n` configs were applied.
async fn wait_settled(r: &Rig, within: Duration, n: usize) -> AppSnapshot {
    let mut rx = r.h.snapshots.clone();
    let mock = r.mock.clone();
    wait_for(&mut rx, within, "sequence end", move |s| {
        s.safe_apply.sequence.is_none()
            && matches!(
                s.safe_apply.phase,
                Phase::Idle | Phase::Committed | Phase::Reverted { .. }
            )
            && mock.applied().len() >= n
    })
    .await
}

#[tokio::test(start_paused = true)]
async fn defaults_expose_policy_and_no_watchdog() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect(&mut r).await;
    let s = r.h.snapshots.borrow().clone();
    let policy = s.safe_apply.policy.expect("policy is always set");
    assert!(policy.revert_on_loss && !policy.loss_ignored);
    assert_eq!(s.safe_apply.sequence, None);
    assert_eq!(s.relock, None);
}

#[tokio::test(start_paused = true)]
async fn loss_policy_off_keeps_a_change_without_service() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect(&mut r).await;
    // Live, without restarting the actor.
    send(
        &r.h,
        Command::SetAutomation(AutomationConfig {
            revert_on_loss: false,
            ..AutomationConfig::default()
        }),
    )
    .await;
    // B7 has no cell: no re-attach. With the policy off, no revert either.
    send(&r.h, Command::Apply(cfg(&[7]))).await;
    wait_phase(&r, Duration::from_secs(120), "probation", |p| {
        *p == Phase::Probation
    })
    .await;
    assert!(
        r.h.snapshots
            .borrow()
            .safe_apply
            .policy
            .unwrap()
            .loss_ignored
    );
    wait_phase(&r, Duration::from_secs(200), "committed", |p| {
        *p == Phase::Committed
    })
    .await;
    assert_eq!(r.mock.applied(), vec![cfg(&[7])], "never reverted");
    assert!(!dir.path().join("pending-change.json").exists());
    let events = automation_events(&drain_all(&mut r.alerts));
    assert!(events.contains(&AutomationEvent::LossIgnored {
        reason: RevertReason::NoReattach
    }));
    let log = event_log(dir.path());
    assert_eq!(log.matches("\"safe_apply.loss_ignored\"").count(), 1);
    assert!(log.contains("\"automation.settings\""));
}

#[tokio::test(start_paused = true)]
async fn loss_policy_on_still_reverts_after_turning_back_on() {
    let dir = tempfile::tempdir().unwrap();
    let off = AutomationConfig {
        revert_on_loss: false,
        ..AutomationConfig::default()
    };
    let mut r = spawn_with(new_mock(), dir.path(), off);
    connect(&mut r).await;
    send(&r.h, Command::SetAutomation(AutomationConfig::default())).await;
    send(&r.h, Command::Apply(cfg(&[7]))).await;
    wait_phase(&r, Duration::from_secs(150), "reverted", |p| {
        matches!(p, Phase::Reverted { .. })
    })
    .await;
    assert_eq!(r.mock.applied(), vec![cfg(&[7]), initial()]);
}

#[tokio::test(start_paused = true)]
async fn primary_band_apply_runs_two_steps() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_with(new_mock(), dir.path(), primary(3, false));
    connect(&mut r).await;
    send(&r.h, Command::Apply(cfg(&[1, 3, 20]))).await;

    let mut rx = r.h.snapshots.clone();
    let s = wait_for(&mut rx, Duration::from_secs(10), "step 1", |s| {
        s.safe_apply.sequence.as_ref().is_some_and(|q| q.step == 1)
    })
    .await;
    let q = s.safe_apply.sequence.unwrap();
    assert_eq!((q.total, q.primary), (2, 3));
    assert_eq!(q.final_config, cfg(&[1, 3, 20]));
    assert_eq!(q.origin, SequenceOrigin::User);
    assert_eq!(r.mock.applied(), vec![cfg(&[3])], "step 1 is B3 alone");

    let s = wait_for(&mut rx, Duration::from_secs(300), "step 2", |s| {
        s.safe_apply.sequence.as_ref().is_some_and(|q| q.step == 2)
    })
    .await;
    // Step 2 goes back to the config from before step 1 if it fails.
    assert_eq!(s.safe_apply.previous, Some(initial()));

    let s = wait_settled(&r, Duration::from_secs(300), 2).await;
    assert_eq!(s.safe_apply.phase, Phase::Committed);
    assert_eq!(r.mock.applied(), vec![cfg(&[3]), cfg(&[1, 3, 20])]);
    assert!(automation_events(&drain_all(&mut r.alerts)).is_empty());
    let log = event_log(dir.path());
    for kind in ["sequence.started", "sequence.step2", "sequence.ended"] {
        assert!(log.contains(&format!("\"{kind}\"")), "{kind} missing");
    }
}

/// The star moved but the band set didn't: the router still has to settle on the starred band.
#[tokio::test(start_paused = true)]
async fn a_star_on_the_same_bands_runs_two_steps() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_with(new_mock(), dir.path(), primary(3, false));
    connect(&mut r).await;
    send(&r.h, Command::Apply(initial())).await;

    let s = wait_settled(&r, Duration::from_secs(600), 2).await;
    assert_eq!(s.safe_apply.phase, Phase::Committed);
    assert_eq!(r.mock.applied(), vec![cfg(&[3]), initial()]);
}

#[tokio::test(start_paused = true)]
async fn primary_band_is_one_step_when_it_cannot_help() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_with(new_mock(), dir.path(), primary(3, false));
    connect(&mut r).await;
    // B3 not in the set: one step.
    send(&r.h, Command::Apply(cfg(&[28]))).await;
    sleep(Duration::from_secs(2)).await;
    assert_eq!(r.h.snapshots.borrow().safe_apply.sequence, None);
    wait_settled(&r, Duration::from_secs(300), 1).await;
    // B3 alone: one step.
    send(&r.h, Command::Apply(cfg(&[3]))).await;
    sleep(Duration::from_secs(2)).await;
    assert_eq!(r.h.snapshots.borrow().safe_apply.sequence, None);
    wait_settled(&r, Duration::from_secs(300), 2).await;
    assert_eq!(r.mock.applied(), vec![cfg(&[28]), cfg(&[3])]);
    // Already on B3 alone: step 1 would change nothing, so the set is applied in one step.
    send(&r.h, Command::Apply(cfg(&[1, 3]))).await;
    sleep(Duration::from_secs(2)).await;
    assert_eq!(r.h.snapshots.borrow().safe_apply.sequence, None);
    wait_settled(&r, Duration::from_secs(300), 3).await;
    assert_eq!(r.mock.applied().last(), Some(&cfg(&[1, 3])));
}

#[tokio::test(start_paused = true)]
async fn failed_step_one_stops_the_sequence() {
    let dir = tempfile::tempdir().unwrap();
    // B7 has no cell: step 1 never re-attaches.
    let mut r = spawn_with(new_mock(), dir.path(), primary(7, false));
    connect(&mut r).await;
    send(&r.h, Command::Apply(cfg(&[3, 7]))).await;
    let s = wait_settled(&r, Duration::from_secs(200), 2).await;
    assert!(matches!(s.safe_apply.phase, Phase::Reverted { .. }));
    sleep(Duration::from_secs(30)).await;
    assert_eq!(r.mock.applied(), vec![cfg(&[7]), initial()], "no step 2");
    let events = automation_events(&drain_all(&mut r.alerts));
    assert_eq!(
        events,
        vec![AutomationEvent::PrimaryLockFailed {
            primary: 7,
            origin: SequenceOrigin::User
        }]
    );
}

#[tokio::test(start_paused = true)]
async fn failed_step_two_reverts_to_the_config_from_before_step_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_with(new_mock(), dir.path(), primary(3, false));
    connect(&mut r).await;
    // The router "accepts" step 2 but keeps B3 alone: a verify mismatch.
    *r.probe.swallow_apply.lock().unwrap() = Some(cfg(&[1, 3, 20]));
    send(&r.h, Command::Apply(cfg(&[1, 3, 20]))).await;
    let s = wait_settled(&r, Duration::from_secs(600), 2).await;
    assert_eq!(
        s.safe_apply.phase,
        Phase::Reverted {
            reason: RevertReason::VerifyMismatch,
            fallback: false
        }
    );
    assert_eq!(r.mock.applied(), vec![cfg(&[3]), initial()]);
    let events = automation_events(&drain_all(&mut r.alerts));
    assert_eq!(
        events,
        vec![AutomationEvent::WidenFailed {
            primary: 3,
            origin: SequenceOrigin::User,
            ended_on: WidenFallback::Original,
        }]
    );
}

#[tokio::test(start_paused = true)]
async fn refused_step_two_leaves_the_primary_band() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_with(new_mock(), dir.path(), primary(3, false));
    connect(&mut r).await;
    *r.probe.refuse_apply.lock().unwrap() = Some(cfg(&[1, 3, 20]));
    send(&r.h, Command::Apply(cfg(&[1, 3, 20]))).await;
    let mut rx = r.h.snapshots.clone();
    wait_for(&mut rx, Duration::from_secs(300), "step 2", |s| {
        s.safe_apply.sequence.as_ref().is_some_and(|q| q.step == 2)
    })
    .await;
    wait_settled(&r, Duration::from_secs(60), 1).await;
    assert_eq!(r.mock.applied(), vec![cfg(&[3])]);
    let events = automation_events(&drain_all(&mut r.alerts));
    assert_eq!(
        events,
        vec![AutomationEvent::WidenFailed {
            primary: 3,
            origin: SequenceOrigin::User,
            ended_on: WidenFallback::PrimaryOnly,
        }]
    );
}

#[tokio::test(start_paused = true)]
async fn revert_now_in_step_one_ends_the_sequence() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_with(new_mock(), dir.path(), primary(3, false));
    connect(&mut r).await;
    send(&r.h, Command::Apply(cfg(&[1, 3, 20]))).await;
    wait_phase(&r, Duration::from_secs(60), "probation", |p| {
        *p == Phase::Probation
    })
    .await;
    // A reboot isn't offered while the sequence runs.
    assert!(request_reboot(&r.h).await.is_err());
    send(&r.h, Command::RevertNow).await;
    wait_settled(&r, Duration::from_secs(120), 2).await;
    sleep(Duration::from_secs(30)).await;
    assert_eq!(r.mock.applied(), vec![cfg(&[3]), initial()]);
    // The user's own revert isn't reported as a failed step.
    assert!(automation_events(&drain_all(&mut r.alerts)).is_empty());
    assert!(event_log(dir.path()).contains("reverted_by_user"));
}

#[tokio::test(start_paused = true)]
async fn relock_needs_the_setting_and_waits_120_s() {
    let dir = tempfile::tempdir().unwrap();
    // Primary set but re-lock off: nothing happens (the router serves B28 on Auto).
    let mut r = spawn_with(new_mock(), dir.path(), primary(3, false));
    connect(&mut r).await;
    sleep(Duration::from_secs(600)).await;
    assert!(r.mock.applied().is_empty());
    assert_eq!(r.h.snapshots.borrow().relock, None);

    send(&r.h, Command::SetAutomation(primary(3, true))).await;
    sleep(Duration::from_secs(110)).await;
    assert!(r.mock.applied().is_empty(), "not before 120 s off B3");
    sleep(Duration::from_secs(20)).await;
    assert_eq!(r.mock.applied(), vec![cfg(&[3])]);
    let s = r.h.snapshots.borrow().clone();
    let q = s.safe_apply.sequence.expect("relock sequence");
    assert_eq!(q.origin, SequenceOrigin::Relock);
    assert_eq!(q.final_config, initial());
    assert!(automation_events(&drain_all(&mut r.alerts)).contains(
        &AutomationEvent::RelockStarted {
            primary: 3,
            serving_band: 28
        }
    ));
}

#[tokio::test(start_paused = true)]
async fn relock_success_resets_failures() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_with(new_mock(), dir.path(), primary(3, true));
    connect(&mut r).await;
    let mut rx = r.h.snapshots.clone();
    wait_for(&mut rx, Duration::from_secs(200), "relock", |s| {
        s.safe_apply.sequence.is_some()
    })
    .await;
    // From here on the router keeps B3 as the serving band.
    *r.probe.band_override.lock().unwrap() = Some(3);
    let s = wait_settled(&r, Duration::from_secs(600), 2).await;
    assert_eq!(r.mock.applied(), vec![cfg(&[3]), initial()]);
    let v = s.relock.expect("relock view");
    assert_eq!((v.primary, v.failures, v.given_up), (3, 0, false));
    assert!(v.next_try_in_s.is_some(), "rate limit running");
    // On B3: no further attempt.
    sleep(Duration::from_secs(3600)).await;
    assert_eq!(r.mock.applied().len(), 2);
    let log = event_log(dir.path());
    assert_eq!(log.matches("\"relock.started\"").count(), 1);
    assert!(log.contains("\"ok\":true"));
}

#[tokio::test(start_paused = true)]
async fn relock_rate_limit_and_give_up() {
    let dir = tempfile::tempdir().unwrap();
    // The mock always goes back to B28 on Auto: every attempt fails.
    let mut r = spawn_with(new_mock(), dir.path(), primary(3, true));
    connect(&mut r).await;
    wait_settled(&r, Duration::from_secs(1200), 2).await;
    assert_eq!(r.h.snapshots.borrow().relock.unwrap().failures, 1);
    // The next attempt waits for the 30-minute limit.
    sleep(Duration::from_secs(20 * 60)).await;
    assert_eq!(r.mock.applied().len(), 2);
    wait_settled(&r, Duration::from_secs(20 * 60), 4).await;
    assert_eq!(r.h.snapshots.borrow().relock.unwrap().failures, 2);
    let s = wait_settled(&r, Duration::from_secs(40 * 60), 6).await;
    let v = s.relock.unwrap();
    assert_eq!((v.failures, v.given_up), (3, true));
    let events = automation_events(&drain_all(&mut r.alerts));
    assert!(events.contains(&AutomationEvent::RelockGaveUp {
        primary: 3,
        attempts: 3
    }));
    // Given up: no fourth attempt.
    sleep(Duration::from_secs(90 * 60)).await;
    assert_eq!(r.mock.applied().len(), 6);
    let log = event_log(dir.path());
    assert_eq!(log.matches("\"relock.started\"").count(), 3);
    assert_eq!(log.matches("\"relock.gave_up\"").count(), 1);
    assert_eq!(log.matches("\"relock.outcome\"").count(), 3);

    // A settings change clears it.
    send(
        &r.h,
        Command::SetAutomation(AutomationConfig {
            revert_on_loss: false,
            ..primary(3, true)
        }),
    )
    .await;
    sleep(Duration::from_secs(1)).await;
    let v = r.h.snapshots.borrow().relock.unwrap();
    assert_eq!((v.failures, v.given_up), (0, false));
    sleep(Duration::from_secs(200)).await;
    assert_eq!(r.mock.applied().len(), 7, "watching again");
}

#[tokio::test(start_paused = true)]
async fn relock_serving_primary_for_10_min_clears_give_up() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_with(new_mock(), dir.path(), primary(3, true));
    connect(&mut r).await;
    wait_settled(&r, Duration::from_secs(1200), 2).await;
    assert_eq!(r.h.snapshots.borrow().relock.unwrap().failures, 1);
    *r.probe.band_override.lock().unwrap() = Some(3);
    sleep(Duration::from_secs(9 * 60)).await;
    assert_eq!(r.h.snapshots.borrow().relock.unwrap().failures, 1);
    sleep(Duration::from_secs(2 * 60)).await;
    assert_eq!(r.h.snapshots.borrow().relock.unwrap().failures, 0);
    assert!(event_log(dir.path()).contains("\"relock.reset\""));
}

#[tokio::test(start_paused = true)]
async fn relock_never_runs_next_to_band_runner() {
    let dir = tempfile::tempdir().unwrap();
    let lock = dir.path().join("band-runner.lock");
    std::fs::write(&lock, "1234").unwrap();
    let mut r = spawn_with(new_mock(), dir.path(), primary(3, true));
    connect(&mut r).await;
    sleep(Duration::from_secs(600)).await;
    assert!(r.mock.applied().is_empty());
    std::fs::remove_file(&lock).unwrap();
    sleep(Duration::from_secs(5)).await;
    assert_eq!(r.mock.applied(), vec![cfg(&[3])]);
}

#[tokio::test(start_paused = true)]
async fn relock_never_runs_with_a_foreign_journal() {
    let dir = tempfile::tempdir().unwrap();
    let rec = JournalRecord {
        version: JOURNAL_VERSION,
        id: "bbbbbbbb-2222-4333-8444-555555555555".into(),
        driver: "mock".into(),
        host: "10.0.0.1".into(),
        previous: cfg(&[1]),
        requested: cfg(&[3]),
        phase: JournalPhase::Probation,
        phase_changed_at_ms: 1,
        created_at_ms: 1,
    };
    Journal::new(dir.path().to_path_buf()).write(&rec).unwrap();
    let mut r = spawn_with(new_mock(), dir.path(), primary(3, true));
    connect(&mut r).await;
    sleep(Duration::from_secs(600)).await;
    assert!(r.mock.applied().is_empty());
    assert!(!event_log(dir.path()).contains("\"relock.started\""));
}

#[tokio::test(start_paused = true)]
async fn relock_waits_while_a_change_is_open() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_with(new_mock(), dir.path(), primary(3, true));
    connect(&mut r).await;
    // A user change to B28 alone: B3 isn't allowed, so no watchdog afterwards either.
    send(&r.h, Command::Apply(cfg(&[28]))).await;
    wait_settled(&r, Duration::from_secs(400), 1).await;
    sleep(Duration::from_secs(600)).await;
    assert_eq!(r.mock.applied(), vec![cfg(&[28])]);
}

// ---------------------------------------------------------------------------------------------
// Two-step sequences across restarts (pending-sequence.json)

const SEQ_FILE: &str = "pending-sequence.json";

fn seq_record(step: u8, change: Option<&str>) -> SequenceRecord {
    SequenceRecord {
        version: SEQUENCE_VERSION,
        driver: "mock".into(),
        host: HOST.into(),
        origin: SequenceOrigin::User,
        primary: 3,
        step,
        final_config: cfg(&[1, 3, 20]),
        original: Some(initial()),
        change: change.map(str::to_owned),
        created_at_ms: 1,
        step1: None,
    }
}

/// A mock router that already has the given config.
fn mock_on(config: NetConfig) -> Arc<MockDriver> {
    Arc::new(MockDriver::new(MockScenario {
        initial: config,
        ..MockScenario::zain_like()
    }))
}

fn ignored_files(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("pending-sequence.ignored-")
        })
        .count()
}

#[tokio::test(start_paused = true)]
async fn restart_during_step_one_still_runs_step_two() {
    let dir = tempfile::tempdir().unwrap();
    let mock = new_mock();
    let mut r = spawn_with(mock.clone(), dir.path(), primary(3, false));
    connect(&mut r).await;
    send(&r.h, Command::Apply(cfg(&[1, 3, 20]))).await;
    wait_phase(&r, Duration::from_secs(60), "step 1 probation", |p| {
        *p == Phase::Probation
    })
    .await;
    assert!(dir.path().join(SEQ_FILE).exists(), "sequence on file");
    assert!(r.h.clone().shutdown(Duration::from_secs(5)).await);
    drop(r);

    sleep(Duration::from_secs(30)).await;
    let mut r = spawn_with(mock.clone(), dir.path(), primary(3, false));
    let mut rx = r.h.snapshots.clone();
    let s = wait_for(&mut rx, Duration::from_secs(2), "restored step 1", |s| {
        s.safe_apply.sequence.is_some()
    })
    .await;
    assert_eq!(s.safe_apply.sequence.unwrap().step, 1);
    connect(&mut r).await;
    let s = wait_settled(&r, Duration::from_secs(900), 2).await;
    assert_eq!(s.safe_apply.phase, Phase::Committed);
    assert_eq!(mock.applied(), vec![cfg(&[3]), cfg(&[1, 3, 20])]);
    assert!(automation_events(&drain_all(&mut r.alerts)).contains(
        &AutomationEvent::SequenceResumed {
            primary: 3,
            origin: SequenceOrigin::User,
            step: 1
        }
    ));
    assert!(!dir.path().join(SEQ_FILE).exists(), "cleared at the end");
    assert!(event_log(dir.path()).contains("\"sequence.resumed\""));
}

#[tokio::test(start_paused = true)]
async fn step_two_due_at_launch_runs_once_connected() {
    let dir = tempfile::tempdir().unwrap();
    SequenceStore::new(dir.path().to_path_buf())
        .write(&seq_record(2, None))
        .unwrap();
    let mock = mock_on(cfg(&[3]));
    let mut r = spawn_with(mock.clone(), dir.path(), primary(3, false));
    sleep(Duration::from_secs(5)).await;
    assert!(mock.applied().is_empty(), "nothing before the login");
    connect(&mut r).await;
    let s = wait_settled(&r, Duration::from_secs(600), 1).await;
    assert_eq!(s.safe_apply.phase, Phase::Committed);
    assert_eq!(mock.applied(), vec![cfg(&[1, 3, 20])]);
    let events = automation_events(&drain_all(&mut r.alerts));
    assert_eq!(
        events,
        vec![AutomationEvent::SequenceResumed {
            primary: 3,
            origin: SequenceOrigin::User,
            step: 2
        }]
    );
    assert!(!dir.path().join(SEQ_FILE).exists());
}

/// Step 2 is only due when step 1 committed (its archived journal says so).
#[tokio::test(start_paused = true)]
async fn step_one_finished_while_closed() {
    let id = "cccccccc-2222-4333-8444-555555555555";
    for (phase, step2) in [
        (JournalPhase::Committed, true),
        (JournalPhase::Reverted, false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let rec = JournalRecord {
            version: JOURNAL_VERSION,
            id: id.into(),
            driver: "mock".into(),
            host: HOST.into(),
            previous: initial(),
            requested: cfg(&[3]),
            phase,
            phase_changed_at_ms: 2,
            created_at_ms: 1,
        };
        let journal = Journal::new(dir.path().to_path_buf());
        journal.write(&rec).unwrap();
        journal.archive(&rec).unwrap();
        SequenceStore::new(dir.path().to_path_buf())
            .write(&seq_record(1, Some(id)))
            .unwrap();
        let mock = mock_on(cfg(&[3]));
        let mut r = spawn_with(mock.clone(), dir.path(), primary(3, false));
        connect(&mut r).await;
        sleep(Duration::from_secs(400)).await;
        let want = if step2 {
            vec![cfg(&[1, 3, 20])]
        } else {
            vec![]
        };
        assert_eq!(mock.applied(), want, "{phase:?}");
        assert_eq!(r.h.snapshots.borrow().safe_apply.sequence, None);
        assert!(!dir.path().join(SEQ_FILE).exists(), "{phase:?}");
    }
}

#[tokio::test(start_paused = true)]
async fn unusable_sequence_files_never_change_bands() {
    let foreign = SequenceRecord {
        host: "10.0.0.1".into(),
        ..seq_record(2, None)
    };
    let other_driver = SequenceRecord {
        driver: "huawei_h122".into(),
        ..seq_record(2, None)
    };
    let not_two_step = SequenceRecord {
        final_config: cfg(&[3]),
        ..seq_record(2, None)
    };
    let cases: Vec<Option<SequenceRecord>> = vec![
        None,
        Some(foreign),
        Some(other_driver),
        Some(not_two_step),
        // Step 1 never got a journal.
        Some(seq_record(1, None)),
    ];
    for case in cases {
        let dir = tempfile::tempdir().unwrap();
        match &case {
            Some(rec) => SequenceStore::new(dir.path().to_path_buf())
                .write(rec)
                .unwrap(),
            None => std::fs::write(dir.path().join(SEQ_FILE), "{ not json").unwrap(),
        }
        let mock = mock_on(cfg(&[3]));
        let mut r = spawn_with(mock.clone(), dir.path(), primary(3, false));
        connect(&mut r).await;
        sleep(Duration::from_secs(600)).await;
        assert!(mock.applied().is_empty(), "{case:?}");
        assert_eq!(r.h.snapshots.borrow().safe_apply.sequence, None);
        assert_eq!(
            automation_events(&drain_all(&mut r.alerts)),
            vec![AutomationEvent::SequenceDiscarded],
            "{case:?}"
        );
        assert!(!dir.path().join(SEQ_FILE).exists(), "{case:?}");
        assert_eq!(ignored_files(dir.path()), 1, "kept aside: {case:?}");
        assert!(event_log(dir.path()).contains("\"sequence.discarded\""));
        // New sequences still work afterwards (B3 alone already: one step).
        send(&r.h, Command::Apply(cfg(&[1, 3, 20]))).await;
        wait_settled(&r, Duration::from_secs(600), 1).await;
        assert_eq!(mock.applied(), vec![cfg(&[1, 3, 20])], "{case:?}");
    }
}

#[tokio::test(start_paused = true)]
async fn sequence_not_matching_the_journal_is_set_aside() {
    let dir = tempfile::tempdir().unwrap();
    // A step-1 sequence for change X, but the open journal is change Y (B28 alone).
    Journal::new(dir.path().to_path_buf())
        .write(&JournalRecord {
            version: JOURNAL_VERSION,
            id: "dddddddd-2222-4333-8444-555555555555".into(),
            driver: "mock".into(),
            host: HOST.into(),
            previous: initial(),
            requested: cfg(&[28]),
            phase: JournalPhase::Probation,
            phase_changed_at_ms: 1,
            created_at_ms: 1,
        })
        .unwrap();
    SequenceStore::new(dir.path().to_path_buf())
        .write(&seq_record(1, Some("eeeeeeee-2222-4333-8444-555555555555")))
        .unwrap();
    let mock = mock_on(cfg(&[28]));
    let mut r = spawn_with(mock.clone(), dir.path(), primary(3, false));
    connect(&mut r).await;
    sleep(Duration::from_secs(600)).await;
    assert_eq!(
        phase(&r),
        Phase::Committed,
        "the B28 change is still watched"
    );
    assert!(mock.applied().is_empty(), "no step 2");
    assert_eq!(r.h.snapshots.borrow().safe_apply.sequence, None);
    assert_eq!(
        automation_events(&drain_all(&mut r.alerts)),
        vec![AutomationEvent::SequenceDiscarded]
    );
}

// ---------------------------------------------------------------------------------------------
// Re-lock reset gating and speed tests

#[tokio::test(start_paused = true)]
async fn relock_own_step_one_does_not_reset_the_failure_count() {
    let dir = tempfile::tempdir().unwrap();
    let probe = Probe::new(new_mock());
    // Step 1 alone holds B3 for longer than the 10-minute reset.
    let timers = SafeApplyTimers {
        probation: Duration::from_secs(600),
        ..SafeApplyTimers::default()
    };
    let h = spawn_actor(
        probe.clone(),
        ActorConfig {
            timers,
            automation: primary(3, true),
            ..actor_cfg(dir.path())
        },
    );
    let alerts = h.alerts.subscribe();
    let mut r = Rig {
        mock: probe.mock.clone(),
        probe,
        h,
        alerts,
    };
    connect(&mut r).await;
    wait_settled(&r, Duration::from_secs(2000), 2).await;
    assert_eq!(r.h.snapshots.borrow().relock.unwrap().failures, 1);
    wait_settled(&r, Duration::from_secs(4000), 4).await;
    assert_eq!(r.h.snapshots.borrow().relock.unwrap().failures, 2);
    assert!(!event_log(dir.path()).contains("\"relock.reset\""));
}

#[tokio::test(start_paused = true)]
async fn apply_is_refused_while_a_speed_test_runs() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect(&mut r).await;
    send(&r.h, Command::SetSpeedTestActive(true)).await;
    send(&r.h, Command::Apply(cfg(&[3]))).await;
    sleep(Duration::from_secs(5)).await;
    assert!(r.mock.applied().is_empty());
    assert!(drain(&mut r.alerts).iter().any(|a| matches!(
        a,
        Alert::SafeApply {
            event: SafeApplyEvent::Rejected { message }
        } if message == SPEED_TEST_RUNNING
    )));
    send(&r.h, Command::SetSpeedTestActive(false)).await;
    send(&r.h, Command::Apply(cfg(&[3]))).await;
    sleep(Duration::from_secs(5)).await;
    assert_eq!(r.mock.applied(), vec![cfg(&[3])]);
}

#[tokio::test(start_paused = true)]
async fn step_two_waits_for_a_speed_test() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_with(new_mock(), dir.path(), primary(3, false));
    connect(&mut r).await;
    send(&r.h, Command::Apply(cfg(&[1, 3, 20]))).await;
    wait_phase(&r, Duration::from_secs(60), "probation", |p| {
        *p == Phase::Probation
    })
    .await;
    send(&r.h, Command::SetSpeedTestActive(true)).await;
    wait_phase(&r, Duration::from_secs(300), "step 1 committed", |p| {
        *p == Phase::Committed
    })
    .await;
    sleep(Duration::from_secs(60)).await;
    assert_eq!(r.mock.applied(), vec![cfg(&[3])], "step 2 waits");
    let q = r.h.snapshots.borrow().safe_apply.sequence.clone().unwrap();
    assert_eq!(q.step, 2);
    send(&r.h, Command::SetSpeedTestActive(false)).await;
    wait_settled(&r, Duration::from_secs(400), 2).await;
    assert_eq!(r.mock.applied(), vec![cfg(&[3]), cfg(&[1, 3, 20])]);
}

#[tokio::test(start_paused = true)]
async fn relock_waits_for_a_speed_test_up_to_its_cap() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_with(new_mock(), dir.path(), primary(3, true));
    connect(&mut r).await;
    send(&r.h, Command::SetSpeedTestActive(true)).await;
    sleep(Duration::from_secs(400)).await;
    assert!(r.mock.applied().is_empty(), "no re-lock during the test");
    // A test that never reports its end stops holding the watchdog after 10 minutes.
    sleep(Duration::from_secs(210)).await;
    assert_eq!(r.mock.applied(), vec![cfg(&[3])]);
}

fn identity(salt: &str) -> RouterIdentity {
    RouterIdentity::new(salt, 1000)
}

fn gated_mock(salt: &str) -> Arc<MockDriver> {
    let mock = new_mock();
    mock.set_identity(Some(identity(salt)));
    mock
}

async fn settle_session(r: &Rig) -> SessionView {
    sleep(Duration::from_secs(5)).await;
    r.h.snapshots.borrow().session.clone()
}

#[tokio::test(start_paused = true)]
async fn automatic_connect_asks_when_no_router_is_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let mock = gated_mock("aa");
    let r = spawn_on(mock.clone(), dir.path());
    send(&r.h, Command::ConnectAutomatic(creds())).await;
    assert_eq!(
        settle_session(&r).await,
        SessionView::ConfirmRouter { first_time: true }
    );
    // It stays asked: no retry sends anything by itself.
    sleep(Duration::from_secs(300)).await;
    assert_eq!(mock.login_attempts(), 0);
}

#[tokio::test(start_paused = true)]
async fn pressing_connect_records_a_first_router_and_automatic_connect_then_trusts_it() {
    let dir = tempfile::tempdir().unwrap();
    let mock = gated_mock("aa");
    let r = spawn_on(mock.clone(), dir.path());
    send(&r.h, Command::Connect(creds())).await;
    assert_eq!(settle_session(&r).await, SessionView::LoggedIn);
    assert_eq!(
        identity_file::load(dir.path(), HOST, "admin"),
        Some(identity("aa"))
    );
    assert!(r.h.shutdown(Duration::from_secs(15)).await);

    // The next launch.
    let r = spawn_on(mock.clone(), dir.path());
    send(&r.h, Command::ConnectAutomatic(creds())).await;
    assert_eq!(settle_session(&r).await, SessionView::LoggedIn);
}

#[tokio::test(start_paused = true)]
async fn a_changed_router_gets_no_proof_until_the_user_trusts_it() {
    let dir = tempfile::tempdir().unwrap();
    identity_file::save(dir.path(), HOST, "admin", &identity("aa")).unwrap();
    let mock = gated_mock("bb");
    let r = spawn_on(mock.clone(), dir.path());

    for connect in [
        Command::ConnectAutomatic(creds()),
        Command::Connect(creds()),
    ] {
        send(&r.h, connect).await;
        assert_eq!(
            settle_session(&r).await,
            SessionView::ConfirmRouter { first_time: false }
        );
        assert_eq!(mock.login_attempts(), 0);
    }

    send(&r.h, Command::ConnectTrusting(creds())).await;
    assert_eq!(settle_session(&r).await, SessionView::LoggedIn);
    assert_eq!(mock.login_attempts(), 1);
    assert_eq!(
        identity_file::load(dir.path(), HOST, "admin"),
        Some(identity("bb"))
    );
}

#[tokio::test(start_paused = true)]
async fn a_failed_login_records_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mock = gated_mock("aa");
    let r = spawn_on(mock.clone(), dir.path());
    send(&r.h, Command::Connect(Credentials::new("admin", "wrong"))).await;
    assert_eq!(settle_session(&r).await, SessionView::BadCredentials);
    assert_eq!(identity_file::load(dir.path(), HOST, "admin"), None);
}

/// A verified run keeps logging back in, or a band change could lose its way to revert.
#[tokio::test(start_paused = true)]
async fn a_verified_run_can_log_back_in_when_the_salt_changes() {
    let dir = tempfile::tempdir().unwrap();
    let mock = gated_mock("aa");
    let r = spawn_on(mock.clone(), dir.path());
    send(&r.h, Command::Connect(creds())).await;
    assert_eq!(settle_session(&r).await, SessionView::LoggedIn);

    mock.set_identity(Some(identity("bb")));
    mock.drop_session();
    sleep(Duration::from_secs(400)).await;
    assert_eq!(r.h.snapshots.borrow().session, SessionView::LoggedIn);
    // The new salt got recorded, so the login really went through after it changed.
    assert_eq!(
        identity_file::load(dir.path(), HOST, "admin"),
        Some(identity("bb"))
    );
    assert!(mock.login_attempts() >= 2);
}

#[tokio::test(start_paused = true)]
async fn the_demo_driver_has_nothing_to_check() {
    let dir = tempfile::tempdir().unwrap();
    let r = spawn_on(new_mock(), dir.path());
    send(&r.h, Command::ConnectAutomatic(creds())).await;
    assert_eq!(settle_session(&r).await, SessionView::LoggedIn);
    assert_eq!(identity_file::load(dir.path(), HOST, "admin"), None);
}

// ---------------------------------------------------------------------------------------------
// Log out

async fn logout(h: &ActorHandle) -> Result<LogoutOutcome, RouterError> {
    let (tx, rx) = oneshot::channel();
    send(h, Command::Logout(tx)).await;
    rx.await.unwrap()
}

fn events_log(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("events.jsonl")).unwrap_or_default()
}

#[tokio::test(start_paused = true)]
async fn logout_ends_the_session_and_sends_nothing_until_connect() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect(&mut r).await;
    assert!(r.mock.is_logged_in());

    assert_eq!(logout(&r.h).await, Ok(LogoutOutcome { confirmed: true }));
    assert_eq!(r.mock.logouts(), 1);
    assert!(!r.mock.is_logged_in(), "the router's session ended");
    let s = r.h.snapshots.borrow().clone();
    assert_eq!(s.session, SessionView::LoggedOut);
    // No stale values from the old session.
    assert!(s.signal.is_none() && s.status.is_none() && s.config.is_none());
    assert!(s.capabilities.is_none() && s.device.is_none());
    assert!(!s.in_service);
    assert!(logout_marker::is_marked(dir.path(), HOST, "admin"));
    // The saved identity stays, so the next Connect needs no question.
    let log = events_log(dir.path());
    assert!(log.contains("\"session.logout\""), "{log}");
    assert!(log.contains("\"router_state\":\"loggedOut\""), "{log}");

    r.probe.counts.reset();
    sleep(Duration::from_secs(600)).await;
    assert_eq!(r.probe.counts.total(), 0, "no router call while logged out");
    assert_eq!(r.h.snapshots.borrow().session, SessionView::LoggedOut);
    assert!(drain(&mut r.alerts).contains(&Alert::Session {
        view: SessionView::LoggedOut
    }));

    // Nothing else reaches the router either.
    send(&r.h, Command::Apply(cfg(&[3]))).await;
    let ticket = request_reboot(&r.h).await.unwrap();
    assert_eq!(
        confirm_reboot(&r.h, &ticket.id).await,
        Err(RouterError::SessionLost { code: None })
    );
    sleep(Duration::from_secs(5)).await;
    assert_eq!(r.probe.counts.total(), 0);
    assert!(r.mock.applied().is_empty());

    // Connect clears the marker and logs in again.
    connect(&mut r).await;
    assert!(!logout_marker::is_marked(dir.path(), HOST, "admin"));
}

#[tokio::test(start_paused = true)]
async fn logout_is_refused_while_something_holds_the_router() {
    #[derive(Debug, Clone, Copy)]
    enum Busy {
        SpeedTest,
        Scan,
        BandChange,
    }
    let table = [
        (Busy::SpeedTest, SPEED_TEST_RUNNING),
        (Busy::Scan, CHANGE_IN_PROGRESS),
        (Busy::BandChange, CHANGE_IN_PROGRESS),
    ];
    for (busy, refusal) in table {
        let dir = tempfile::tempdir().unwrap();
        let mut r = spawn_on(new_mock(), dir.path());
        connect(&mut r).await;
        match busy {
            Busy::SpeedTest => send(&r.h, Command::SetSpeedTestActive(true)).await,
            Busy::Scan => send(&r.h, Command::SetScanActive(true)).await,
            Busy::BandChange => {
                send(&r.h, Command::Apply(cfg(&[3]))).await;
                wait_phase(&r, Duration::from_secs(60), "probation", |p| {
                    *p == Phase::Probation
                })
                .await;
            }
        }
        assert_eq!(
            logout(&r.h).await,
            Err(RouterError::Validation {
                message: refusal.to_string()
            }),
            "logout while {busy:?}"
        );
        assert_eq!(r.mock.logouts(), 0);
        assert!(!logout_marker::is_marked(dir.path(), HOST, "admin"));
        assert_eq!(r.h.snapshots.borrow().session, SessionView::LoggedIn);
    }
}

#[tokio::test(start_paused = true)]
async fn a_failed_or_slow_router_logout_still_logs_out() {
    for slow in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut r = spawn_on(new_mock(), dir.path());
        connect(&mut r).await;
        if slow {
            r.mock.set_logout_delay(Duration::from_secs(30));
        } else {
            r.mock
                .set_logout_error(Some(api(100002, ApiErrorKind::NotSupported)));
        }
        let started = Instant::now();
        assert_eq!(logout(&r.h).await, Ok(LogoutOutcome { confirmed: false }));
        assert!(started.elapsed() <= Duration::from_secs(6), "slow={slow}");
        assert_eq!(r.h.snapshots.borrow().session, SessionView::LoggedOut);
        assert!(logout_marker::is_marked(dir.path(), HOST, "admin"));
        let log = events_log(dir.path());
        assert!(log.contains("\"session.logout\""), "{log}");
        assert!(log.contains("\"ok\":false"), "{log}");
        r.probe.counts.reset();
        sleep(Duration::from_secs(300)).await;
        assert_eq!(r.probe.counts.total(), 0, "slow={slow}");
    }
}

#[tokio::test(start_paused = true)]
async fn the_next_launch_stays_logged_out_until_connect() {
    let dir = tempfile::tempdir().unwrap();
    let mock = new_mock();
    let mut r = spawn_on(mock.clone(), dir.path());
    connect(&mut r).await;
    logout(&r.h).await.unwrap();
    assert!(r.h.shutdown(Duration::from_secs(15)).await);

    let attempts = mock.login_attempts();
    let r = spawn_on(mock.clone(), dir.path());
    send(&r.h, Command::ConnectAutomatic(creds())).await;
    assert_eq!(settle_session(&r).await, SessionView::LoggedOut);
    sleep(Duration::from_secs(300)).await;
    assert_eq!(mock.login_attempts(), attempts, "no login by itself");
    assert_eq!(r.probe.counts.total(), 0);
    assert!(events_log(dir.path()).contains("\"session.auto_connect_skipped\""));

    send(&r.h, Command::Connect(creds())).await;
    assert_eq!(settle_session(&r).await, SessionView::LoggedIn);
    assert!(!logout_marker::is_marked(dir.path(), HOST, "admin"));
}

#[tokio::test(start_paused = true)]
async fn a_band_change_to_watch_overrides_the_logged_out_marker() {
    let dir = tempfile::tempdir().unwrap();
    logout_marker::mark(dir.path(), HOST, "admin", 1).unwrap();
    Journal::new(dir.path().to_path_buf())
        .write(&JournalRecord {
            version: JOURNAL_VERSION,
            id: "11111111-2222-4333-8444-555555555555".into(),
            driver: "mock".into(),
            host: HOST.into(),
            previous: initial(),
            requested: cfg(&[3]),
            phase: JournalPhase::Probation,
            phase_changed_at_ms: 1,
            created_at_ms: 1,
        })
        .unwrap();
    let mock = mock_on(cfg(&[3]));
    let r = spawn_on(mock.clone(), dir.path());
    send(&r.h, Command::ConnectAutomatic(creds())).await;
    assert_eq!(settle_session(&r).await, SessionView::LoggedIn);
    let log = events_log(dir.path());
    assert!(log.contains("\"session.logout_overridden\""), "{log}");
    // The user didn't press Connect: once the change is over, the next launch stays out.
    assert!(logout_marker::is_marked(dir.path(), HOST, "admin"));
}

#[tokio::test(start_paused = true)]
async fn a_logged_out_marker_for_another_router_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    logout_marker::mark(dir.path(), "192.168.1.1", "admin", 1).unwrap();
    let r = spawn_on(new_mock(), dir.path());
    send(&r.h, Command::ConnectAutomatic(creds())).await;
    assert_eq!(settle_session(&r).await, SessionView::LoggedIn);
}

/// The WAN address is read at login and every 30 s; a new range is logged with both ranges
/// only, and no full address reaches the event log.
#[tokio::test(start_paused = true)]
async fn reads_the_wan_address_and_logs_ranges_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect(&mut r).await;
    let first = mock_wan(0, 0x25_8E);
    let mut rx = r.h.snapshots.clone();
    let s = wait_for(&mut rx, Duration::from_secs(5), "wan", |s| s.wan.is_some()).await;
    let wan = s.wan.unwrap();
    assert_eq!(wan.ipv4, Some(first.to_string()));
    assert_eq!(wan.range.as_deref(), Some("10.45"));
    assert_eq!(wan.kind, Some(WanKind::Private));
    assert!(!wan.ipv6);

    r.probe.counts.reset();
    sleep(Duration::from_secs(61)).await;
    let reads = get(&r.probe.counts.wan);
    assert!((2..=3).contains(&reads), "wan reads in 61 s: {reads}");

    // Data off, then a new address in another range.
    r.mock.set_wan(None);
    wait_for(&mut rx, Duration::from_secs(40), "no address", |s| {
        s.wan.as_ref().is_some_and(|w| w.range.is_none())
    })
    .await;
    let second = mock_wan(2, 0x0B_17);
    r.mock.set_wan(Some(second));
    wait_for(&mut rx, Duration::from_secs(40), "new range", |s| {
        s.wan.as_ref().and_then(|w| w.range.as_deref()) == Some("100.70")
    })
    .await;
    // The same range again logs nothing new.
    r.mock.set_wan(Some(mock_wan(2, 0x0C_01)));
    sleep(Duration::from_secs(40)).await;

    let log = std::fs::read_to_string(dir.path().join("events.jsonl")).unwrap();
    let changes: Vec<serde_json::Value> = log
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|e| e["kind"] == "wan.range_changed")
        .collect();
    assert_eq!(changes.len(), 1, "{log}");
    assert_eq!(changes[0]["detail"]["from"], "10.45");
    assert_eq!(changes[0]["detail"]["to"], "100.70");
    for ip in [first, second, mock_wan(2, 0x0C_01)] {
        assert!(
            !log.contains(&ip.to_string()),
            "full address {ip} in events"
        );
    }

    // Logging out forgets the address.
    let (tx, orx) = oneshot::channel();
    send(&r.h, Command::Logout(tx)).await;
    orx.await.unwrap().unwrap();
    assert_eq!(r.h.snapshots.borrow().wan, None);
}

// ---------------------------------------------------------------------------------------------
// Refresh IP

async fn start_ip_refresh(
    h: &ActorHandle,
    target: Option<&str>,
    max_tries: u8,
) -> Result<(), RouterError> {
    let (tx, rx) = oneshot::channel();
    send(
        h,
        Command::StartIpRefresh {
            target: target.map(str::to_string),
            max_tries,
            reply: tx,
        },
    )
    .await;
    rx.await.unwrap()
}

async fn turn_data_on(h: &ActorHandle) -> Result<(), RouterError> {
    let (tx, rx) = oneshot::channel();
    send(h, Command::TurnDataOn(tx)).await;
    rx.await.unwrap()
}

/// Waits for the end of the current run and returns its outcome.
async fn ip_refresh_ends(r: &Rig, within: Duration) -> IpRefreshOutcome {
    let mut rx = r.h.snapshots.clone();
    let s = wait_for(&mut rx, within, "ip refresh end", |s| {
        s.ip_refresh
            .as_ref()
            .is_some_and(|v| !v.active && v.last.is_some())
    })
    .await;
    s.ip_refresh.unwrap().last.unwrap().outcome
}

fn validation_err(m: &str) -> RouterError {
    RouterError::Validation {
        message: m.to_string(),
    }
}

/// Connects and waits until the WAN range is known, which an IP refresh needs.
async fn connect_with_wan(r: &mut Rig) {
    connect(r).await;
    let mut rx = r.h.snapshots.clone();
    wait_for(&mut rx, Duration::from_secs(5), "wan", |s| {
        s.in_service && s.wan.as_ref().is_some_and(|w| w.range.is_some())
    })
    .await;
}

async fn wait_data_off(r: &Rig) {
    let mut rx = r.h.snapshots.clone();
    wait_for(&mut rx, Duration::from_secs(10), "data off", |s| {
        s.ip_refresh
            .as_ref()
            .is_some_and(|v| v.phase == IpRefreshPhase::Off)
    })
    .await;
}

fn data_journal_path(dir: &Path) -> std::path::PathBuf {
    dir.join("pending-data-switch.json")
}

#[tokio::test(start_paused = true)]
async fn refresh_ip_turns_data_off_and_on_and_logs_ranges_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect_with_wan(&mut r).await;
    let addresses = [mock_wan(1, 0x0102), mock_wan(2, 0x0304)];
    r.mock.queue_wan(&addresses);

    start_ip_refresh(&r.h, Some("100.70"), 3).await.unwrap();
    wait_data_off(&r).await;
    let view = r.h.snapshots.borrow().ip_refresh.clone().unwrap();
    assert_eq!((view.try_n, view.max_tries), (1, 3));
    assert_eq!(view.from_range.as_deref(), Some("10.45"));
    assert!(data_journal_path(dir.path()).exists(), "journal before off");
    assert!(!r.mock.data_on());

    assert_eq!(
        ip_refresh_ends(&r, Duration::from_secs(300)).await,
        IpRefreshOutcome::Matched
    );
    assert_eq!(r.mock.switch_posts(), vec![false, true, false, true]);
    assert!(r.mock.data_on());
    assert!(!data_journal_path(dir.path()).exists());
    let s = r.h.snapshots.borrow().clone();
    let last = s.ip_refresh.unwrap().last.unwrap();
    assert_eq!(last.tries, 2);
    assert_eq!(last.to.as_deref(), Some("100.70"));

    let log = events_log(dir.path());
    for kind in ["ip_refresh.started", "ip_refresh.try", "ip_refresh.ended"] {
        assert!(log.contains(&format!("\"{kind}\"")), "{kind}: {log}");
    }
    for ip in [mock_wan(0, 0x25_8E), addresses[0], addresses[1]] {
        assert!(
            !log.contains(&ip.to_string()),
            "full address {ip} in events"
        );
    }
    let ended = drain_all(&mut r.alerts).into_iter().any(|a| {
        matches!(a, Alert::IpRefresh { event: IpRefreshAlert::Ended { result } }
            if result.outcome == IpRefreshOutcome::Matched)
    });
    assert!(ended);
}

/// While anything else holds the router, Refresh IP is refused; while it runs, everything
/// else is.
#[tokio::test(start_paused = true)]
async fn ip_refresh_refusals_both_ways() {
    #[derive(Debug, Clone, Copy)]
    enum Busy {
        BandChange,
        Scan,
        SpeedTest,
        RebootTicket,
        LoggedOut,
    }
    let table = [
        (Busy::BandChange, CHANGE_IN_PROGRESS),
        (Busy::Scan, CHANGE_IN_PROGRESS),
        (Busy::SpeedTest, SPEED_TEST_RUNNING),
        (Busy::RebootTicket, REBOOT_PENDING),
        (Busy::LoggedOut, NOT_CONNECTED),
    ];
    for (busy, refusal) in table {
        let dir = tempfile::tempdir().unwrap();
        let mut r = spawn_on(new_mock(), dir.path());
        connect_with_wan(&mut r).await;
        match busy {
            Busy::BandChange => {
                send(&r.h, Command::Apply(cfg(&[3]))).await;
                wait_phase(&r, Duration::from_secs(60), "probation", |p| {
                    *p == Phase::Probation
                })
                .await;
            }
            Busy::Scan => send(&r.h, Command::SetScanActive(true)).await,
            Busy::SpeedTest => send(&r.h, Command::SetSpeedTestActive(true)).await,
            Busy::RebootTicket => {
                request_reboot(&r.h).await.unwrap();
            }
            Busy::LoggedOut => {
                logout(&r.h).await.unwrap();
            }
        }
        assert_eq!(
            start_ip_refresh(&r.h, None, 1).await,
            Err(validation_err(refusal)),
            "ip refresh while {busy:?}"
        );
        assert!(r.mock.switch_posts().is_empty(), "{busy:?}");
    }

    // The other way: during a run.
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect_with_wan(&mut r).await;
    start_ip_refresh(&r.h, Some("100"), 5).await.unwrap();
    drain_all(&mut r.alerts);
    send(&r.h, Command::Apply(cfg(&[3]))).await;
    assert_eq!(
        refusal_text(request_reboot(&r.h).await).as_deref(),
        Some(IP_REFRESH_RUNNING)
    );
    assert_eq!(
        refusal_text(request_factory_reset(&r.h).await).as_deref(),
        Some(IP_REFRESH_RUNNING)
    );
    assert_eq!(logout(&r.h).await, Err(validation_err(IP_REFRESH_RUNNING)));
    assert_eq!(
        start_ip_refresh(&r.h, None, 1).await,
        Err(validation_err(IP_REFRESH_RUNNING))
    );
    assert_eq!(
        turn_data_on(&r.h).await,
        Err(validation_err(IP_REFRESH_RUNNING))
    );
    send(&r.h, Command::SetSpeedTestActive(true)).await;
    sleep(Duration::from_secs(1)).await;
    let rejected = drain_all(&mut r.alerts).into_iter().any(|a| {
        a == Alert::SafeApply {
            event: SafeApplyEvent::Rejected {
                message: IP_REFRESH_RUNNING.to_string(),
            },
        }
    });
    assert!(rejected, "apply refused");
    assert!(r.mock.applied().is_empty());
    assert!(events_log(dir.path()).contains("\"conflict\":\"ip_refresh\""));
    assert_eq!(r.h.snapshots.borrow().session, SessionView::LoggedIn);

    // Stop turns data back on.
    send(&r.h, Command::SetSpeedTestActive(false)).await;
    send(&r.h, Command::StopIpRefresh).await;
    assert_eq!(
        ip_refresh_ends(&r, Duration::from_secs(60)).await,
        IpRefreshOutcome::Stopped
    );
    assert!(r.mock.data_on());
    assert_eq!(r.mock.switch_posts().last(), Some(&true));
}

#[tokio::test(start_paused = true)]
async fn ip_refresh_needs_the_switch_and_service() {
    let dir = tempfile::tempdir().unwrap();
    let mock = new_mock();
    mock.set_data_switch_supported(false);
    mock.set_network_search_supported(false);
    let mut r = spawn_on(mock, dir.path());
    connect_with_wan(&mut r).await;
    assert_eq!(
        start_ip_refresh(&r.h, None, 1).await,
        Err(validation_err(router_core::IP_NO_METHOD))
    );

    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect_with_wan(&mut r).await;
    for bad in ["10.45.1", "300", "x"] {
        assert_eq!(
            start_ip_refresh(&r.h, Some(bad), 2).await,
            Err(validation_err(router_core::IP_TARGET_INVALID))
        );
    }
    assert_eq!(
        start_ip_refresh(&r.h, Some("100"), 6).await,
        Err(validation_err(router_core::IP_TRIES_INVALID))
    );
    r.mock.set_fault(Some(Fault::NoServiceForever));
    let mut rx = r.h.snapshots.clone();
    wait_for(&mut rx, Duration::from_secs(10), "no service", |s| {
        !s.in_service
    })
    .await;
    assert_eq!(
        start_ip_refresh(&r.h, None, 1).await,
        Err(validation_err(IP_NOT_IN_SERVICE))
    );
    assert!(r.mock.switch_posts().is_empty());
}

/// A session loss while data is off: the normal relogin, then "on" again.
#[tokio::test(start_paused = true)]
async fn a_session_loss_while_data_is_off_turns_it_back_on() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect_with_wan(&mut r).await;
    start_ip_refresh(&r.h, Some("100"), 5).await.unwrap();
    wait_data_off(&r).await;
    r.mock.drop_session();
    assert_eq!(
        ip_refresh_ends(&r, Duration::from_secs(120)).await,
        IpRefreshOutcome::SessionLost
    );
    assert!(r.mock.data_on());
    assert_eq!(r.mock.switch_posts(), vec![false, true]);
    assert!(!data_journal_path(dir.path()).exists());
    assert_eq!(r.h.snapshots.borrow().session, SessionView::LoggedIn);
}

#[tokio::test(start_paused = true)]
async fn data_that_wont_come_back_on_keeps_the_journal_and_alerts() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect_with_wan(&mut r).await;
    r.mock.fail_data_on(3);
    start_ip_refresh(&r.h, Some("100"), 5).await.unwrap();
    let mut rx = r.h.snapshots.clone();
    wait_for(&mut rx, Duration::from_secs(60), "on failures", |s| {
        s.ip_refresh
            .as_ref()
            .is_some_and(|v| v.data_on_failures >= 2)
    })
    .await;
    assert!(data_journal_path(dir.path()).exists());
    assert_eq!(
        ip_refresh_ends(&r, Duration::from_secs(120)).await,
        IpRefreshOutcome::DataOnFailed
    );
    assert!(r.mock.data_on());
    assert_eq!(r.mock.switch_posts(), vec![false, true], "no second off");
    let alerts = drain_all(&mut r.alerts);
    assert!(alerts.contains(&Alert::IpRefresh {
        event: IpRefreshAlert::DataOnFailed { attempt: 1 }
    }));
    assert!(events_log(dir.path()).contains("\"ip_refresh.data_on_failed\""));
}

fn write_data_journal(dir: &Path, host: &str) {
    DataSwitchStore::new(dir.to_path_buf())
        .write(&DataSwitchRecord {
            version: DATA_SWITCH_VERSION,
            id: "data-1".into(),
            host: host.into(),
            driver: "mock".into(),
            created_at_ms: 1,
            phase: DataSwitchPhase::Off,
        })
        .unwrap();
}

/// The app was closed with data off: it turns data back on after the first login, even with
/// the logged-out marker, and says so.
#[tokio::test(start_paused = true)]
async fn data_left_off_at_launch_is_turned_back_on() {
    let dir = tempfile::tempdir().unwrap();
    write_data_journal(dir.path(), HOST);
    logout_marker::mark(dir.path(), HOST, "admin", 1).unwrap();
    let mock = new_mock();
    mock.set_data_on(false);
    let mut r = spawn_on(mock.clone(), dir.path());
    sleep(Duration::from_secs(10)).await;
    assert!(mock.switch_posts().is_empty(), "nothing before a login");
    send(&r.h, Command::ConnectAutomatic(creds())).await;
    assert_eq!(
        ip_refresh_ends(&r, Duration::from_secs(60)).await,
        IpRefreshOutcome::Resumed
    );
    assert!(mock.data_on());
    assert_eq!(mock.switch_posts(), vec![true]);
    assert!(!data_journal_path(dir.path()).exists());
    let log = events_log(dir.path());
    assert!(log.contains("\"ip_refresh.resumed\""), "{log}");
    assert!(log.contains("\"session.logout_overridden\""), "{log}");
    assert!(!log.contains("\"ip_refresh.data_on_failed\""), "{log}");
    assert!(drain_all(&mut r.alerts).iter().any(|a| matches!(
        a,
        Alert::IpRefresh { event: IpRefreshAlert::Ended { result } }
            if result.outcome == IpRefreshOutcome::Resumed
    )));
}

/// The data-switch record is read before the band journal, and one for another router is
/// shown, never acted on.
#[tokio::test(start_paused = true)]
async fn a_foreign_data_journal_is_shown_and_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    write_data_journal(dir.path(), "192.168.1.1");
    Journal::new(dir.path().to_path_buf())
        .write(&JournalRecord {
            version: JOURNAL_VERSION,
            id: "11111111-2222-4333-8444-555555555555".into(),
            driver: "mock".into(),
            host: HOST.into(),
            previous: initial(),
            requested: cfg(&[3]),
            phase: JournalPhase::Probation,
            phase_changed_at_ms: 1,
            created_at_ms: 1,
        })
        .unwrap();
    let mock = mock_on(cfg(&[3]));
    mock.set_data_on(false);
    let mut r = spawn_on(mock.clone(), dir.path());
    connect(&mut r).await;
    sleep(Duration::from_secs(5)).await;
    let s = r.h.snapshots.borrow().clone();
    assert!(s
        .ip_refresh
        .as_ref()
        .is_some_and(|v| v.journal_blocked && !v.active));
    assert!(mock.switch_posts().is_empty(), "never acted on");
    assert!(data_journal_path(dir.path()).exists());
    let log = events_log(dir.path());
    let foreign = log.find("\"journal.foreign\"").expect("foreign logged");
    let resumed = log
        .find("\"safe_apply.resumed\"")
        .expect("band journal resumed");
    assert!(foreign < resumed, "data journal first: {log}");
    assert_eq!(
        start_ip_refresh(&r.h, None, 1).await,
        Err(validation_err(CHANGE_IN_PROGRESS))
    );
}

/// Data off with no record of ours: only a notice, and the user's button.
#[tokio::test(start_paused = true)]
async fn data_off_without_a_journal_is_only_a_notice() {
    let dir = tempfile::tempdir().unwrap();
    let mock = new_mock();
    mock.set_data_on(false);
    let mut r = spawn_on(mock.clone(), dir.path());
    connect(&mut r).await;
    sleep(Duration::from_secs(30)).await;
    assert!(r.h.snapshots.borrow().data_switch_off);
    assert!(mock.switch_posts().is_empty(), "never turned on by itself");
    turn_data_on(&r.h).await.unwrap();
    assert_eq!(mock.switch_posts(), vec![true]);
    assert!(mock.data_on());
    sleep(Duration::from_secs(1)).await;
    assert!(!r.h.snapshots.borrow().data_switch_off);
    assert!(events_log(dir.path()).contains("\"ip_refresh.manual_on\""));
}

#[tokio::test(start_paused = true)]
async fn a_foreign_data_journal_blocks_refresh_ip() {
    let dir = tempfile::tempdir().unwrap();
    write_data_journal(dir.path(), "192.168.1.1");
    let mut r = spawn_on(new_mock(), dir.path());
    connect_with_wan(&mut r).await;
    assert_eq!(
        start_ip_refresh(&r.h, None, 1).await,
        Err(validation_err(DATA_JOURNAL_BLOCKED))
    );
    assert!(r.mock.switch_posts().is_empty());
}

#[tokio::test(start_paused = true)]
async fn shutdown_with_data_off_turns_it_on_once() {
    let dir = tempfile::tempdir().unwrap();
    let mock = new_mock();
    let mut r = spawn_on(mock.clone(), dir.path());
    connect_with_wan(&mut r).await;
    start_ip_refresh(&r.h, None, 1).await.unwrap();
    wait_data_off(&r).await;
    assert!(r.h.shutdown(Duration::from_secs(15)).await);
    assert_eq!(mock.switch_posts(), vec![false, true]);
    // The record stays for the next launch to confirm.
    assert!(data_journal_path(dir.path()).exists());
}

// ---------------------------------------------------------------------------------------------
// APN profiles

#[tokio::test(start_paused = true)]
async fn apn_profiles_are_read_after_login_every_minute_and_on_request() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect(&mut r).await;
    let mut rx = r.h.snapshots.clone();
    let s = wait_for(&mut rx, Duration::from_secs(5), "apn", |s| s.apn.is_some()).await;
    let apn = s.apn.unwrap();
    assert_eq!(apn.current, Some(1));
    assert_eq!(apn.profiles.len(), 3);
    assert_eq!(apn.auto_apn, Some(false));
    assert_eq!(get(&r.probe.counts.apn), 1, "once at login");
    // Never in the 1 s poll.
    sleep(Duration::from_secs(30)).await;
    assert_eq!(get(&r.probe.counts.apn), 1);
    sleep(Duration::from_secs(32)).await;
    assert_eq!(get(&r.probe.counts.apn), 2, "every 60 s while shown");
    // Hidden: no routine reads, but a request still reads.
    send(&r.h, Command::SetVisible(false)).await;
    sleep(Duration::from_secs(150)).await;
    assert_eq!(get(&r.probe.counts.apn), 2);
    send(&r.h, Command::ApnRefresh).await;
    sleep(Duration::from_secs(1)).await;
    assert_eq!(get(&r.probe.counts.apn), 3);
    // Nothing about APNs goes into the event log.
    let log = events_log(dir.path());
    assert!(
        !log.contains("internet.example") && !log.contains("Operator internet"),
        "{log}"
    );
    logout(&r.h).await.unwrap();
    sleep(Duration::from_secs(1)).await;
    assert_eq!(r.h.snapshots.borrow().apn, None, "forgotten at logout");
}

#[tokio::test(start_paused = true)]
async fn a_router_without_apn_profiles_is_never_asked() {
    let dir = tempfile::tempdir().unwrap();
    let mock = new_mock();
    mock.set_apn_supported(false);
    let mut r = spawn_on(mock, dir.path());
    connect(&mut r).await;
    sleep(Duration::from_secs(130)).await;
    send(&r.h, Command::ApnRefresh).await;
    sleep(Duration::from_secs(1)).await;
    assert_eq!(get(&r.probe.counts.apn), 0);
    assert_eq!(r.h.snapshots.borrow().apn, None);
}

// ---------------------------------------------------------------------------------------------
// APN switch

use router_core::actor::{APN_CHANGE_RUNNING, APN_JOURNAL_BLOCKED};
use router_core::apn::{APN_ARCHIVE_DIR, APN_JOURNAL_FILE};
use router_core::{ApnTarget, APN_AUTO_ON, APN_PROFILE_CHANGED, APN_PROFILE_MISSING};

/// The target for profile `index`, from the last read in the snapshot.
fn apn_target(r: &Rig, index: u32) -> ApnTarget {
    let s = r.h.snapshots.borrow();
    let p = s.apn.as_ref().expect("profiles read").get(index).unwrap();
    ApnTarget {
        index,
        fingerprint: p.fingerprint.clone(),
    }
}

async fn apn_switch(h: &ActorHandle, target: ApnTarget) -> Result<(), RouterError> {
    let (tx, rx) = oneshot::channel();
    send(h, Command::ApnSwitch { target, reply: tx }).await;
    rx.await.unwrap()
}

/// Connects and waits for the profile list and the WAN address.
async fn connect_with_apn(r: &mut Rig) {
    connect_with_wan(r).await;
    let mut rx = r.h.snapshots.clone();
    wait_for(&mut rx, Duration::from_secs(5), "apn", |s| s.apn.is_some()).await;
}

async fn wait_apn(r: &Rig, within: Duration, what: &str, pred: impl Fn(&Phase) -> bool) {
    let mut rx = r.h.snapshots.clone();
    wait_for(&mut rx, within, what, |s| {
        s.apn_apply.as_ref().is_some_and(|v| pred(&v.phase))
    })
    .await;
}

fn apn_journal_path(dir: &Path) -> std::path::PathBuf {
    dir.join(APN_JOURNAL_FILE)
}

#[tokio::test(start_paused = true)]
async fn an_apn_switch_with_service_is_kept_and_logs_indexes_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect_with_apn(&mut r).await;
    assert_eq!(r.h.snapshots.borrow().apn_apply, None, "none yet");
    apn_switch(&r.h, apn_target(&r, 2)).await.unwrap();
    assert!(
        apn_journal_path(dir.path()).exists(),
        "on disk before the POST"
    );
    wait_apn(&r, Duration::from_secs(60), "probation", |p| {
        *p == Phase::Probation
    })
    .await;
    assert_eq!(r.mock.apn_posts(), vec![2]);
    assert_eq!(
        r.h.snapshots.borrow().safe_apply.phase,
        Phase::Idle,
        "bands untouched"
    );
    wait_apn(&r, Duration::from_secs(150), "committed", |p| {
        *p == Phase::Committed
    })
    .await;
    assert_eq!(r.mock.current_apn(), 2);
    assert!(!apn_journal_path(dir.path()).exists());
    assert!(dir.path().join(APN_ARCHIVE_DIR).exists());
    assert!(!dir.path().join("pending-change.json").exists());
    let log = events_log(dir.path());
    for kind in [
        "apn.started",
        "apn.applied",
        "apn.verified",
        "apn.reattached",
        "apn.committed",
    ] {
        assert!(log.contains(&format!("\"{kind}\"")), "{kind}: {log}");
    }
    for secret in [
        "work.example",
        "Work",
        "internet.example",
        "Operator internet",
    ] {
        assert!(!log.contains(secret), "{secret} in {log}");
    }
    let journal = std::fs::read_dir(dir.path().join(APN_ARCHIVE_DIR))
        .unwrap()
        .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
        .collect::<String>();
    assert!(
        !journal.contains("work.example") && journal.contains("\"index\": 2"),
        "{journal}"
    );
    let alerts = drain_all(&mut r.alerts);
    assert!(alerts.iter().any(|a| matches!(
        a,
        Alert::Apn {
            event: SafeApplyEvent::Committed
        }
    )));
}

#[tokio::test(start_paused = true)]
async fn a_wrong_apn_is_undone_by_itself() {
    let dir = tempfile::tempdir().unwrap();
    let mock = new_mock();
    mock.apn_breaks_data(Some(2));
    let mut r = spawn_on(mock.clone(), dir.path());
    connect_with_apn(&mut r).await;
    apn_switch(&r.h, apn_target(&r, 2)).await.unwrap();
    wait_apn(&r, Duration::from_secs(150), "reverted", |p| {
        matches!(
            p,
            Phase::Reverted {
                reason: RevertReason::NoReattach,
                fallback: false
            }
        )
    })
    .await;
    assert_eq!(mock.apn_posts(), vec![2, 1]);
    assert_eq!(mock.current_apn(), 1);
    assert!(!apn_journal_path(dir.path()).exists());
    assert!(events_log(dir.path()).contains("\"apn.reverted\""));
    assert!(drain_all(&mut r.alerts).iter().any(|a| matches!(
        a,
        Alert::Apn {
            event: SafeApplyEvent::Reverted { .. }
        }
    )));
}

#[tokio::test(start_paused = true)]
async fn keep_and_revert_go_to_the_open_apn_switch() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect_with_apn(&mut r).await;
    apn_switch(&r.h, apn_target(&r, 2)).await.unwrap();
    wait_apn(&r, Duration::from_secs(60), "probation", |p| {
        *p == Phase::Probation
    })
    .await;
    send(&r.h, Command::KeepNow).await;
    wait_apn(&r, Duration::from_secs(5), "kept", |p| {
        *p == Phase::Committed
    })
    .await;

    apn_switch(&r.h, apn_target(&r, 3)).await.unwrap();
    wait_apn(&r, Duration::from_secs(60), "probation", |p| {
        *p == Phase::Probation
    })
    .await;
    send(&r.h, Command::RevertNow).await;
    wait_apn(&r, Duration::from_secs(60), "reverted", |p| {
        matches!(
            p,
            Phase::Reverted {
                reason: RevertReason::UserRequested,
                ..
            }
        )
    })
    .await;
    assert_eq!(r.mock.current_apn(), 2);
    assert!(r.mock.applied().is_empty(), "no band change");
}

#[tokio::test(start_paused = true)]
async fn an_open_apn_switch_refuses_everything_else() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect_with_apn(&mut r).await;
    apn_switch(&r.h, apn_target(&r, 2)).await.unwrap();
    wait_apn(&r, Duration::from_secs(60), "probation", |p| {
        *p == Phase::Probation
    })
    .await;
    let busy = Err(validation_err(APN_CHANGE_RUNNING));
    assert_eq!(logout(&r.h).await.map(|_| ()), busy);
    assert_eq!(start_ip_refresh(&r.h, None, 1).await, busy);
    assert_eq!(request_reboot(&r.h).await.map(|_| ()), busy);
    assert_eq!(request_factory_reset(&r.h).await.map(|_| ()), busy);
    assert_eq!(apn_switch(&r.h, apn_target(&r, 3)).await, busy);
    drain_all(&mut r.alerts);
    send(&r.h, Command::Apply(cfg(&[3]))).await;
    sleep(Duration::from_secs(1)).await;
    let alerts = drain_all(&mut r.alerts);
    assert!(alerts.iter().any(|a| matches!(
        a,
        Alert::SafeApply { event: SafeApplyEvent::Rejected { message } } if message == APN_CHANGE_RUNNING
    )), "{alerts:?}");
    assert!(r.mock.applied().is_empty());
    assert!(r.mock.switch_posts().is_empty());
}

#[tokio::test(start_paused = true)]
async fn an_apn_switch_is_refused_while_anything_else_holds_the_router() {
    #[derive(Debug, Clone, Copy)]
    enum Busy {
        SpeedTest,
        Scan,
        BandChange,
        IpRefresh,
        RebootTicket,
    }
    let table = [
        (Busy::SpeedTest, SPEED_TEST_RUNNING),
        (Busy::Scan, CHANGE_IN_PROGRESS),
        (Busy::BandChange, CHANGE_IN_PROGRESS),
        (Busy::IpRefresh, IP_REFRESH_RUNNING),
        (Busy::RebootTicket, REBOOT_PENDING),
    ];
    for (busy, refusal) in table {
        let dir = tempfile::tempdir().unwrap();
        let mut r = spawn_on(new_mock(), dir.path());
        connect_with_apn(&mut r).await;
        match busy {
            Busy::SpeedTest => send(&r.h, Command::SetSpeedTestActive(true)).await,
            Busy::Scan => send(&r.h, Command::SetScanActive(true)).await,
            Busy::BandChange => {
                send(&r.h, Command::Apply(cfg(&[3]))).await;
                wait_phase(&r, Duration::from_secs(60), "probation", |p| {
                    *p == Phase::Probation
                })
                .await;
            }
            Busy::IpRefresh => {
                start_ip_refresh(&r.h, None, 1).await.unwrap();
            }
            Busy::RebootTicket => {
                request_reboot(&r.h).await.unwrap();
            }
        }
        assert_eq!(
            apn_switch(&r.h, apn_target(&r, 2)).await,
            Err(validation_err(refusal)),
            "while {busy:?}"
        );
        assert!(r.mock.apn_posts().is_empty(), "{busy:?}");
        assert_eq!(get(&r.probe.counts.apn_switch), 0, "{busy:?}");
    }
}

#[tokio::test(start_paused = true)]
async fn an_apn_switch_needs_an_unchanged_profile_and_auto_apn_off() {
    let dir = tempfile::tempdir().unwrap();
    let mock = new_mock();
    let mut r = spawn_on(mock.clone(), dir.path());
    connect_with_apn(&mut r).await;
    let stale = apn_target(&r, 2);
    // The profile was edited on the router's own page since the list was read.
    let edited = router_core::drivers::mock::mock_profile(2, "Work", Some("other.example"), false);
    mock.set_apn_profiles(
        1,
        vec![
            router_core::drivers::mock::mock_profile(
                1,
                "Operator internet",
                Some("internet.example"),
                true,
            ),
            edited,
        ],
    );
    send(&r.h, Command::ApnRefresh).await;
    sleep(Duration::from_secs(1)).await;
    assert_eq!(
        apn_switch(&r.h, stale.clone()).await,
        Err(validation_err(APN_PROFILE_CHANGED))
    );
    assert_eq!(
        apn_switch(&r.h, ApnTarget { index: 7, ..stale }).await,
        Err(validation_err(APN_PROFILE_MISSING))
    );
    // Already in use: the safe-apply's own refusal comes back as the reply.
    assert!(apn_switch(&r.h, apn_target(&r, 1)).await.is_err());
    mock.set_auto_apn(true);
    send(&r.h, Command::ApnRefresh).await;
    sleep(Duration::from_secs(1)).await;
    assert_eq!(
        apn_switch(&r.h, apn_target(&r, 2)).await,
        Err(validation_err(APN_AUTO_ON))
    );
    assert!(mock.apn_posts().is_empty());
    assert_eq!(
        r.h.snapshots
            .borrow()
            .apn_apply
            .as_ref()
            .map(|v| v.phase.clone())
            .unwrap_or(Phase::Idle),
        Phase::Idle
    );
}

#[tokio::test(start_paused = true)]
async fn the_driver_checks_the_profile_again_before_its_post() {
    let dir = tempfile::tempdir().unwrap();
    let mock = new_mock();
    let mut r = spawn_on(mock.clone(), dir.path());
    connect_with_apn(&mut r).await;
    let target = apn_target(&r, 2);
    // Edited after the app's last read: the actor can't tell, the driver's own read can.
    mock.set_apn_profiles(
        1,
        vec![
            router_core::drivers::mock::mock_profile(
                1,
                "Operator internet",
                Some("internet.example"),
                true,
            ),
            router_core::drivers::mock::mock_profile(2, "Work", Some("other.example"), false),
        ],
    );
    apn_switch(&r.h, target).await.unwrap();
    sleep(Duration::from_secs(2)).await;
    // Closed as not applied: nothing was sent, and the alert says so.
    assert!(drain_all(&mut r.alerts).iter().any(|a| matches!(
        a,
        Alert::Apn {
            event: SafeApplyEvent::Reverted {
                reason: RevertReason::ApplyRejected,
                ..
            }
        }
    )));
    assert_eq!(r.h.snapshots.borrow().apn_apply, None);
    assert_eq!(get(&r.probe.counts.apn_switch), 1);
    assert!(mock.apn_posts().is_empty());
    assert_eq!(mock.current_apn(), 1);
}

fn write_apn_journal(dir: &Path, host: &str, phase: JournalPhase) {
    let fp = |i: u32| mock_profiles().get(i).unwrap().fingerprint.clone();
    let rec = JournalRecord {
        version: JOURNAL_VERSION,
        id: "aaaaaaaa-2222-4333-8444-555555555555".into(),
        driver: "mock".into(),
        host: host.into(),
        previous: ApnTarget {
            index: 1,
            fingerprint: fp(1),
        },
        requested: ApnTarget {
            index: 2,
            fingerprint: fp(2),
        },
        phase,
        phase_changed_at_ms: 1,
        created_at_ms: 1,
    };
    Journal::with_files(dir.to_path_buf(), APN_JOURNAL_FILE, APN_ARCHIVE_DIR)
        .write_record(&rec)
        .unwrap();
}

/// The mock's own profiles, as it starts.
fn mock_profiles() -> router_core::ApnProfiles {
    router_core::ApnProfiles {
        current: Some(1),
        profiles: vec![
            router_core::drivers::mock::mock_profile(
                1,
                "Operator internet",
                Some("internet.example"),
                true,
            ),
            router_core::drivers::mock::mock_profile(2, "Work", Some("work.example"), false),
            router_core::drivers::mock::mock_profile(3, "Network default", None, false),
        ],
        auto_apn: Some(false),
    }
}

#[tokio::test(start_paused = true)]
async fn an_apn_switch_left_open_at_launch_is_undone_after_login() {
    let dir = tempfile::tempdir().unwrap();
    let mock = new_mock();
    // The app closed in probation on profile 2, which has no data.
    mock.set_apn_profiles(2, mock_profiles().profiles);
    mock.apn_breaks_data(Some(2));
    write_apn_journal(dir.path(), HOST, JournalPhase::Probation);
    logout_marker::mark(dir.path(), HOST, "admin", 1).unwrap();
    let r = spawn_on(mock.clone(), dir.path());
    // A launch auto-connect overrides the logged-out marker to watch it.
    send(&r.h, Command::ConnectAutomatic(creds())).await;
    wait_apn(&r, Duration::from_secs(200), "reverted", |p| {
        matches!(
            p,
            Phase::Reverted {
                reason: RevertReason::ResumedWithoutService,
                ..
            }
        )
    })
    .await;
    assert_eq!(mock.current_apn(), 1);
    assert!(!apn_journal_path(dir.path()).exists());
    let log = events_log(dir.path());
    assert!(log.contains("\"apn.resumed\""), "{log}");
    assert!(log.contains("\"session.logout_overridden\""), "{log}");
    drop(r);
}

#[tokio::test(start_paused = true)]
async fn a_foreign_apn_journal_blocks_apn_switches_only() {
    let dir = tempfile::tempdir().unwrap();
    let mock = new_mock();
    write_apn_journal(dir.path(), "192.168.1.1", JournalPhase::Probation);
    let mut r = spawn_on(mock.clone(), dir.path());
    connect_with_apn(&mut r).await;
    assert_eq!(
        apn_switch(&r.h, apn_target(&r, 2)).await,
        Err(validation_err(APN_JOURNAL_BLOCKED))
    );
    assert!(mock.apn_posts().is_empty());
    assert!(
        apn_journal_path(dir.path()).exists(),
        "kept, never acted on"
    );
    assert!(events_log(dir.path()).contains("\"journal.foreign\""));
    // Bands can still change.
    send(&r.h, Command::Apply(cfg(&[3]))).await;
    wait_phase(&r, Duration::from_secs(60), "probation", |p| {
        *p == Phase::Probation
    })
    .await;
}

// ---------------------------------------------------------------------------------------------
// APN add, edit and delete

use router_core::{
    ApnAuth, ApnDraft, ApnIpType, PasswordChange, Secret, APN_PASSWORD_HIDDEN, APN_PROFILE_IN_USE,
    APN_PROFILE_READ_ONLY,
};

const TYPED_SECRET: &str = "typed-secret-5678";

fn apn_draft(name: &str, password: PasswordChange) -> ApnDraft {
    ApnDraft {
        name: name.into(),
        apn: "travel.example".into(),
        username: "demo".into(),
        password,
        auth: ApnAuth::Pap,
        ip_type: ApnIpType::Ipv4v6,
    }
}

fn typed() -> PasswordChange {
    PasswordChange::Set {
        value: Secret::new(TYPED_SECRET),
    }
}

async fn apn_add(h: &ActorHandle, draft: ApnDraft) -> Result<u32, RouterError> {
    let (tx, rx) = oneshot::channel();
    send(h, Command::ApnAdd { draft, reply: tx }).await;
    rx.await.unwrap()
}

async fn apn_edit(h: &ActorHandle, target: ApnTarget, draft: ApnDraft) -> Result<(), RouterError> {
    let (tx, rx) = oneshot::channel();
    send(
        h,
        Command::ApnEdit {
            target,
            draft,
            reply: tx,
        },
    )
    .await;
    rx.await.unwrap()
}

async fn apn_delete(h: &ActorHandle, target: ApnTarget) -> Result<(), RouterError> {
    let (tx, rx) = oneshot::channel();
    send(h, Command::ApnDelete { target, reply: tx }).await;
    rx.await.unwrap()
}

#[tokio::test(start_paused = true)]
async fn apn_profiles_are_added_edited_and_deleted_with_indexes_only_logged() {
    let dir = tempfile::tempdir().unwrap();
    let mock = new_mock();
    let mut r = spawn_on(mock.clone(), dir.path());
    connect_with_apn(&mut r).await;
    assert!(
        r.h.snapshots
            .borrow()
            .capabilities
            .as_ref()
            .unwrap()
            .apn
            .edit
    );

    let index = apn_add(&r.h, apn_draft("Travel", typed())).await.unwrap();
    assert_eq!(index, 4);
    assert_eq!(mock.apn_password(4).as_deref(), Some(TYPED_SECRET));
    let s = r.h.snapshots.borrow().clone();
    let added = s
        .apn
        .as_ref()
        .unwrap()
        .get(4)
        .expect("read again after the add");
    assert!(added.has_password && added.name == "Travel");

    // Keep: the saved password stays, the rest changes.
    let t = apn_target(&r, 4);
    apn_edit(&r.h, t, apn_draft("Trip", PasswordChange::Keep))
        .await
        .unwrap();
    assert_eq!(mock.apn_password(4).as_deref(), Some(TYPED_SECRET));
    assert_eq!(
        r.h.snapshots
            .borrow()
            .apn
            .as_ref()
            .unwrap()
            .get(4)
            .unwrap()
            .name,
        "Trip"
    );
    apn_edit(
        &r.h,
        apn_target(&r, 4),
        apn_draft("Trip", PasswordChange::Clear),
    )
    .await
    .unwrap();
    assert_eq!(mock.apn_password(4).as_deref(), Some(""));

    apn_delete(&r.h, apn_target(&r, 4)).await.unwrap();
    assert!(r
        .h
        .snapshots
        .borrow()
        .apn
        .as_ref()
        .unwrap()
        .get(4)
        .is_none());
    assert_eq!(
        mock.apn_writes(),
        vec![("add", 4), ("edit", 4), ("edit", 4), ("delete", 4)]
    );
    assert_eq!(mock.current_apn(), 1, "the profile in use never moved");
    assert!(mock.apn_posts().is_empty(), "no switch");
    assert!(!apn_journal_path(dir.path()).exists());

    let log = events_log(dir.path());
    for kind in [
        "apn.profile_added",
        "apn.profile_edited",
        "apn.profile_deleted",
    ] {
        assert!(log.contains(&format!("\"{kind}\"")), "{kind}: {log}");
    }
    for secret in [TYPED_SECRET, "Travel", "Trip", "travel.example", "demo"] {
        assert!(!log.contains(secret), "{secret} in {log}");
    }
}

#[tokio::test(start_paused = true)]
async fn apn_writes_are_refused_on_profiles_in_use_or_the_operators() {
    let dir = tempfile::tempdir().unwrap();
    let mock = new_mock();
    let mut r = spawn_on(mock.clone(), dir.path());
    connect_with_apn(&mut r).await;
    // Profile 1 is in use and the operator's; make profile 2 the operator's too.
    let mut ro = router_core::drivers::mock::mock_profile(2, "Work", Some("work.example"), true);
    ro.read_only = true;
    mock.set_apn_profiles(
        1,
        vec![
            router_core::drivers::mock::mock_profile(1, "Operator internet", None, true),
            ro,
        ],
    );
    send(&r.h, Command::ApnRefresh).await;
    sleep(Duration::from_secs(1)).await;
    assert_eq!(
        apn_delete(&r.h, apn_target(&r, 1)).await,
        Err(validation_err(APN_PROFILE_IN_USE))
    );
    assert_eq!(
        apn_edit(&r.h, apn_target(&r, 2), apn_draft("X", typed())).await,
        Err(validation_err(APN_PROFILE_READ_ONLY))
    );
    assert_eq!(
        apn_delete(
            &r.h,
            ApnTarget {
                index: 2,
                fingerprint: "0000000000000000".into()
            }
        )
        .await,
        Err(validation_err(APN_PROFILE_READ_ONLY))
    );
    assert!(mock.apn_writes().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_hidden_password_cant_be_kept() {
    let dir = tempfile::tempdir().unwrap();
    let mock = new_mock();
    mock.set_apn_password(2, "saved-secret");
    mock.apn_masks_passwords(true);
    let mut r = spawn_on(mock.clone(), dir.path());
    connect_with_apn(&mut r).await;
    assert_eq!(
        apn_edit(
            &r.h,
            apn_target(&r, 2),
            apn_draft("Work 2", PasswordChange::Keep)
        )
        .await,
        Err(validation_err(APN_PASSWORD_HIDDEN))
    );
    assert!(mock.apn_writes().is_empty());
    // Typing it again works.
    apn_edit(&r.h, apn_target(&r, 2), apn_draft("Work 2", typed()))
        .await
        .unwrap();
    assert!(!events_log(dir.path()).contains("saved-secret"));
}

#[tokio::test(start_paused = true)]
async fn apn_writes_are_refused_while_anything_else_holds_the_router() {
    #[derive(Debug, Clone, Copy)]
    enum Busy {
        SpeedTest,
        Scan,
        BandChange,
        IpRefresh,
        ApnSwitch,
        RebootTicket,
    }
    let table = [
        (Busy::SpeedTest, SPEED_TEST_RUNNING),
        (Busy::Scan, CHANGE_IN_PROGRESS),
        (Busy::BandChange, CHANGE_IN_PROGRESS),
        (Busy::IpRefresh, IP_REFRESH_RUNNING),
        (Busy::ApnSwitch, APN_CHANGE_RUNNING),
        (Busy::RebootTicket, REBOOT_PENDING),
    ];
    for (busy, refusal) in table {
        let dir = tempfile::tempdir().unwrap();
        let mut r = spawn_on(new_mock(), dir.path());
        connect_with_apn(&mut r).await;
        match busy {
            Busy::SpeedTest => send(&r.h, Command::SetSpeedTestActive(true)).await,
            Busy::Scan => send(&r.h, Command::SetScanActive(true)).await,
            Busy::BandChange => {
                send(&r.h, Command::Apply(cfg(&[3]))).await;
                wait_phase(&r, Duration::from_secs(60), "probation", |p| {
                    *p == Phase::Probation
                })
                .await;
            }
            Busy::IpRefresh => start_ip_refresh(&r.h, None, 1).await.unwrap(),
            Busy::ApnSwitch => {
                apn_switch(&r.h, apn_target(&r, 2)).await.unwrap();
                wait_apn(&r, Duration::from_secs(60), "probation", |p| {
                    *p == Phase::Probation
                })
                .await;
            }
            Busy::RebootTicket => {
                request_reboot(&r.h).await.unwrap();
            }
        }
        let want = Err(validation_err(refusal));
        assert_eq!(
            apn_add(&r.h, apn_draft("New", typed())).await.map(|_| ()),
            want,
            "add while {busy:?}"
        );
        assert_eq!(
            apn_edit(&r.h, apn_target(&r, 3), apn_draft("New", typed())).await,
            want,
            "edit while {busy:?}"
        );
        assert_eq!(
            apn_delete(&r.h, apn_target(&r, 3)).await,
            want,
            "delete while {busy:?}"
        );
        assert_eq!(get(&r.probe.counts.apn_write), 0, "{busy:?}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_foreign_apn_journal_blocks_apn_writes() {
    let dir = tempfile::tempdir().unwrap();
    let mock = new_mock();
    write_apn_journal(dir.path(), "192.168.1.1", JournalPhase::Probation);
    let mut r = spawn_on(mock.clone(), dir.path());
    connect_with_apn(&mut r).await;
    assert_eq!(
        apn_add(&r.h, apn_draft("New", typed())).await,
        Err(validation_err(APN_JOURNAL_BLOCKED))
    );
    assert_eq!(get(&r.probe.counts.apn_write), 0);
}

#[tokio::test(start_paused = true)]
async fn an_unexpected_switch_after_a_write_is_switched_back_with_automatic_undo() {
    let dir = tempfile::tempdir().unwrap();
    let mock = new_mock();
    let mut r = spawn_on(mock.clone(), dir.path());
    connect_with_apn(&mut r).await;
    drain_all(&mut r.alerts);
    // The router makes the edited profile the one in use, as some firmware might.
    mock.apn_write_switches_to(Some(2));
    let r1 = apn_edit(&r.h, apn_target(&r, 2), apn_draft("Work 2", typed())).await;
    assert_eq!(
        r1,
        Err(validation_err(router_core::APN_ROUTER_SWITCHED)),
        "the driver says so"
    );
    let alerts = drain_all(&mut r.alerts);
    assert!(
        alerts
            .iter()
            .any(|a| matches!(a, Alert::ApnUnexpectedSwitch { reverting: true })),
        "{alerts:?}"
    );
    assert!(
        apn_journal_path(dir.path()).exists(),
        "a protected switch back"
    );
    wait_apn(&r, Duration::from_secs(150), "committed", |p| {
        *p == Phase::Committed
    })
    .await;
    assert_eq!(mock.current_apn(), 1, "back on the profile from before");
    assert_eq!(mock.apn_posts(), vec![1]);
    let log = events_log(dir.path());
    assert!(log.contains("\"apn.unexpected_switch\""), "{log}");
    assert!(!log.contains(TYPED_SECRET), "{log}");
}

#[tokio::test(start_paused = true)]
async fn an_unexpected_switch_under_auto_apn_is_only_reported() {
    let dir = tempfile::tempdir().unwrap();
    let mock = new_mock();
    mock.set_auto_apn(true);
    let mut r = spawn_on(mock.clone(), dir.path());
    connect_with_apn(&mut r).await;
    drain_all(&mut r.alerts);
    mock.apn_write_switches_to(Some(3));
    assert!(apn_delete(&r.h, apn_target(&r, 2)).await.is_err());
    let alerts = drain_all(&mut r.alerts);
    assert!(
        alerts
            .iter()
            .any(|a| matches!(a, Alert::ApnUnexpectedSwitch { reverting: false })),
        "{alerts:?}"
    );
    assert!(mock.apn_posts().is_empty());
    assert_eq!(mock.current_apn(), 3);
}

#[tokio::test(start_paused = true)]
async fn a_refused_plain_write_is_reported_as_needing_encryption() {
    let dir = tempfile::tempdir().unwrap();
    let mock = new_mock();
    mock.apn_refuses_writes(true);
    let mut r = spawn_on(mock.clone(), dir.path());
    connect_with_apn(&mut r).await;
    assert_eq!(
        apn_add(&r.h, apn_draft("New", typed())).await,
        Err(validation_err(router_core::APN_NEEDS_ENCRYPTION))
    );
}

/// The address read before the switch is no evidence for it: when every WAN read fails after
/// the POST, the switch is undone instead of kept, even with service.
#[tokio::test(start_paused = true)]
async fn an_apn_switch_without_a_fresh_wan_read_is_undone() {
    let dir = tempfile::tempdir().unwrap();
    let mock = new_mock();
    let mut r = spawn_on(mock.clone(), dir.path());
    connect_with_apn(&mut r).await;
    assert!(r
        .h
        .snapshots
        .borrow()
        .wan
        .as_ref()
        .is_some_and(|w| w.range.is_some()));
    mock.set_wan_error(Some(RouterError::Parse {
        message: "unreadable".to_string(),
    }));
    apn_switch(&r.h, apn_target(&r, 2)).await.unwrap();
    wait_apn(&r, Duration::from_secs(200), "reverted", |p| {
        matches!(p, Phase::Reverted { .. })
    })
    .await;
    assert_eq!(mock.apn_posts(), vec![2, 1]);
    assert_eq!(mock.current_apn(), 1);
    assert!(!events_log(dir.path()).contains("\"apn.committed\""));
}

/// A session lost while data is off logs back in at the fast rate, so data goes back on soon.
#[tokio::test(start_paused = true)]
async fn a_session_lost_while_data_is_off_relogs_quickly() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect_with_wan(&mut r).await;
    start_ip_refresh(&r.h, Some("100"), 1).await.unwrap();
    wait_data_off(&r).await;
    r.mock.set_fault(Some(Fault::SessionTakeover));
    sleep(Duration::from_secs(40)).await;
    assert!(!r.mock.data_on(), "still off while logins fail");
    r.mock.set_fault(None);
    sleep(Duration::from_secs(7)).await;
    assert!(r.mock.data_on(), "back on within the fast relogin");
}

#[tokio::test(start_paused = true)]
async fn turn_data_on_is_refused_while_something_holds_the_router() {
    #[derive(Debug, Clone, Copy)]
    enum Busy {
        BandChange,
        Scan,
        RebootTicket,
    }
    let table = [
        (Busy::BandChange, CHANGE_IN_PROGRESS),
        (Busy::Scan, CHANGE_IN_PROGRESS),
        (Busy::RebootTicket, REBOOT_PENDING),
    ];
    for (busy, refusal) in table {
        let dir = tempfile::tempdir().unwrap();
        let mock = new_mock();
        mock.set_data_on(false);
        let mut r = spawn_on(mock.clone(), dir.path());
        connect(&mut r).await;
        match busy {
            Busy::BandChange => {
                send(&r.h, Command::Apply(cfg(&[3]))).await;
                wait_phase(&r, Duration::from_secs(60), "grace", |p| {
                    !matches!(p, Phase::Idle)
                })
                .await;
            }
            Busy::Scan => send(&r.h, Command::SetScanActive(true)).await,
            Busy::RebootTicket => {
                request_reboot(&r.h).await.unwrap();
            }
        }
        assert_eq!(
            turn_data_on(&r.h).await,
            Err(validation_err(refusal)),
            "while {busy:?}"
        );
        assert!(mock.switch_posts().is_empty(), "{busy:?}");
    }
}

/// Mobile data off on the router (not by this app): an APN switch would fail its check, so
/// it isn't started.
#[tokio::test(start_paused = true)]
async fn an_apn_switch_is_refused_while_mobile_data_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let mock = new_mock();
    mock.set_data_on(false);
    let mut r = spawn_on(mock.clone(), dir.path());
    connect(&mut r).await;
    let mut rx = r.h.snapshots.clone();
    wait_for(&mut rx, Duration::from_secs(30), "notice", |s| {
        s.data_switch_off && s.apn.is_some()
    })
    .await;
    assert_eq!(
        apn_switch(&r.h, apn_target(&r, 2)).await,
        Err(validation_err(router_core::DATA_SWITCH_OFF))
    );
    assert!(mock.apn_posts().is_empty());
}

/// "No rights" on an optional read (the APN list here) turns that feature off for the run; it
/// is not a lost session, so the app doesn't log in again every few seconds.
#[tokio::test(start_paused = true)]
async fn a_refused_apn_read_is_not_a_lost_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    r.mock
        .set_apn_error(Some(RouterError::SessionLost { code: None }));
    connect(&mut r).await;
    sleep(Duration::from_secs(120)).await;
    assert_eq!(r.mock.login_attempts(), 1, "one login, no loop");
    let log = events_log(dir.path());
    assert!(!log.contains("\"session.lost\""), "{log}");
    assert_eq!(log.matches("\"feature.refused\"").count(), 1, "{log}");
    assert!(log.contains("\"what\":\"apn\""), "{log}");
}

/// The same for the internet address read.
#[tokio::test(start_paused = true)]
async fn a_refused_wan_read_is_not_a_lost_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    r.mock
        .set_wan_error(Some(RouterError::SessionLost { code: None }));
    connect(&mut r).await;
    sleep(Duration::from_secs(120)).await;
    assert_eq!(r.mock.login_attempts(), 1, "one login, no loop");
    let log = events_log(dir.path());
    assert!(!log.contains("\"session.lost\""), "{log}");
    assert!(log.contains("\"what\":\"wan\""), "{log}");
}

/// The event lines for `kind`, parsed.
fn events_of(dir: &Path, kind: &str) -> Vec<serde_json::Value> {
    events_log(dir)
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["kind"] == kind)
        .collect()
}

/// Keeping the router's code changes nothing about what counts as a lost session: 100003,
/// 125002 and 125003 all still end in a re-login, and `session.lost` names the code.
#[tokio::test(start_paused = true)]
async fn every_lost_session_code_still_ends_in_a_relogin() {
    for code in [100003u32, 125002, 125003] {
        let dir = tempfile::tempdir().unwrap();
        let mut r = spawn_on(new_mock(), dir.path());
        connect(&mut r).await;
        assert_eq!(r.mock.login_attempts(), 1);
        r.mock.drop_session_with_code(code);
        sleep(Duration::from_secs(60)).await;
        assert!(r.mock.login_attempts() >= 2, "code {code}: no re-login");
        assert!(r.mock.is_logged_in(), "code {code}");
        assert_eq!(r.h.snapshots.borrow().session, SessionView::LoggedIn);
        let lost = events_of(dir.path(), "session.lost");
        assert!(!lost.is_empty(), "code {code}");
        assert_eq!(lost[0]["detail"]["error"]["kind"], "sessionLost");
        assert_eq!(lost[0]["detail"]["error"]["code"], code, "{:?}", lost[0]);
    }
}

/// A refused optional read keeps the router's code in `feature.refused`.
#[tokio::test(start_paused = true)]
async fn a_refused_read_keeps_the_router_code() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    r.mock
        .set_apn_error(Some(RouterError::SessionLost { code: Some(100003) }));
    connect(&mut r).await;
    sleep(Duration::from_secs(30)).await;
    let refused = events_of(dir.path(), "feature.refused");
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert_eq!(refused[0]["detail"]["what"], "apn");
    assert_eq!(refused[0]["detail"]["error"]["code"], 100003);
}

/// A band change refused with "no rights" is still a lost session, and both `safe_apply.post`
/// and `session.lost` keep the router's code.
#[tokio::test(start_paused = true)]
async fn a_refused_band_change_keeps_the_router_code() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(new_mock(), dir.path());
    connect(&mut r).await;
    r.mock.set_fault(Some(Fault::ApplyError(100003)));
    send(&r.h, Command::Apply(cfg(&[3]))).await;
    sleep(Duration::from_secs(30)).await;
    let posts = events_of(dir.path(), "safe_apply.post");
    assert!(!posts.is_empty());
    assert_eq!(posts[0]["detail"]["error"]["kind"], "sessionLost");
    assert_eq!(
        posts[0]["detail"]["error"]["code"], 100003,
        "{:?}",
        posts[0]
    );
    let lost = events_of(dir.path(), "session.lost");
    assert!(!lost.is_empty());
    assert_eq!(lost[0]["detail"]["error"]["code"], 100003);
    assert!(r.mock.login_attempts() >= 2, "it logged in again");
    assert!(r.mock.applied().is_empty());
}

/// A session that dies right after each login backs off instead of logging in every 2 s.
#[tokio::test(start_paused = true)]
async fn a_session_lost_right_after_login_backs_off() {
    let dir = tempfile::tempdir().unwrap();
    let r = spawn_on(new_mock(), dir.path());
    r.mock.set_wan_error(Some(RouterError::SessionTakenOver));
    send(&r.h, Command::Connect(creds())).await;
    sleep(Duration::from_secs(90)).await;
    let n = r.mock.login_attempts();
    assert!(n <= 6, "{n} logins in 90 s");
}

// ---------------------------------------------------------------------------------------------
// Refresh IP by network search (the H122's method)

/// A mock that refreshes its IP by network search only, as the H122 does.
fn search_mock() -> Arc<MockDriver> {
    let mock = new_mock();
    mock.set_data_switch_supported(false);
    mock
}

fn count_kind(dir: &Path, kind: &str) -> usize {
    events_log(dir)
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|e| e["kind"] == kind)
        .count()
}

fn details(dir: &Path, kind: &str) -> Vec<serde_json::Value> {
    events_log(dir)
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|e| e["kind"] == kind)
        .map(|e| e["detail"].clone())
        .collect()
}

async fn wait_searching(r: &Rig) {
    let mut rx = r.h.snapshots.clone();
    wait_for(&mut rx, Duration::from_secs(10), "searching", |s| {
        s.ip_refresh
            .as_ref()
            .is_some_and(|v| v.phase == IpRefreshPhase::Searching)
    })
    .await;
}

/// The search runs in a task of its own: commands get their answer and snapshots keep coming
/// while the router searches, polls slow down, and the optional reads wait.
#[tokio::test(start_paused = true)]
async fn the_actor_keeps_answering_during_a_long_search() {
    let dir = tempfile::tempdir().unwrap();
    let mock = search_mock();
    mock.set_search_duration(Duration::from_secs(120));
    let mut r = spawn_on(mock, dir.path());
    connect_with_wan(&mut r).await;
    r.mock.queue_wan(&[mock_wan(1, 0x0102)]);

    let started = Instant::now();
    start_ip_refresh(&r.h, None, 1).await.unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "start doesn't wait"
    );
    wait_searching(&r).await;
    let view = r.h.snapshots.borrow().ip_refresh.clone().unwrap();
    assert_eq!(view.method, Some(router_core::IpMethod::NetworkSearch));

    r.probe.counts.reset();
    sleep(Duration::from_secs(30)).await;
    // A command is answered at once, mid-search.
    let asked = Instant::now();
    assert_eq!(
        refusal_text(request_reboot(&r.h).await).as_deref(),
        Some(IP_REFRESH_RUNNING)
    );
    assert!(asked.elapsed() < Duration::from_secs(1));
    // Snapshots keep coming.
    let mut rx = r.h.snapshots.clone();
    wait_for(
        &mut rx,
        Duration::from_secs(40),
        "a minute into the search",
        |s| {
            s.ip_refresh
                .as_ref()
                .is_some_and(|v| v.phase == IpRefreshPhase::Searching && v.seconds_in_phase >= 60)
        },
    )
    .await;
    let polls = get(&r.probe.counts.signal);
    assert!((8..=25).contains(&polls), "polls every 3 s: {polls}");
    assert_eq!(get(&r.probe.counts.wan), 0, "no WAN reads while searching");
    assert_eq!(get(&r.probe.counts.apn), 0, "no APN reads while searching");
    assert_eq!(r.h.snapshots.borrow().session, SessionView::LoggedIn);

    assert_eq!(
        ip_refresh_ends(&r, Duration::from_secs(300)).await,
        IpRefreshOutcome::Changed
    );
    assert_eq!(r.mock.searches(), 1);
    assert!(r.mock.switch_posts().is_empty());
    let search = details(dir.path(), "ip_refresh.search");
    assert_eq!(search.len(), 1);
    assert_eq!(search[0]["service_dropped"], true);
    assert_eq!(search[0]["networks"], 3);
    let ended = details(dir.path(), "ip_refresh.ended");
    assert_eq!(ended[0]["method"], "networkSearch");
    assert_eq!(ended[0]["bands_changed"], false);
    let last =
        r.h.snapshots
            .borrow()
            .ip_refresh
            .clone()
            .unwrap()
            .last
            .unwrap();
    assert_eq!(last.to.as_deref(), Some("10.62"));
    // Ranges only, never an address.
    assert!(!events_log(dir.path()).contains(&mock_wan(1, 0x0102).to_string()));
}

#[tokio::test(start_paused = true)]
async fn a_search_starts_with_a_session_refresh_when_the_last_is_old() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(search_mock(), dir.path());
    connect_with_wan(&mut r).await;
    assert_eq!(count_kind(dir.path(), "session.refreshed"), 0);
    sleep(Duration::from_secs(90)).await;
    start_ip_refresh(&r.h, None, 1).await.unwrap();
    assert_eq!(count_kind(dir.path(), "session.refreshed"), 1);
    assert_eq!(
        ip_refresh_ends(&r, Duration::from_secs(300)).await,
        IpRefreshOutcome::Unchanged
    );

    // A fresh session isn't refreshed again.
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(search_mock(), dir.path());
    connect_with_wan(&mut r).await;
    start_ip_refresh(&r.h, None, 1).await.unwrap();
    assert_eq!(count_kind(dir.path(), "session.refreshed"), 0);
}

/// The H122 answers some calls with a session-type code while the session is fine: a refused
/// search turns the search off, and the session stays.
#[tokio::test(start_paused = true)]
async fn a_refused_search_keeps_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let mock = search_mock();
    mock.refuse_search(Some(RouterError::SessionLost { code: Some(100003) }));
    let mut r = spawn_on(mock, dir.path());
    connect_with_wan(&mut r).await;
    let logins = get(&r.probe.counts.login);

    start_ip_refresh(&r.h, None, 1).await.unwrap();
    assert_eq!(
        ip_refresh_ends(&r, Duration::from_secs(30)).await,
        IpRefreshOutcome::SearchRefused
    );
    let last =
        r.h.snapshots
            .borrow()
            .ip_refresh
            .clone()
            .unwrap()
            .last
            .unwrap();
    assert_eq!(
        last.refusal,
        Some(RouterError::SessionLost { code: Some(100003) })
    );
    let refused = details(dir.path(), "feature.refused");
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert_eq!(refused[0]["what"], "network_search");
    assert_eq!(refused[0]["error"]["code"], 100003);
    assert_eq!(
        details(dir.path(), "ip_refresh.ended")[0]["refusal"]["code"],
        100003
    );

    sleep(Duration::from_secs(20)).await;
    assert_eq!(r.h.snapshots.borrow().session, SessionView::LoggedIn);
    assert_eq!(get(&r.probe.counts.login), logins, "no re-login");
    assert_eq!(count_kind(dir.path(), "session.lost"), 0);
    // Nothing is left to try this session.
    assert_eq!(
        start_ip_refresh(&r.h, None, 1).await,
        Err(validation_err(router_core::IP_NO_METHOD))
    );
}

#[tokio::test(start_paused = true)]
async fn a_band_change_during_a_search_is_logged_and_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let mock = search_mock();
    mock.search_sets_config(Some(cfg(&[1, 3])));
    let mut r = spawn_on(mock, dir.path());
    connect_with_wan(&mut r).await;
    start_ip_refresh(&r.h, None, 1).await.unwrap();
    ip_refresh_ends(&r, Duration::from_secs(300)).await;

    let last =
        r.h.snapshots
            .borrow()
            .ip_refresh
            .clone()
            .unwrap()
            .last
            .unwrap();
    assert!(last.bands_changed);
    assert_eq!(last.bands_before, Some(initial()));
    let changed = details(dir.path(), "ip_refresh.bands_changed");
    assert_eq!(changed.len(), 1);
    assert_eq!(
        changed[0]["after"],
        serde_json::to_value(cfg(&[1, 3])).unwrap()
    );
    assert_eq!(
        details(dir.path(), "ip_refresh.ended")[0]["bands_changed"],
        true
    );
    assert!(
        r.mock.applied().is_empty(),
        "nothing is changed back by itself"
    );
    let alerted = drain_all(&mut r.alerts).into_iter().any(|a| {
        matches!(a, Alert::IpRefresh { event: IpRefreshAlert::Ended { result } } if result.bands_changed)
    });
    assert!(alerted);
}

/// A refused "off" with a session-type code is no lost session: the switch stays on, the same
/// try goes on as a search, and the data switch isn't used again this session.
#[tokio::test(start_paused = true)]
async fn a_refused_off_falls_back_to_a_search_without_a_relogin() {
    let dir = tempfile::tempdir().unwrap();
    let mock = new_mock();
    mock.refuse_data_off(Some(RouterError::SessionLost { code: Some(125003) }));
    let mut r = spawn_on(mock, dir.path());
    connect_with_wan(&mut r).await;
    r.mock.queue_wan(&[mock_wan(2, 0x0304)]);
    let logins = get(&r.probe.counts.login);

    start_ip_refresh(&r.h, None, 1).await.unwrap();
    assert_eq!(
        ip_refresh_ends(&r, Duration::from_secs(300)).await,
        IpRefreshOutcome::Changed
    );
    assert!(r.mock.data_on());
    assert!(r.mock.switch_posts().is_empty(), "no off ever went through");
    assert_eq!(r.mock.searches(), 1);
    assert!(!data_journal_path(dir.path()).exists());
    let fallback = details(dir.path(), "ip_refresh.method_fallback");
    assert_eq!(fallback.len(), 1);
    assert_eq!(fallback[0]["error"]["code"], 125003);
    let refused = details(dir.path(), "feature.refused");
    assert_eq!(refused[0]["what"], "data_switch_write");
    assert_eq!(get(&r.probe.counts.login), logins, "no re-login");
    assert_eq!(count_kind(dir.path(), "session.lost"), 0);
    assert_eq!(r.h.snapshots.borrow().session, SessionView::LoggedIn);

    // The next run goes straight to the search.
    sleep(Duration::from_secs(5)).await;
    start_ip_refresh(&r.h, None, 1).await.unwrap();
    let view = r.h.snapshots.borrow().ip_refresh.clone().unwrap();
    assert_eq!(view.method, Some(router_core::IpMethod::NetworkSearch));
    ip_refresh_ends(&r, Duration::from_secs(300)).await;
    assert_eq!(count_kind(dir.path(), "ip_refresh.method_fallback"), 1);
}

/// A search that never answers ends at the actor's call limit, then the online wait; a router
/// that stays without service ends the run with the online timeout.
#[tokio::test(start_paused = true)]
async fn a_search_that_never_answers_ends_in_the_online_wait() {
    let dir = tempfile::tempdir().unwrap();
    let mock = search_mock();
    mock.search_never_answers(true);
    let mut r = spawn_on(mock, dir.path());
    connect_with_wan(&mut r).await;
    start_ip_refresh(&r.h, None, 1).await.unwrap();
    assert_eq!(
        ip_refresh_ends(&r, Duration::from_secs(600)).await,
        IpRefreshOutcome::OnlineTimeout
    );
    let search = details(dir.path(), "ip_refresh.search");
    assert_eq!(search[0]["timed_out"], true);
    assert_eq!(r.h.snapshots.borrow().session, SessionView::LoggedIn);
}

#[tokio::test(start_paused = true)]
async fn service_that_never_comes_back_ends_with_the_online_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let mock = search_mock();
    mock.search_breaks_service(true);
    let mut r = spawn_on(mock, dir.path());
    connect_with_wan(&mut r).await;
    start_ip_refresh(&r.h, None, 1).await.unwrap();
    assert_eq!(
        ip_refresh_ends(&r, Duration::from_secs(600)).await,
        IpRefreshOutcome::OnlineTimeout
    );
    // Nothing else is tried by itself: no reboot, no band change.
    assert_eq!(get(&r.probe.counts.reboot), 0);
    assert!(r.mock.applied().is_empty());
}

// ---------------------------------------------------------------------------------------------
// New IP by a band switch

async fn start_ip_reattach(h: &ActorHandle) -> Result<(), RouterError> {
    let (tx, rx) = oneshot::channel();
    send(h, Command::StartIpReattach(tx)).await;
    rx.await.unwrap()
}

/// A mock on `config` where B3 serves when it is allowed (B1's signal is the weakest).
fn mock_b3_serving(config: NetConfig) -> Arc<MockDriver> {
    let mut scenario = MockScenario {
        initial: config,
        ..MockScenario::zain_like()
    };
    for c in &mut scenario.cells {
        if c.band == 1 {
            c.sinr = -5.0;
        }
    }
    Arc::new(MockDriver::new(scenario))
}

/// Waits for the end of a band switch for a new IP and returns its result.
async fn reattach_ends(r: &Rig, within: Duration) -> IpReattachResult {
    let mut rx = r.h.snapshots.clone();
    let s = wait_for(&mut rx, within, "ip reattach end", |s| {
        s.ip_reattach
            .as_ref()
            .is_some_and(|v| !v.active && v.last.is_some())
    })
    .await;
    s.ip_reattach.unwrap().last.unwrap()
}

/// B1+B3 with B3 serving: B3 alone for a moment, kept as soon as it has an address, then B1+B3
/// again with the full probation; the range is new.
#[tokio::test(start_paused = true)]
async fn reattach_switches_to_the_serving_band_and_back_for_a_new_range() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(mock_b3_serving(cfg(&[1, 3])), dir.path());
    connect_with_wan(&mut r).await;
    r.mock
        .queue_reattach_wan(&[mock_wan(1, 0x0101), mock_wan(2, 0x0202)]);
    let t0 = Instant::now();
    start_ip_reattach(&r.h).await.unwrap();

    let mut rx = r.h.snapshots.clone();
    let s = wait_for(&mut rx, Duration::from_secs(5), "step 1", |s| {
        s.safe_apply.sequence.as_ref().is_some_and(|q| q.step == 1)
    })
    .await;
    let q = s.safe_apply.sequence.unwrap();
    assert_eq!(q.origin, SequenceOrigin::IpReattach);
    assert_eq!(q.step1, Some(cfg(&[3])));
    assert_eq!((q.primary, q.final_config.clone()), (3, cfg(&[1, 3])));
    let view = s.ip_reattach.unwrap();
    assert!(view.active);
    assert_eq!(view.from_range.as_deref(), Some("10.45"));
    // Anything else is refused while it runs.
    assert_eq!(
        start_ip_reattach(&r.h).await,
        Err(validation_err(CHANGE_IN_PROGRESS))
    );
    assert_eq!(
        start_ip_refresh(&r.h, None, 1).await,
        Err(validation_err(CHANGE_IN_PROGRESS))
    );

    wait_for(&mut rx, Duration::from_secs(120), "step 2", |s| {
        s.safe_apply.sequence.as_ref().is_some_and(|q| q.step == 2)
    })
    .await;
    let step2_at = Instant::now();
    assert!(
        step2_at - t0 < Duration::from_secs(60),
        "step 1 kept early, after {:?}",
        step2_at - t0
    );

    let result = reattach_ends(&r, Duration::from_secs(600)).await;
    assert!(
        Instant::now() - step2_at >= Duration::from_secs(180),
        "step 2 has the full probation"
    );
    assert_eq!(result.outcome, IpReattachOutcome::Changed);
    assert_eq!(result.from.as_deref(), Some("10.45"));
    assert_eq!(result.to.as_deref(), Some("100.70"));
    assert_eq!(result.restore, None);
    assert_eq!(r.mock.applied(), vec![cfg(&[3]), cfg(&[1, 3])]);
    assert!(!dir.path().join(SEQ_FILE).exists());

    let alerts = drain_all(&mut r.alerts);
    assert!(
        automation_events(&alerts).is_empty(),
        "no relock or step alerts"
    );
    assert!(alerts
        .iter()
        .any(|a| matches!(a, Alert::IpReattachEnded { result }
        if result.outcome == IpReattachOutcome::Changed)));
    let log = event_log(dir.path());
    for kind in [
        "ip_reattach.started",
        "ip_reattach.step1_kept",
        "ip_reattach.ended",
    ] {
        assert!(log.contains(&format!("\"{kind}\"")), "{kind} missing");
    }
    for ip in [
        mock_wan(0, 0x25_8E),
        mock_wan(1, 0x0101),
        mock_wan(2, 0x0202),
    ] {
        assert!(
            !log.contains(&ip.to_string()),
            "full address {ip} in events"
        );
    }
}

/// One band allowed: step 1 widens to every supported band, then the one band comes back.
#[tokio::test(start_paused = true)]
async fn reattach_on_one_band_widens_and_comes_back() {
    let dir = tempfile::tempdir().unwrap();
    let mock = mock_on(cfg(&[3]));
    let supported = MockScenario::zain_like().supported;
    let mut r = spawn_on(mock.clone(), dir.path());
    connect_with_wan(&mut r).await;
    start_ip_reattach(&r.h).await.unwrap();
    let result = reattach_ends(&r, Duration::from_secs(900)).await;
    let wide = NetConfig {
        lte_bands: supported,
        ..cfg(&[3])
    };
    assert_eq!(mock.applied(), vec![wide, cfg(&[3])]);
    // The mock keeps its address here.
    assert_eq!(result.outcome, IpReattachOutcome::Unchanged);
    assert_eq!(result.to.as_deref(), Some("10.45"));
    assert_eq!(result.restore, None);
}

/// Step 1 without service is undone by safe apply and reported as such, not as a failed
/// primary-band lock.
#[tokio::test(start_paused = true)]
async fn reattach_step_one_without_service_is_undone() {
    let dir = tempfile::tempdir().unwrap();
    let mock = mock_b3_serving(cfg(&[1, 3]));
    let mut r = spawn_on(mock.clone(), dir.path());
    connect_with_wan(&mut r).await;
    start_ip_reattach(&r.h).await.unwrap();
    assert_eq!(mock.applied(), vec![cfg(&[3])]);
    mock.set_fault(Some(Fault::NoServiceForever));
    let mut rx = r.h.snapshots.clone();
    let m = mock.clone();
    wait_for(
        &mut rx,
        Duration::from_secs(200),
        "revert POST",
        move |_| m.applied().len() >= 2,
    )
    .await;
    mock.set_fault(None);
    let result = reattach_ends(&r, Duration::from_secs(300)).await;
    assert_eq!(result.outcome, IpReattachOutcome::Step1Failed);
    assert_eq!(result.restore, None, "back on the bands from before");
    assert_eq!(mock.applied(), vec![cfg(&[3]), cfg(&[1, 3])]);
    sleep(Duration::from_secs(30)).await;
    assert_eq!(mock.applied().len(), 2, "no step 2");
    let events = automation_events(&drain_all(&mut r.alerts));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AutomationEvent::PrimaryLockFailed { .. })),
        "{events:?}"
    );
    assert!(event_log(dir.path()).contains("\"step1Failed\""));
}

/// Refused while a scan or an IP refresh holds the router, and after three in an hour.
#[tokio::test(start_paused = true)]
async fn reattach_refusals_and_hourly_limit() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(mock_b3_serving(cfg(&[1, 3])), dir.path());
    connect_with_wan(&mut r).await;

    send(&r.h, Command::SetScanActive(true)).await;
    assert_eq!(
        start_ip_reattach(&r.h).await,
        Err(validation_err(CHANGE_IN_PROGRESS))
    );
    send(&r.h, Command::SetScanActive(false)).await;

    start_ip_refresh(&r.h, None, 1).await.unwrap();
    assert_eq!(
        start_ip_reattach(&r.h).await,
        Err(validation_err(IP_REFRESH_RUNNING))
    );
    ip_refresh_ends(&r, Duration::from_secs(300)).await;
    let mut rx = r.h.snapshots.clone();
    wait_for(&mut rx, Duration::from_secs(60), "online again", |s| {
        s.in_service && s.wan.as_ref().is_some_and(|w| w.range.is_some())
    })
    .await;

    for n in 0..3 {
        start_ip_reattach(&r.h).await.unwrap();
        let result = reattach_ends(&r, Duration::from_secs(900)).await;
        assert_eq!(result.restore, None, "run {n}");
    }
    assert_eq!(r.mock.applied().len(), 6);
    assert_eq!(
        start_ip_reattach(&r.h).await,
        Err(validation_err(IP_REATTACH_COOLDOWN))
    );
    sleep(Duration::from_secs(3600)).await;
    start_ip_reattach(&r.h).await.unwrap();
}

async fn begin_page_scripts(h: &ActorHandle) -> Result<(), RouterError> {
    let (tx, rx) = oneshot::channel();
    send(h, Command::BeginPageScripts(tx)).await;
    rx.await.unwrap()
}

/// While the page scripts are read, the router isn't handed to anything else: Refresh IP, the
/// band switch for a new IP and a second read are refused until it ends.
#[tokio::test(start_paused = true)]
async fn page_scripts_hold_the_router_until_they_end() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(mock_b3_serving(cfg(&[1, 3])), dir.path());
    connect_with_wan(&mut r).await;

    begin_page_scripts(&r.h).await.unwrap();
    assert_eq!(
        begin_page_scripts(&r.h).await,
        Err(validation_err(PAGE_SCRIPTS_RUNNING))
    );
    assert_eq!(
        start_ip_refresh(&r.h, None, 1).await,
        Err(validation_err(PAGE_SCRIPTS_RUNNING))
    );
    assert_eq!(
        start_ip_reattach(&r.h).await,
        Err(validation_err(PAGE_SCRIPTS_RUNNING))
    );
    assert!(r.mock.applied().is_empty(), "nothing sent");

    send(&r.h, Command::EndPageScripts).await;
    start_ip_refresh(&r.h, None, 1).await.unwrap();
    // And a read can't start while Refresh IP runs.
    assert_eq!(
        begin_page_scripts(&r.h).await,
        Err(validation_err(IP_REFRESH_RUNNING))
    );
}

/// With automatic undo off a step without service would be kept, so the switch is refused and
/// nothing is sent; turning it back on allows it again.
#[tokio::test(start_paused = true)]
async fn reattach_refused_while_automatic_undo_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = spawn_on(mock_b3_serving(cfg(&[1, 3])), dir.path());
    connect_with_wan(&mut r).await;
    send(
        &r.h,
        Command::SetAutomation(AutomationConfig {
            revert_on_loss: false,
            ..AutomationConfig::default()
        }),
    )
    .await;
    assert_eq!(
        start_ip_reattach(&r.h).await,
        Err(validation_err(IP_REATTACH_UNDO_OFF))
    );
    sleep(Duration::from_secs(30)).await;
    assert!(r.mock.applied().is_empty(), "nothing sent");
    assert!(r.h.snapshots.borrow().safe_apply.sequence.is_none());

    send(&r.h, Command::SetAutomation(AutomationConfig::default())).await;
    start_ip_reattach(&r.h).await.unwrap();
}

/// A restart during step 1 brings the sequence back with its own step 1 (here every supported
/// band, which isn't a primary band alone), and step 2 follows.
#[tokio::test(start_paused = true)]
async fn reattach_restart_in_step_one_resumes_with_its_step_one() {
    let dir = tempfile::tempdir().unwrap();
    let mock = mock_on(cfg(&[3]));
    let wide = NetConfig {
        lte_bands: MockScenario::zain_like().supported,
        ..cfg(&[3])
    };
    let mut r = spawn_on(mock.clone(), dir.path());
    connect_with_wan(&mut r).await;
    start_ip_reattach(&r.h).await.unwrap();
    wait_phase(&r, Duration::from_secs(30), "step 1 grace", |p| {
        *p == Phase::ReattachGrace
    })
    .await;
    let rec = SequenceStore::new(dir.path().to_path_buf())
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(rec.origin, SequenceOrigin::IpReattach);
    assert_eq!(rec.step1, Some(wide.clone()));
    assert!(r.h.clone().shutdown(Duration::from_secs(5)).await);
    drop(r);

    let mut r = spawn_on(mock.clone(), dir.path());
    let mut rx = r.h.snapshots.clone();
    let s = wait_for(&mut rx, Duration::from_secs(2), "restored step 1", |s| {
        s.safe_apply.sequence.is_some()
    })
    .await;
    let q = s.safe_apply.sequence.unwrap();
    assert_eq!((q.step, q.step1), (1, Some(wide.clone())));
    connect(&mut r).await;
    let s = wait_settled(&r, Duration::from_secs(900), 2).await;
    assert_eq!(s.safe_apply.phase, Phase::Committed);
    assert_eq!(mock.applied(), vec![wide, cfg(&[3])]);
    assert!(automation_events(&drain_all(&mut r.alerts)).contains(
        &AutomationEvent::SequenceResumed {
            primary: 1,
            origin: SequenceOrigin::IpReattach,
            step: 1
        }
    ));
    let result = reattach_ends(&r, Duration::from_secs(120)).await;
    // The range from before the restart isn't kept.
    assert_eq!(result.outcome, IpReattachOutcome::Unknown);
    assert_eq!(result.restore, None);
    assert_eq!(ignored_files(dir.path()), 0);
}
