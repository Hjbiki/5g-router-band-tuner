//! Auto-scan: tries band sets one by one through the actor's own safe apply, measures each, and
//! ends back on the bands it started from. It shares the wait, apply and keep logic
//! (`ChangeCtl`) with the headless `band-runner`.
//!
//! Every config goes through a real safe apply (journal, verify, settle, auto-revert), so a band
//! with no service reverts by itself. `scan.json` is written after every step: if the PC or the
//! app dies mid-scan, the journal finishes or reverts the open change at the next launch, the host
//! then returns to the starting bands, and the results so far stay readable. An internet drop
//! only fails that config's speed test; the scan moves on.
//!
//! Only the band set changes between configs; network mode and 5G option stay as they were.
//!
//! A scan waits less for a config than a manual change does, because most bands it tries have no
//! cell at all: see `GIVE_UP`. A refused request says nothing about the band, so it's tried again.

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, watch};
use tokio::time::{sleep, Instant};
use ts_rs::TS;

use crate::actor::{Alert, AppSnapshot, Command, SessionView};
use crate::bands::lte::lte_band_meta;
use crate::bands::Duplex;
use crate::safe_apply::{RevertReason, SafeApplyEvent};
use crate::{LteBandSet, NetConfig, Phase, Signal};

pub const SCAN_FILE: &str = "scan.json";
const FILE_VERSION: u32 = 1;

/// How long a new config settles in probation before it is read and kept: 5G attaches late.
const SETTLE: Duration = Duration::from_secs(30);
/// A config that is already active needs no reconnect, only a short look at the signal.
const SETTLE_ACTIVE: Duration = Duration::from_secs(5);
const SETTLE_RETURN: Duration = Duration::from_secs(15);
const SAMPLE_EVERY: Duration = Duration::from_secs(1);
const KEEP_WITHIN: Duration = Duration::from_secs(60);
const ONLINE_WITHIN: Duration = Duration::from_secs(5 * 60);
/// Verify and settle on top of the user's re-attach grace, before the scan reverts by itself.
const APPLY_SLACK: Duration = Duration::from_secs(120);
/// On the H122-373, bands with a cell were in service about 15 s after the request, and settled
/// by about 25 s (real scan, 2026-10-31).
const GIVE_UP: GiveUpAfter = GiveUpAfter {
    reattach: Duration::from_secs(60),
    no_service: Duration::from_secs(30),
};
/// Most speed tests a scan runs on one config.
pub const MAX_REPEATS: u32 = 3;
/// A real run stopped mid-upload on a config that works, so a test that fails gets one more go.
const TEST_RETRY_AFTER: Duration = Duration::from_secs(5);
/// Times one config is tried again after the router refused the request.
const REFUSED_RETRIES: u32 = 1;
/// After this many configs in a row the router refused, the scan stops and goes back.
const MAX_REFUSED_IN_A_ROW: u32 = 3;
const RETURN_RETRY: Duration = Duration::from_secs(15);
/// Thorough mode combines at most this many bands that worked: 6 pairs and 4 triples.
const MAX_COMBINED_BANDS: usize = 4;
/// Most combinations the user can ask for in one scan. The UI's `MAX_CUSTOM_COMBOS` matches it.
pub const MAX_CUSTOM_SETS: usize = 12;

/// Not idle, committed or reverted.
pub fn active(p: &Phase) -> bool {
    !matches!(p, Phase::Idle | Phase::Committed | Phase::Reverted { .. })
}

// ---------------------------------------------------------------------------------------------
// Signal statistics

/// One poll, reduced to what a report needs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Sample {
    pub lte_band: Option<u16>,
    pub lte_pci: Option<u16>,
    pub lte_sinr: Option<f32>,
    pub lte_rsrp: Option<f32>,
    pub nr_present: bool,
    pub nr_band: Option<u16>,
    pub nr_sinr: Option<f32>,
    pub nr_rsrp: Option<f32>,
    pub in_service: bool,
}

impl Sample {
    pub fn from_signal(sig: &Signal, in_service: bool) -> Self {
        let lte = sig.lte.as_ref();
        let nr = sig.nr.as_ref();
        Self {
            lte_band: lte.and_then(|l| l.band),
            lte_pci: lte.and_then(|l| l.pci),
            lte_sinr: lte.and_then(|l| l.sinr),
            lte_rsrp: lte.and_then(|l| l.rsrp),
            nr_present: nr.is_some(),
            nr_band: nr.and_then(|n| n.band.as_ref()).map(|b| b.primary),
            nr_sinr: nr.and_then(|n| n.sinr),
            nr_rsrp: nr.and_then(|n| n.rsrp),
            in_service,
        }
    }
}

/// Mean, min and max of the values that were present.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Stat {
    pub avg: Option<f32>,
    pub min: Option<f32>,
    pub max: Option<f32>,
    pub n: usize,
}

impl Stat {
    pub fn of(values: impl Iterator<Item = Option<f32>>) -> Self {
        let v: Vec<f32> = values.flatten().filter(|x| x.is_finite()).collect();
        if v.is_empty() {
            return Self::default();
        }
        let sum: f32 = v.iter().sum();
        Self {
            avg: Some(sum / v.len() as f32),
            min: v.iter().copied().reduce(f32::min),
            max: v.iter().copied().reduce(f32::max),
            n: v.len(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Driving one safe-apply change

#[derive(Debug)]
pub enum Applied {
    /// Re-attached and in probation after this many seconds.
    Probation(f32),
    /// The router already has this config.
    AlreadyActive,
    Rejected(String),
    Reverted {
        reason: RevertReason,
        fallback: bool,
    },
    /// No outcome in time: the change was reverted; this is how it ended.
    TimedOut(Phase),
    Stopped,
}

/// When to stop waiting for a change that was applied, counted from when it started.
#[derive(Debug, Clone, Copy)]
pub struct GiveUpAfter {
    pub reattach: Duration,
    /// The router has reported no service for this long without a break.
    pub no_service: Duration,
}

/// Commands, snapshots and alerts of one actor, with the waits every band-change driver needs.
pub struct ChangeCtl {
    pub commands: mpsc::Sender<Command>,
    pub snapshots: watch::Receiver<AppSnapshot>,
    pub alerts: broadcast::Receiver<Alert>,
}

impl ChangeCtl {
    pub fn snap(&self) -> AppSnapshot {
        self.snapshots.borrow().clone()
    }

    pub async fn send(&self, c: Command) {
        let _ = self.commands.send(c).await;
    }

    /// Waits until `pred` holds or `within` passes; `None` on timeout.
    pub async fn wait_for(
        &mut self,
        within: Duration,
        pred: impl Fn(&AppSnapshot) -> bool,
    ) -> Option<AppSnapshot> {
        let deadline = Instant::now() + within;
        loop {
            {
                let s = self.snapshots.borrow_and_update();
                if pred(&s) {
                    return Some(s.clone());
                }
            }
            if Instant::now() >= deadline {
                return None;
            }
            let _ = tokio::time::timeout(Duration::from_secs(1), self.snapshots.changed()).await;
        }
    }

    /// `wait_for`, also ending when `stop` says so. `None` then, as on a timeout.
    pub async fn wait_or_stop(
        &mut self,
        within: Duration,
        stop: &mut (dyn FnMut() -> bool + Send),
        pred: impl Fn(&AppSnapshot) -> bool,
    ) -> Option<AppSnapshot> {
        let deadline = Instant::now() + within;
        loop {
            let step = Duration::from_secs(1).min(deadline - Instant::now());
            if let Some(s) = self.wait_for(step, &pred).await {
                return Some(s);
            }
            if stop() || Instant::now() >= deadline {
                return None;
            }
        }
    }

    /// Never returns while a change is active: a revert that keeps failing is retried by the
    /// actor every 30 s, and this waits with it. `waiting` hears about each long wait.
    pub async fn until_no_change(&mut self, waiting: &mut (dyn FnMut(&Phase) + Send)) -> Phase {
        loop {
            if let Some(s) = self
                .wait_for(Duration::from_secs(60), |s| !active(&s.safe_apply.phase))
                .await
            {
                return s.safe_apply.phase;
            }
            waiting(&self.snap().safe_apply.phase);
        }
    }

    /// Reverts the active change and waits for the end. RevertNow only counts in grace,
    /// probation or after a failed revert, so it is sent again whenever the change is there.
    pub async fn revert_and_wait(&mut self) -> Phase {
        self.end_and_wait(|| Command::RevertNow).await
    }

    /// The same, for a band that gave no service: the revert's reason says so.
    pub async fn give_up_and_wait(&mut self) -> Phase {
        self.end_and_wait(|| Command::GiveUp).await
    }

    async fn end_and_wait(&mut self, command: fn() -> Command) -> Phase {
        let mut last_sent: Option<Instant> = None;
        loop {
            let phase = self.snap().safe_apply.phase;
            if !active(&phase) {
                return phase;
            }
            let revertable = matches!(
                phase,
                Phase::ReattachGrace | Phase::Probation | Phase::RevertFailed
            );
            if revertable && last_sent.is_none_or(|t| t.elapsed() >= Duration::from_secs(5)) {
                self.send(command()).await;
                last_sent = Some(Instant::now());
            }
            self.wait_for(Duration::from_secs(1), |s| !active(&s.safe_apply.phase))
                .await;
        }
    }

    fn drain_alerts(&mut self) -> Vec<Alert> {
        let mut out = Vec::new();
        loop {
            match self.alerts.try_recv() {
                Ok(a) => out.push(a),
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
        out
    }

    /// Applies `cfg` and waits for probation (re-attached and settled), or for the outcome.
    /// `stop` is checked before sending, never after: a queued Apply always runs.
    pub async fn apply(
        &mut self,
        cfg: &NetConfig,
        within: Duration,
        stop: &mut (dyn FnMut() -> bool + Send),
    ) -> Applied {
        self.apply_limited(cfg, within, None, stop).await
    }

    /// `apply`, ending a change early by `limits`. A change given up on ends as `Reverted`.
    pub async fn apply_limited(
        &mut self,
        cfg: &NetConfig,
        within: Duration,
        limits: Option<GiveUpAfter>,
        stop: &mut (dyn FnMut() -> bool + Send),
    ) -> Applied {
        if stop() {
            return Applied::Stopped;
        }
        self.drain_alerts();
        let started = Instant::now();
        self.send(Command::Apply(cfg.clone())).await;
        let mut seen_active = false;
        let mut active_since = None;
        let mut no_service_since = None;
        let deadline = started + within;
        loop {
            for a in self.drain_alerts() {
                match a {
                    Alert::SafeApply {
                        event: SafeApplyEvent::Rejected { message },
                    } => {
                        return if message.contains("already") {
                            Applied::AlreadyActive
                        } else {
                            Applied::Rejected(message)
                        };
                    }
                    // A request the router refused closes the change at once, so the snapshots
                    // may never show it as active.
                    Alert::SafeApply {
                        event: SafeApplyEvent::Reverted { reason, fallback },
                    } => return Applied::Reverted { reason, fallback },
                    _ => {}
                }
            }
            let snap = self.snap();
            let phase = snap.safe_apply.phase.clone();
            seen_active |= active(&phase);
            match &phase {
                Phase::Probation => {
                    return Applied::Probation(started.elapsed().as_secs_f32());
                }
                Phase::Reverted { reason, fallback } if seen_active => {
                    return Applied::Reverted {
                        reason: *reason,
                        fallback: *fallback,
                    };
                }
                _ => {}
            }
            // Before the change starts there's nothing to revert yet: keep waiting for it.
            if seen_active && stop() {
                self.revert_and_wait().await;
                return Applied::Stopped;
            }
            if let (Some(limits), true) = (limits, seen_active) {
                let now = Instant::now();
                let since = *active_since.get_or_insert(now);
                let says_no_service = phase == Phase::ReattachGrace
                    && snap.session == SessionView::LoggedIn
                    && !snap.in_service;
                // A poll that fails says nothing either way, so it doesn't break the run.
                if snap.in_service {
                    no_service_since = None;
                } else if says_no_service {
                    no_service_since.get_or_insert(now);
                }
                let no_cell = says_no_service
                    && no_service_since.is_some_and(|t| now - t >= limits.no_service);
                if no_cell || now - since >= limits.reattach {
                    return match self.give_up_and_wait().await {
                        Phase::Reverted { reason, fallback } => {
                            Applied::Reverted { reason, fallback }
                        }
                        other => Applied::TimedOut(other),
                    };
                }
            }
            if Instant::now() >= deadline {
                let p = self.revert_and_wait().await;
                return Applied::TimedOut(p);
            }
            sleep(Duration::from_millis(250)).await;
        }
    }

    /// Ends a probation by keeping the change; `true` once it is committed.
    pub async fn keep(&mut self) -> bool {
        if self.snap().safe_apply.phase != Phase::Probation {
            return false;
        }
        self.send(Command::KeepNow).await;
        match self
            .wait_for(KEEP_WITHIN, |s| !active(&s.safe_apply.phase))
            .await
        {
            Some(s) => s.safe_apply.phase == Phase::Committed,
            None => matches!(self.until_no_change(&mut |_| {}).await, Phase::Committed),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The scan's record

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ScanMode {
    /// Single bands and the current set.
    Quick,
    /// Quick, then 2- and 3-band combinations of the bands that worked.
    Thorough,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ScanStatus {
    Running,
    /// Going back to the starting bands.
    Returning,
    /// Every config was tried.
    Done,
    /// The user pressed Stop.
    Stopped,
    /// The app or PC went away mid-scan. The remaining configs can still be run.
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ScanStep {
    Applying,
    Settling,
    Testing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ScanOutcome {
    /// It stayed in service and the speed test finished.
    Measured,
    /// It stayed in service, but the speed test failed.
    TestFailed,
    /// No service: safe apply put the previous bands back.
    NoService,
    /// The router refused it, or it ended some other way (`note` says how).
    Failed,
}

/// What the speed test measured.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScanSpeed {
    pub download_mbps: Option<f64>,
    pub upload_mbps: Option<f64>,
    pub idle_ping_ms: Option<f64>,
    pub ping_down_ms: Option<f64>,
    pub ping_up_ms: Option<f64>,
    pub bytes_used: u64,
}

/// One config's result. Band and PCI only: no cell or eNB identity is recorded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ScanResult {
    #[ts(type = "number")]
    pub at_ms: u64,
    pub bands: LteBandSet,
    /// These are the bands the scan started from.
    pub is_start: bool,
    pub outcome: ScanOutcome,
    pub note: Option<String>,
    pub lte_band: Option<u16>,
    pub lte_pci: Option<u16>,
    pub nr_band: Option<u16>,
    pub got_5g: bool,
    pub lte_rsrp: Option<f32>,
    pub lte_sinr: Option<f32>,
    pub nr_sinr: Option<f32>,
    pub download_mbps: Option<f64>,
    pub upload_mbps: Option<f64>,
    pub idle_ping_ms: Option<f64>,
    pub ping_down_ms: Option<f64>,
    pub ping_up_ms: Option<f64>,
    #[ts(type = "number")]
    pub bytes_used: u64,
    /// Speed tests behind the speeds, which are their average.
    #[serde(default = "one")]
    pub tests: u32,
    #[serde(default)]
    pub download_min_mbps: Option<f64>,
    #[serde(default)]
    pub download_max_mbps: Option<f64>,
    #[serde(default)]
    pub upload_min_mbps: Option<f64>,
    #[serde(default)]
    pub upload_max_mbps: Option<f64>,
    /// What the router said about 5G once this config was measured: see `Status`.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub endc: Option<bool>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub endc_restricted: Option<bool>,
}

fn one() -> u32 {
    1
}

/// `scan.json`: everything needed to show the results and to resume.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ScanFile {
    pub version: u32,
    pub host: String,
    pub driver: String,
    pub mode: ScanMode,
    pub status: ScanStatus,
    pub step: ScanStep,
    /// The config to return to.
    pub start: NetConfig,
    pub queue: Vec<LteBandSet>,
    /// Index into `queue` of the config being tried.
    pub next: u32,
    pub combos_added: bool,
    pub results: Vec<ScanResult>,
    #[ts(type = "number")]
    pub started_ms: u64,
    /// Set when the scan couldn't get back to the starting bands.
    pub return_failed: bool,
    /// Speed tests per config.
    #[serde(default = "one")]
    pub repeats: u32,
    /// Which of them is running.
    #[serde(default = "one")]
    pub test: u32,
    /// The router's model: another router can sit at the same address.
    #[serde(default)]
    pub model: Option<String>,
    /// Downlink-only bands that were chosen. They can't carry a connection, so they're never tried
    /// alone: Thorough adds them to the bands that worked.
    #[serde(default)]
    pub add_on_bands: Vec<u8>,
    /// Combinations the user asked for. Tried after the single bands in both modes; Thorough's own
    /// combinations skip them.
    #[serde(default)]
    pub custom_sets: Vec<LteBandSet>,
}

impl ScanFile {
    pub fn new(
        host: &str,
        driver: &str,
        mode: ScanMode,
        start: NetConfig,
        bands: &[u8],
        started_ms: u64,
    ) -> Self {
        let queue = quick_queue(start.lte_bands, bands);
        Self {
            version: FILE_VERSION,
            host: host.to_string(),
            driver: driver.to_string(),
            mode,
            status: ScanStatus::Running,
            step: ScanStep::Applying,
            start,
            queue,
            next: 0,
            combos_added: false,
            results: Vec::new(),
            started_ms,
            return_failed: false,
            repeats: 1,
            test: 1,
            model: None,
            add_on_bands: bands
                .iter()
                .copied()
                .filter(|b| downlink_only(*b))
                .collect(),
            custom_sets: Vec::new(),
        }
    }

    /// Queues the combinations the user asked for, after the single bands. A set that's already
    /// queued (the start set, a single band, a repeat) is skipped, and at most `MAX_CUSTOM_SETS`
    /// are kept.
    pub fn add_custom(&mut self, sets: &[LteBandSet]) {
        for set in sets {
            if self.custom_sets.len() >= MAX_CUSTOM_SETS {
                break;
            }
            if !self.queue.contains(set) {
                self.queue.push(*set);
                self.custom_sets.push(*set);
            }
        }
    }

    /// The scan was mid-way when the app last ran.
    pub fn was_interrupted(&self) -> bool {
        matches!(self.status, ScanStatus::Running | ScanStatus::Returning)
    }

    /// Configs left to try.
    pub fn can_continue(&self) -> bool {
        (self.next as usize) < self.queue.len()
    }
}

pub fn load_file(dir: &Path) -> Option<ScanFile> {
    let text = std::fs::read_to_string(dir.join(SCAN_FILE)).ok()?;
    serde_json::from_str::<ScanFile>(&text)
        .ok()
        .filter(|f| f.version == FILE_VERSION)
}

pub fn save_file(dir: &Path, f: &ScanFile) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(f).map_err(std::io::Error::other)?;
    let tmp = dir.join("scan.json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(tmp, dir.join(SCAN_FILE))
}

pub fn remove_file(dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(dir.join(SCAN_FILE)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// A band with no uplink, such as B32: it only works as an add-on to another band.
pub fn downlink_only(band: u8) -> bool {
    lte_band_meta(band).is_some_and(|m| m.duplex == Duplex::SdlOnly)
}

/// A combination the user may ask for: 2 bands or more, all supported, and one that can carry the
/// connection (not only downlink-only bands).
pub fn valid_custom(set: LteBandSet, supported: LteBandSet) -> bool {
    let bands = set.bands();
    bands.len() >= 2
        && bands.iter().all(|b| supported.contains(*b))
        && bands.iter().any(|b| !downlink_only(*b))
}

/// The current set first (it needs no reconnect), then each chosen band alone. A downlink-only band
/// can't work alone, so it doesn't get a turn.
pub fn quick_queue(start: LteBandSet, bands: &[u8]) -> Vec<LteBandSet> {
    let mut queue = vec![start];
    for b in bands.iter().filter(|b| !downlink_only(**b)) {
        if let Ok(set) = LteBandSet::from_bands(&[*b]) {
            if !queue.contains(&set) {
                queue.push(set);
            }
        }
    }
    queue
}

/// Pairs and triples of the single bands that worked best on download, then each add-on band with
/// each of those bands, skipping what's queued.
pub fn combinations(
    results: &[ScanResult],
    queued: &[LteBandSet],
    add_ons: &[u8],
) -> Vec<LteBandSet> {
    let mut worked: Vec<(u8, f64)> = results
        .iter()
        .filter(|r| r.outcome == ScanOutcome::Measured)
        .filter_map(|r| match r.bands.bands()[..] {
            [b] => Some((b, r.download_mbps.unwrap_or(0.0))),
            _ => None,
        })
        .collect();
    worked.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut top: Vec<u8> = worked
        .into_iter()
        .take(MAX_COMBINED_BANDS)
        .map(|w| w.0)
        .collect();
    top.sort_unstable();

    let mut out = Vec::new();
    let mut add = |bands: &[u8]| {
        if let Ok(set) = LteBandSet::from_bands(bands) {
            if !queued.contains(&set) && !out.contains(&set) {
                out.push(set);
            }
        }
    };
    for (i, a) in top.iter().enumerate() {
        for (j, b) in top.iter().enumerate().skip(i + 1) {
            add(&[*a, *b]);
            for c in top.iter().skip(j + 1) {
                add(&[*a, *b, *c]);
            }
        }
    }
    for add_on in add_ons {
        for b in &top {
            add(&[*b, *add_on]);
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Running

/// What a scan needs from the app around it.
#[async_trait]
pub trait ScanHost: Send + Sync {
    /// One speed test on the current config. `Err` holds the cause.
    async fn measure(&self) -> Result<ScanSpeed, String>;
    /// Called after every change to the record: write `scan.json` and tell the UI.
    fn publish(&self, file: &ScanFile);
    fn now_ms(&self) -> u64;
}

fn no_service_reason(r: RevertReason) -> bool {
    matches!(
        r,
        RevertReason::NoReattach
            | RevertReason::LossOfService
            | RevertReason::ResumedWithoutService
    )
}

async fn sample_for(
    ctl: &mut ChangeCtl,
    samples: &mut Vec<Sample>,
    window: Duration,
    stop: &watch::Receiver<bool>,
    must_stay_in_probation: bool,
) {
    let end = Instant::now() + window;
    while Instant::now() < end && !*stop.borrow() {
        let s = ctl.snap();
        if must_stay_in_probation && s.safe_apply.phase != Phase::Probation {
            return;
        }
        if s.session == SessionView::LoggedIn {
            if let Some(sig) = &s.signal {
                samples.push(Sample::from_signal(sig, s.in_service));
            }
        }
        sleep(SAMPLE_EVERY).await;
    }
}

fn reading(
    at_ms: u64,
    bands: LteBandSet,
    is_start: bool,
    outcome: ScanOutcome,
    note: Option<String>,
    samples: &[Sample],
) -> ScanResult {
    let last = samples.iter().rev().find(|s| s.lte_band.is_some());
    ScanResult {
        at_ms,
        bands,
        is_start,
        outcome,
        note,
        lte_band: last.and_then(|s| s.lte_band),
        lte_pci: last.and_then(|s| s.lte_pci),
        nr_band: samples.iter().rev().find_map(|s| s.nr_band),
        got_5g: samples.iter().any(|s| s.nr_present),
        lte_rsrp: Stat::of(samples.iter().map(|s| s.lte_rsrp)).avg,
        lte_sinr: Stat::of(samples.iter().map(|s| s.lte_sinr)).avg,
        nr_sinr: Stat::of(samples.iter().map(|s| s.nr_sinr)).avg,
        download_mbps: None,
        upload_mbps: None,
        idle_ping_ms: None,
        ping_down_ms: None,
        ping_up_ms: None,
        bytes_used: 0,
        tests: 0,
        download_min_mbps: None,
        download_max_mbps: None,
        upload_min_mbps: None,
        upload_max_mbps: None,
        endc: None,
        endc_restricted: None,
    }
}

fn present(tests: &[ScanSpeed], value: fn(&ScanSpeed) -> Option<f64>) -> Vec<f64> {
    tests.iter().filter_map(value).collect()
}

fn mean(tests: &[ScanSpeed], value: fn(&ScanSpeed) -> Option<f64>) -> Option<f64> {
    let v = present(tests, value);
    (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
}

fn lowest(tests: &[ScanSpeed], value: fn(&ScanSpeed) -> Option<f64>) -> Option<f64> {
    present(tests, value).into_iter().reduce(f64::min)
}

fn highest(tests: &[ScanSpeed], value: fn(&ScanSpeed) -> Option<f64>) -> Option<f64> {
    present(tests, value).into_iter().reduce(f64::max)
}

/// One config from apply to speed test. `None` when the user stopped the scan during it.
async fn try_config(
    ctl: &mut ChangeCtl,
    file: &mut ScanFile,
    bands: LteBandSet,
    apply_within: Duration,
    host: &dyn ScanHost,
    stop: &watch::Receiver<bool>,
) -> Option<ScanResult> {
    let is_start = bands == file.start.lte_bands;
    let cfg = NetConfig {
        lte_bands: bands,
        ..file.start.clone()
    };
    file.step = ScanStep::Applying;
    host.publish(file);
    let stop_rx = stop.clone();
    let mut stopped = move || *stop_rx.borrow();
    let mut retries = REFUSED_RETRIES;
    let applied = loop {
        // A router that's reconnecting would refuse the change outright.
        ctl.wait_for(ONLINE_WITHIN, |s| {
            s.session == SessionView::LoggedIn && !active(&s.safe_apply.phase)
        })
        .await;
        let applied = ctl
            .apply_limited(&cfg, apply_within, Some(GIVE_UP), &mut stopped)
            .await;
        // The band was never tried, as when the session had just expired.
        let refused = matches!(
            applied,
            Applied::Rejected(_)
                | Applied::Reverted {
                    reason: RevertReason::ApplyRejected,
                    ..
                }
        );
        if !refused || retries == 0 {
            break applied;
        }
        retries -= 1;
    };
    let mut samples = Vec::new();
    let done = |outcome, note: Option<String>, samples: &[Sample]| {
        reading(host.now_ms(), bands, is_start, outcome, note, samples)
    };
    match applied {
        Applied::Stopped => return None,
        Applied::Rejected(m) => return Some(done(ScanOutcome::Failed, Some(m), &samples)),
        Applied::TimedOut(p) => {
            let note = format!("no outcome in time, ended {p:?}");
            return Some(done(ScanOutcome::Failed, Some(note), &samples));
        }
        Applied::Reverted { reason, .. } => {
            let outcome = if no_service_reason(reason) {
                ScanOutcome::NoService
            } else {
                ScanOutcome::Failed
            };
            return Some(done(outcome, Some(format!("{reason:?}")), &samples));
        }
        Applied::AlreadyActive => {
            file.step = ScanStep::Settling;
            host.publish(file);
            sample_for(ctl, &mut samples, SETTLE_ACTIVE, stop, false).await;
        }
        Applied::Probation(_) => {
            file.step = ScanStep::Settling;
            host.publish(file);
            sample_for(ctl, &mut samples, SETTLE, stop, true).await;
            if *stop.borrow() {
                ctl.revert_and_wait().await;
                return None;
            }
            let snap = ctl.snap();
            if snap.safe_apply.phase != Phase::Probation {
                let phase = ctl.until_no_change(&mut |_| {}).await;
                return Some(match phase {
                    Phase::Reverted { reason, .. } if no_service_reason(reason) => done(
                        ScanOutcome::NoService,
                        Some(format!("{reason:?}")),
                        &samples,
                    ),
                    other => done(ScanOutcome::Failed, Some(format!("{other:?}")), &samples),
                });
            }
            // With auto-undo off the actor would sit on a dead config: the scan decides here.
            if !snap.in_service {
                ctl.give_up_and_wait().await;
                return Some(done(ScanOutcome::NoService, None, &samples));
            }
            if !ctl.keep().await {
                return Some(done(
                    ScanOutcome::Failed,
                    Some("couldn't be kept".into()),
                    &samples,
                ));
            }
        }
    }

    file.step = ScanStep::Testing;
    let mut tests = Vec::new();
    let mut failure = None;
    for n in 1..=file.repeats.clamp(1, MAX_REPEATS) {
        file.test = n;
        host.publish(file);
        match host.measure().await {
            Ok(m) => tests.push(m),
            Err(cause) => failure = Some(cause),
        }
        if *stop.borrow() {
            return None;
        }
    }
    if tests.is_empty() {
        sample_for(ctl, &mut samples, TEST_RETRY_AFTER, stop, false).await;
        match host.measure().await {
            Ok(m) => tests.push(m),
            Err(cause) => failure = Some(cause),
        }
        if *stop.borrow() {
            return None;
        }
    }
    // 5G often joins only under load.
    sample_for(ctl, &mut samples, SAMPLE_EVERY, stop, false).await;
    let (endc, endc_restricted) = ctl
        .snap()
        .status
        .map_or((None, None), |s| (s.endc, s.endc_restricted));
    if tests.is_empty() {
        return Some(ScanResult {
            endc,
            endc_restricted,
            ..done(ScanOutcome::TestFailed, failure, &samples)
        });
    }
    Some(ScanResult {
        endc,
        endc_restricted,
        download_mbps: mean(&tests, |t| t.download_mbps),
        upload_mbps: mean(&tests, |t| t.upload_mbps),
        idle_ping_ms: mean(&tests, |t| t.idle_ping_ms),
        ping_down_ms: mean(&tests, |t| t.ping_down_ms),
        ping_up_ms: mean(&tests, |t| t.ping_up_ms),
        bytes_used: tests.iter().map(|t| t.bytes_used).sum(),
        tests: tests.len() as u32,
        download_min_mbps: lowest(&tests, |t| t.download_mbps),
        download_max_mbps: highest(&tests, |t| t.download_mbps),
        upload_min_mbps: lowest(&tests, |t| t.upload_mbps),
        upload_max_mbps: highest(&tests, |t| t.upload_mbps),
        ..done(ScanOutcome::Measured, failure, &samples)
    })
}

/// Goes back to the starting config, retrying until the router takes it. Keeps the change once it
/// has settled: these bands were working before the scan. `stop` gives up, so a router that's gone
/// can't hold the app in the return; an open change is still finished by the actor.
pub async fn return_to_start(
    ctl: &mut ChangeCtl,
    start: &NetConfig,
    apply_within: Duration,
    max_tries: u32,
    stop: &mut (dyn FnMut() -> bool + Send),
) -> bool {
    for attempt in 0..max_tries {
        if attempt > 0 {
            ctl.wait_or_stop(RETURN_RETRY, &mut *stop, |_| false).await;
        }
        while ctl
            .wait_or_stop(Duration::from_secs(60), &mut *stop, |s| {
                !active(&s.safe_apply.phase)
            })
            .await
            .is_none()
        {
            if stop() {
                return false;
            }
        }
        ctl.wait_or_stop(ONLINE_WITHIN, &mut *stop, |s| {
            s.session == SessionView::LoggedIn && s.config.is_some()
        })
        .await;
        if stop() {
            return false;
        }
        if ctl.snap().config.as_ref() == Some(start) {
            return true;
        }
        match ctl.apply(start, apply_within, &mut *stop).await {
            Applied::AlreadyActive => return true,
            Applied::Probation(_) => {
                let never = watch::channel(false).1;
                sample_for(ctl, &mut Vec::new(), SETTLE_RETURN, &never, true).await;
                if ctl.keep().await {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// True once the stop switch is pressed from now on. Pressing it before this doesn't count: that's
/// what starts the return in the first place.
pub fn stop_pressed_again(stop: &watch::Receiver<bool>) -> impl FnMut() -> bool + Send {
    let mut rx = stop.clone();
    rx.mark_unchanged();
    move || rx.has_changed().unwrap_or(false)
}

/// Tries the rest of the queue, then returns to the starting bands. The actor holds back its own
/// automation meanwhile; a two-step apply would also test something other than the set asked for.
pub async fn run(
    ctl: &mut ChangeCtl,
    file: &mut ScanFile,
    grace: Duration,
    host: &dyn ScanHost,
    stop: watch::Receiver<bool>,
) {
    let apply_within = grace + APPLY_SLACK;
    ctl.send(Command::SetScanActive(true)).await;
    file.status = ScanStatus::Running;
    host.publish(file);
    let mut refused_in_a_row = 0;
    while !*stop.borrow() && refused_in_a_row < MAX_REFUSED_IN_A_ROW {
        if !file.can_continue() {
            if file.mode == ScanMode::Thorough && !file.combos_added {
                let combos = combinations(&file.results, &file.queue, &file.add_on_bands);
                file.queue.extend(combos);
                file.combos_added = true;
                host.publish(file);
                continue;
            }
            break;
        }
        let bands = file.queue[file.next as usize];
        let Some(result) = try_config(ctl, file, bands, apply_within, host, &stop).await else {
            break;
        };
        refused_in_a_row = if result.outcome == ScanOutcome::Failed {
            refused_in_a_row + 1
        } else {
            0
        };
        file.results.push(result);
        file.next += 1;
        host.publish(file);
    }
    let stopped = *stop.borrow();
    let unfinished = !stopped && file.can_continue();
    file.status = ScanStatus::Returning;
    host.publish(file);
    let start = file.start.clone();
    let mut pressed = stop_pressed_again(&stop);
    file.return_failed = !return_to_start(ctl, &start, apply_within, 4, &mut pressed).await;
    ctl.send(Command::SetScanActive(false)).await;
    file.status = if stopped {
        ScanStatus::Stopped
    } else if unfinished {
        ScanStatus::Interrupted
    } else {
        ScanStatus::Done
    };
    host.publish(file);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(b: &[u8]) -> LteBandSet {
        LteBandSet::from_bands(b).unwrap()
    }

    fn result(bands: &[u8], outcome: ScanOutcome, down: f64) -> ScanResult {
        ScanResult {
            download_mbps: Some(down),
            ..reading(0, set(bands), false, outcome, None, &[])
        }
    }

    #[test]
    fn quick_starts_on_the_current_set_and_skips_a_duplicate() {
        assert_eq!(
            quick_queue(set(&[1, 3]), &[1, 3, 28]),
            vec![set(&[1, 3]), set(&[1]), set(&[3]), set(&[28])]
        );
        assert_eq!(quick_queue(set(&[3]), &[1, 3]), vec![set(&[3]), set(&[1])]);
    }

    #[test]
    fn a_downlink_only_band_is_never_tried_alone() {
        assert!(downlink_only(32) && downlink_only(29) && !downlink_only(3));
        assert_eq!(quick_queue(set(&[3]), &[1, 32]), vec![set(&[3]), set(&[1])]);
    }

    #[test]
    fn thorough_adds_a_downlink_only_band_only_to_bands_that_worked() {
        let results = vec![
            result(&[1], ScanOutcome::Measured, 20.0),
            result(&[3], ScanOutcome::Measured, 100.0),
            result(&[43], ScanOutcome::NoService, 0.0),
        ];
        assert_eq!(
            combinations(&results, &[], &[32]),
            vec![set(&[1, 3]), set(&[1, 32]), set(&[3, 32])]
        );
        // Nothing worked, so there's nothing to add it to.
        assert!(combinations(&results[2..], &[], &[32]).is_empty());
    }

    #[test]
    fn the_file_remembers_the_add_on_bands() {
        let start = NetConfig {
            network_mode: crate::NetworkMode::Auto,
            lte_bands: set(&[3]),
            legacy_band_raw: "0".into(),
            nr_option: None,
        };
        let f = ScanFile::new("h", "mock", ScanMode::Thorough, start, &[1, 32], 5);
        assert_eq!(f.add_on_bands, vec![32]);
        assert_eq!(f.queue, vec![set(&[3]), set(&[1])]);
    }

    fn start_on(bands: &[u8]) -> NetConfig {
        NetConfig {
            network_mode: crate::NetworkMode::Auto,
            lte_bands: set(bands),
            legacy_band_raw: "0".into(),
            nr_option: None,
        }
    }

    #[test]
    fn custom_sets_follow_the_single_bands_and_skip_duplicates() {
        let mut f = ScanFile::new("h", "mock", ScanMode::Quick, start_on(&[1, 3]), &[3, 20], 5);
        f.add_custom(&[set(&[3, 20]), set(&[1, 3]), set(&[3, 20]), set(&[3])]);
        assert_eq!(
            f.queue,
            vec![set(&[1, 3]), set(&[3]), set(&[20]), set(&[3, 20])]
        );
        assert_eq!(f.custom_sets, vec![set(&[3, 20])]);
    }

    #[test]
    fn a_custom_set_needs_two_supported_bands_and_an_uplink() {
        let supported = set(&[1, 3, 20, 29, 32, 41]);
        assert!(!valid_custom(set(&[3]), supported), "one band");
        assert!(!valid_custom(set(&[29, 32]), supported), "downlink only");
        assert!(!valid_custom(set(&[3, 7]), supported), "B7 isn't supported");
        assert!(valid_custom(set(&[3, 32]), supported));
        assert!(valid_custom(set(&[3, 41]), supported));
    }

    #[test]
    fn thorough_does_not_repeat_a_custom_set() {
        let results = vec![
            result(&[1], ScanOutcome::Measured, 20.0),
            result(&[3], ScanOutcome::Measured, 100.0),
            result(&[20], ScanOutcome::Measured, 50.0),
        ];
        let mut f = ScanFile::new("h", "mock", ScanMode::Thorough, start_on(&[3]), &[1, 20], 5);
        f.add_custom(&[set(&[1, 3])]);
        let combos = combinations(&results, &f.queue, &[]);
        assert!(!combos.contains(&set(&[1, 3])));
        assert_eq!(combos, vec![set(&[1, 3, 20]), set(&[1, 20]), set(&[3, 20])]);
    }

    #[test]
    fn custom_sets_are_capped() {
        let mut f = ScanFile::new("h", "mock", ScanMode::Quick, start_on(&[3]), &[], 5);
        let many: Vec<LteBandSet> = (1u8..=20).map(|b| set(&[3, b + 30])).collect();
        f.add_custom(&many);
        assert_eq!(f.custom_sets.len(), MAX_CUSTOM_SETS);
        assert_eq!(f.queue.len(), 1 + MAX_CUSTOM_SETS);
        // Another call can't go past the cap either.
        f.add_custom(&[set(&[1, 3])]);
        assert_eq!(f.custom_sets.len(), MAX_CUSTOM_SETS);
    }

    #[test]
    fn combines_only_bands_that_worked_and_at_most_four() {
        let results = vec![
            result(&[1], ScanOutcome::Measured, 20.0),
            result(&[3], ScanOutcome::Measured, 100.0),
            result(&[43], ScanOutcome::NoService, 0.0),
            result(&[1, 3], ScanOutcome::Measured, 500.0),
            result(&[28], ScanOutcome::TestFailed, 900.0),
        ];
        assert_eq!(combinations(&results, &[], &[]), vec![set(&[1, 3])]);
        // Already queued, so nothing is added.
        assert!(combinations(&results, &[set(&[1, 3])], &[]).is_empty());

        let many: Vec<_> = [1u8, 3, 7, 8, 20, 28]
            .iter()
            .enumerate()
            .map(|(i, b)| result(&[*b], ScanOutcome::Measured, 100.0 - i as f64))
            .collect();
        let combos = combinations(&many, &[], &[]);
        assert_eq!(combos.len(), 10, "6 pairs and 4 triples");
        // The two slowest aren't combined.
        assert!(combos.iter().all(|c| !c.contains(20) && !c.contains(28)));
    }

    #[test]
    fn the_record_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_file(dir.path()).is_none());
        let start = NetConfig {
            network_mode: crate::NetworkMode::Auto,
            lte_bands: set(&[3]),
            legacy_band_raw: "0".into(),
            nr_option: None,
        };
        let mut f = ScanFile::new("h", "mock", ScanMode::Quick, start, &[1], 5);
        save_file(dir.path(), &f).unwrap();
        let back = load_file(dir.path()).unwrap();
        assert_eq!(back, f);
        assert!(back.was_interrupted());
        assert!(back.can_continue());

        f.status = ScanStatus::Done;
        f.next = 2;
        assert!(!f.was_interrupted());
        assert!(!f.can_continue());

        // A file from before repeated tests still loads, as one test per config.
        let mut old = serde_json::to_value(&f).unwrap();
        old.as_object_mut().unwrap().remove("repeats");
        old.as_object_mut().unwrap().remove("test");
        let back: ScanFile = serde_json::from_value(old).unwrap();
        assert_eq!(back.repeats, 1);

        // A file from before custom combinations loads with none.
        let mut old = serde_json::to_value(&f).unwrap();
        assert!(old.as_object_mut().unwrap().remove("customSets").is_some());
        let back: ScanFile = serde_json::from_value(old).unwrap();
        assert!(back.custom_sets.is_empty());

        std::fs::write(dir.path().join(SCAN_FILE), "not json").unwrap();
        assert!(load_file(dir.path()).is_none());
        remove_file(dir.path()).unwrap();
        remove_file(dir.path()).unwrap();
    }
}
