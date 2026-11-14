//! Speed test: a network peer besides the router, and only when the user presses Run. The only
//! other one is the optional update check (`crate::updates`).
//!
//! Every request is made here in Rust; the webview's CSP is unchanged. A test is refused while a
//! band change or a two-step apply is open, and each result records the serving band, PCI and
//! channel at the start. While a test runs the actor is told (`Command::SetSpeedTestActive`), so
//! it refuses a user Apply and holds back step 2 and the re-lock watchdog; the flag is cleared
//! when the test task ends in any way, a panic included. A result whose serving cell or band
//! config changed during the test is stored with an `invalid_reason`.
//! Results are stored in the history database (`crate::history`), which is location-sensitive
//! like `events.jsonl` and stays on this PC. `<app data>/speedtests.jsonl` (last 200) is only the
//! fallback while the database is unavailable; it's imported at the next start.
//!
//! Ookla falls back to Cloudflare only when the CLI never measured (terms not accepted, or it
//! couldn't be installed or started). A run that started and then failed or timed out is
//! reported as a failure instead of spending data on a second test. The Ookla CLI ignores the
//! Light/Full size (`size_applies` is `false` for its results).

pub mod cloudflare;
pub mod ookla;
pub mod schedule;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use router_core::{AppSnapshot, Command, LteBandSet, NetConfig, Phase, Rat, SafeApplyView};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::watch;
use ts_rs::TS;

use crate::commands::{AppState, CmdError};
use crate::settings::{SpeedTestProvider, SpeedTestSettings, SpeedTestSize};

pub const EVENT: &str = "speedtest";
pub const HISTORY_FILE: &str = "speedtests.jsonl";
pub const HISTORY_CAP: usize = 200;

pub const ERR_BUSY: &str = "Finish or revert the current band change first.";
pub const ERR_RUNNING: &str = "a speed test is already running";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SpeedPhase {
    /// Downloading and checking the Ookla CLI (first Ookla test only).
    Installing,
    IdlePing,
    Download,
    Upload,
    /// The Ookla CLI has started, and hasn't reported a phase yet.
    External,
    Done,
}

/// What was serving when the test started.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ServingCell {
    pub rat: Option<Rat>,
    pub lte_band: Option<u16>,
    pub lte_pci: Option<u16>,
    pub lte_earfcn: Option<u32>,
    /// A secondary LTE cell was active (carrier aggregation).
    pub lte_ca: bool,
    pub nr_band: Option<u16>,
    pub nr_pci: Option<u16>,
    pub nr_arfcn: Option<u32>,
}

impl ServingCell {
    /// Band, PCI and channel only: no cell or eNB identity is recorded.
    pub fn from_snapshot(s: &AppSnapshot) -> Self {
        let Some(sig) = &s.signal else {
            return Self::default();
        };
        let lte = sig.lte.as_ref();
        let nr = sig.nr.as_ref();
        Self {
            rat: sig.serving,
            lte_band: lte.and_then(|l| l.band),
            lte_pci: lte.and_then(|l| l.pci),
            lte_earfcn: lte.and_then(|l| l.earfcn_dl),
            lte_ca: lte.is_some_and(|l| l.scc_pci.is_some()),
            nr_band: nr.and_then(|n| n.band.as_ref()).map(|b| b.primary),
            nr_pci: nr.and_then(|n| n.pci),
            nr_arfcn: nr.and_then(|n| n.arfcn_dl),
        }
    }
}

/// Why an Ookla test was measured with Cloudflare instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum FallbackReason {
    TermsNotAccepted,
    /// The CLI couldn't be downloaded, checked or started.
    InstallFailed,
    /// Older results only: a failed run no longer falls back.
    Failed,
    /// Older results only: a timed-out run no longer falls back.
    TimedOut,
}

/// Why a test failed, so the UI can explain it plainly; the event's `message` holds the cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum FailReason {
    /// The server didn't answer in time (often a link still busy or very slow).
    Timeout,
    /// The name couldn't be looked up: no internet, or DNS is down.
    NoInternet,
    /// The connection was refused or reset.
    Refused,
    /// The secure connection couldn't be set up (TLS, certificates, a filtering proxy).
    Tls,
    /// The server answered with an HTTP error.
    Http,
    /// The Ookla CLI stopped without a result.
    Ookla,
    Other,
}

/// Why a finished result doesn't describe one band config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum InvalidReason {
    /// A band change opened, or the band config changed, during the test.
    BandChange,
    /// The serving LTE band or PCI, or the NR band, changed during the test.
    ServingChanged,
    /// The internet address moved to another range (or went away) during the test.
    IpChanged,
    /// An APN switch opened, or the APN in use changed, during the test.
    ApnChange,
}

/// What started a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SpeedTestSource {
    /// The user pressed Run.
    Manual,
    /// One of an auto-scan's tests.
    Scan,
    /// An automatic test on the schedule.
    Auto,
}

/// Live values while a test runs; every field is `None` until measured.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Live {
    pub idle_ping_ms: Option<f64>,
    pub download_mbps: Option<f64>,
    pub upload_mbps: Option<f64>,
    /// Median ping while downloading.
    pub ping_down_ms: Option<f64>,
    /// Median ping while uploading.
    pub ping_up_ms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpeedTestResult {
    #[ts(type = "number")]
    pub at_ms: u64,
    /// The provider that measured.
    pub provider: SpeedTestProvider,
    /// The provider the user had chosen.
    pub requested: SpeedTestProvider,
    pub fallback: Option<FallbackReason>,
    /// Short English detail for a fallback (the CLI's own message), for the log only.
    pub fallback_detail: Option<String>,
    pub size: SpeedTestSize,
    pub serving: ServingCell,
    pub idle_ping_ms: Option<f64>,
    pub download_mbps: Option<f64>,
    pub upload_mbps: Option<f64>,
    pub ping_down_ms: Option<f64>,
    pub ping_up_ms: Option<f64>,
    #[ts(type = "number")]
    pub bytes_used: u64,
    /// Server or data-centre name, as the provider reports it.
    pub server: Option<String>,
    /// `false` when the provider ignored the Light/Full size (the Ookla CLI decides how much
    /// data it uses). Missing in older results.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub size_applies: Option<bool>,
    /// Set when the result doesn't describe a single band config; `None` means valid.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub invalid_reason: Option<InvalidReason>,
    /// The band set the router was allowed to use. Missing in older results.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub allowed_bands: Option<LteBandSet>,
    /// What started the test. Missing in older results: the UI infers it from the scan periods.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub source: Option<SpeedTestSource>,
    /// The internet address's range at the start, "a.b". Never more than the first two numbers.
    /// Missing in older results.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub ip_range: Option<String>,
}

/// `range` when it is exactly two numbers 0..=255 ("10.45"), so nothing longer is ever stored.
pub fn clean_ip_range(range: Option<&str>) -> Option<String> {
    let range = range?;
    let mut parts = range.split('.');
    let ok = |p: Option<&str>| {
        p.is_some_and(|p| !p.is_empty() && p.len() <= 3 && p.parse::<u8>().is_ok())
    };
    (ok(parts.next()) && ok(parts.next()) && parts.next().is_none()).then(|| range.to_string())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpeedTestProgress {
    pub phase: SpeedPhase,
    /// 0..1 within the phase.
    pub fraction: f64,
    #[ts(type = "number")]
    pub bytes_used: u64,
    pub live: Live,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpeedTestStart {
    pub provider: SpeedTestProvider,
    pub size: SpeedTestSize,
    pub serving: ServingCell,
    /// `false` when the chosen provider will ignore `size` (Ookla with accepted terms).
    #[serde(default)]
    #[ts(optional = nullable)]
    pub size_applies: Option<bool>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub source: Option<SpeedTestSource>,
}

/// Whether a test with these settings honours the Light/Full size. Ookla without accepted
/// terms measures with Cloudflare, which does.
pub fn size_applies(cfg: &SpeedTestSettings) -> bool {
    !(cfg.provider == SpeedTestProvider::Ookla && cfg.ookla_terms_accepted)
}

/// The `speedtest` event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum SpeedTestEvent {
    Started(SpeedTestStart),
    Progress(SpeedTestProgress),
    Finished(SpeedTestResult),
    Failed { message: String, reason: FailReason },
    Cancelled,
}

// ---------------------------------------------------------------------------------------------
// History

pub fn history_path(data_dir: &Path) -> PathBuf {
    data_dir.join(HISTORY_FILE)
}

/// Newest last. Unreadable lines are skipped.
pub fn load_history(data_dir: &Path) -> Vec<SpeedTestResult> {
    std::fs::read_to_string(history_path(data_dir))
        .map(|t| {
            t.lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Appends one result and keeps the newest `HISTORY_CAP`.
pub fn append_history(data_dir: &Path, r: &SpeedTestResult) -> std::io::Result<()> {
    let mut all = load_history(data_dir);
    all.push(r.clone());
    let skip = all.len().saturating_sub(HISTORY_CAP);
    let mut text = String::new();
    for r in &all[skip..] {
        text.push_str(&serde_json::to_string(r).map_err(std::io::Error::other)?);
        text.push('\n');
    }
    let path = history_path(data_dir);
    let tmp = path.with_extension("jsonl.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, path)
}

// ---------------------------------------------------------------------------------------------
// Running a test

type Running = Arc<Mutex<Option<watch::Sender<bool>>>>;

/// Only one test at a time; holds the cancel switch of the running one.
#[derive(Default)]
pub struct SpeedTestState {
    running: Running,
}

fn lock(r: &Running) -> std::sync::MutexGuard<'_, Option<watch::Sender<bool>>> {
    r.lock().unwrap_or_else(|p| p.into_inner())
}

impl SpeedTestState {
    /// Claims the single test slot. `on_end` runs once when the returned guard drops: when the
    /// test task finishes, is cancelled, or panics.
    pub fn try_begin(
        &self,
        on_end: Box<dyn FnOnce() + Send>,
    ) -> Option<(RunGuard, watch::Receiver<bool>)> {
        let mut g = lock(&self.running);
        if g.is_some() {
            return None;
        }
        let (tx, rx) = watch::channel(false);
        *g = Some(tx);
        let guard = RunGuard {
            running: self.running.clone(),
            on_end: Some(on_end),
        };
        Some((guard, rx))
    }

    pub fn is_running(&self) -> bool {
        lock(&self.running).is_some()
    }

    pub fn cancel(&self) {
        if let Some(tx) = lock(&self.running).as_ref() {
            let _ = tx.send(true);
        }
    }
}

/// Held by the test task. Dropping it frees the slot and runs `on_end` (which tells the actor
/// the test is over), however the task ends.
pub struct RunGuard {
    running: Running,
    on_end: Option<Box<dyn FnOnce() + Send>>,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        *lock(&self.running) = None;
        if let Some(f) = self.on_end.take() {
            f();
        }
    }
}

/// A band change is open, or a two-step apply hasn't finished (it may sit between its steps
/// with the phase already `Committed`).
pub fn band_change_open(sa: &SafeApplyView) -> bool {
    sa.sequence.is_some()
        || !matches!(
            sa.phase,
            Phase::Idle | Phase::Committed | Phase::Reverted { .. }
        )
}

/// A Refresh IP run is open: mobile data goes off and on, so nothing may measure or change
/// the bands meanwhile.
pub fn ip_refresh_active(s: &AppSnapshot) -> bool {
    s.ip_refresh.as_ref().is_some_and(|v| v.active)
}

/// An APN switch is open: the data connection may drop, and the safe apply is watching it.
pub fn apn_change_open(s: &AppSnapshot) -> bool {
    s.apn_apply.as_ref().is_some_and(|v| {
        !matches!(
            v.phase,
            Phase::Idle | Phase::Committed | Phase::Reverted { .. }
        )
    })
}

/// Why a test can't start from this snapshot: a band change, a Refresh IP run or an APN switch.
pub fn start_refusal(s: &AppSnapshot) -> Option<&'static str> {
    if band_change_open(&s.safe_apply) {
        Some(ERR_BUSY)
    } else if ip_refresh_active(s) {
        Some(router_core::IP_REFRESH_RUNNING)
    } else if apn_change_open(s) {
        Some(router_core::APN_CHANGE_RUNNING)
    } else {
        None
    }
}

/// An APN switch in `phase`, for tests elsewhere in the app.
#[cfg(test)]
pub(crate) fn apn_change(phase: Phase) -> router_core::ApnApplyView {
    router_core::ApnApplyView {
        phase,
        seconds_left: None,
        previous: None,
        requested: None,
    }
}

/// An open Refresh IP run, for tests elsewhere in the app.
#[cfg(test)]
pub(crate) fn running_ip_refresh() -> router_core::IpRefreshView {
    let now = std::time::Instant::now();
    let mut m = router_core::ip_refresh::IpRefresh::new();
    m.start(
        None,
        1,
        Some("10.45".into()),
        &[router_core::IpMethod::DataSwitch],
        now,
    )
    .unwrap();
    m.view(now, false)
}

/// What the result is compared against at the end.
#[derive(Debug, Clone, PartialEq)]
pub struct Baseline {
    pub serving: ServingCell,
    pub config: Option<NetConfig>,
    /// The internet address's range ("a.b"), when known.
    pub ip_range: Option<String>,
    /// Index of the APN profile in use, when known.
    pub apn: Option<u32>,
}

impl Baseline {
    pub fn from_snapshot(s: &AppSnapshot) -> Self {
        Self {
            serving: ServingCell::from_snapshot(s),
            config: s.config.clone(),
            ip_range: clean_ip_range(s.wan.as_ref().and_then(|w| w.range.as_deref())),
            apn: s.apn.as_ref().and_then(|a| a.current),
        }
    }
}

/// Compares the end of a test with its start. A missing end snapshot (the actor restarted)
/// can't vouch for the result either. Carrier aggregation and the NSA 5G leg coming and going
/// with load are normal: only an NR band that changed counts, not one that appeared or left.
/// An internet address that moved to another range, or went away, counts too; an end snapshot
/// that doesn't know the address says nothing.
pub fn invalid_reason(start: &Baseline, end: Option<&AppSnapshot>) -> Option<InvalidReason> {
    let Some(end) = end else {
        return Some(InvalidReason::BandChange);
    };
    let config_changed = matches!(
        (&start.config, &end.config),
        (Some(a), Some(b)) if a != b
    );
    if band_change_open(&end.safe_apply) || config_changed {
        return Some(InvalidReason::BandChange);
    }
    let apn_moved = matches!(
        (start.apn, end.apn.as_ref().and_then(|a| a.current)),
        (Some(a), Some(b)) if a != b
    );
    if apn_change_open(end) || apn_moved {
        return Some(InvalidReason::ApnChange);
    }
    let now = ServingCell::from_snapshot(end);
    let a = &start.serving;
    let lte_changed =
        a.lte_band.is_some() && (a.lte_band, a.lte_pci) != (now.lte_band, now.lte_pci);
    let nr_changed = matches!((a.nr_band, now.nr_band), (Some(x), Some(y)) if x != y);
    if lte_changed || nr_changed {
        return Some(InvalidReason::ServingChanged);
    }
    if let (Some(from), Some(wan)) = (&start.ip_range, &end.wan) {
        if wan.range.as_ref() != Some(from) {
            return Some(InvalidReason::IpChanged);
        }
    }
    None
}

/// Where requests go; tests swap these for a mock server.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub cloudflare: String,
    pub ookla_zip: String,
    pub ookla_zip_sha: String,
    pub ookla_exe_sha: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            cloudflare: cloudflare::DEFAULT_BASE.into(),
            ookla_zip: ookla::ZIP_URL.into(),
            ookla_zip_sha: ookla::ZIP_SHA256.into(),
            ookla_exe_sha: ookla::EXE_SHA256.into(),
        }
    }
}

pub enum Outcome {
    Done(Box<SpeedTestResult>),
    Cancelled,
    /// Why, and the technical cause for the message.
    Failed(FailReason, String),
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// How an Ookla error is handled: `Ok` falls back to Cloudflare (the CLI never measured, so no
/// test data was spent), `Err` is the failure to report instead.
fn ookla_fallback(
    e: ookla::OoklaError,
) -> Result<(FallbackReason, Option<String>), (FailReason, String)> {
    match e {
        ookla::OoklaError::Install(d) | ookla::OoklaError::NotStarted(d) => {
            Ok((FallbackReason::InstallFailed, Some(d)))
        }
        ookla::OoklaError::Failed(d) => Err((
            FailReason::Ookla,
            format!("Ookla Speedtest stopped without a result: {d}"),
        )),
        ookla::OoklaError::TimedOut => Err((
            FailReason::Timeout,
            format!(
                "Ookla Speedtest gave no result within {} s",
                ookla::RUN_TIMEOUT.as_secs()
            ),
        )),
        ookla::OoklaError::Cancelled => Err((FailReason::Other, "cancelled".into())),
    }
}

/// Runs one test with the chosen provider. Ookla falls back to Cloudflare only when the CLI
/// never measured; a run that started and failed is reported as a failure.
pub async fn run_test(
    cfg: SpeedTestSettings,
    serving: ServingCell,
    data_dir: &Path,
    ep: &Endpoints,
    cancel: watch::Receiver<bool>,
    emit: &(dyn Fn(SpeedTestEvent) + Send + Sync),
) -> Outcome {
    let mut result = SpeedTestResult {
        at_ms: now_ms(),
        provider: SpeedTestProvider::Cloudflare,
        requested: cfg.provider,
        fallback: None,
        fallback_detail: None,
        size: cfg.size,
        serving,
        idle_ping_ms: None,
        download_mbps: None,
        upload_mbps: None,
        ping_down_ms: None,
        ping_up_ms: None,
        bytes_used: 0,
        server: None,
        size_applies: Some(true),
        invalid_reason: None,
        allowed_bands: None,
        source: None,
        ip_range: None,
    };
    let progress = |phase, fraction, bytes_used, live: &Live| {
        emit(SpeedTestEvent::Progress(SpeedTestProgress {
            phase,
            fraction,
            bytes_used,
            live: live.clone(),
        }))
    };

    if cfg.provider == SpeedTestProvider::Ookla {
        if !cfg.ookla_terms_accepted {
            result.fallback = Some(FallbackReason::TermsNotAccepted);
        } else {
            match run_ookla(data_dir, ep, cancel.clone(), &progress).await {
                Ok(m) => {
                    result.provider = SpeedTestProvider::Ookla;
                    result.idle_ping_ms = m.idle_ping_ms;
                    result.download_mbps = m.download_mbps;
                    result.upload_mbps = m.upload_mbps;
                    result.ping_down_ms = m.ping_down_ms;
                    result.ping_up_ms = m.ping_up_ms;
                    result.bytes_used = m.bytes_used;
                    result.server = m.server;
                    // The CLI picks its own amount of data.
                    result.size_applies = Some(false);
                    progress(SpeedPhase::Done, 1.0, m.bytes_used, &Live::default());
                    return Outcome::Done(Box::new(result));
                }
                Err(ookla::OoklaError::Cancelled) => return Outcome::Cancelled,
                Err(e) => match ookla_fallback(e) {
                    Ok((reason, detail)) => {
                        result.fallback = Some(reason);
                        result.fallback_detail = detail;
                    }
                    Err((reason, message)) => return Outcome::Failed(reason, message),
                },
            }
        }
    }

    let plan = cloudflare::Plan::for_size(cfg.size);
    match cloudflare::run(&ep.cloudflare, &plan, cancel, &progress).await {
        Ok(m) => {
            result.idle_ping_ms = m.idle_ping_ms;
            result.download_mbps = m.download_mbps;
            result.upload_mbps = m.upload_mbps;
            result.ping_down_ms = m.ping_down_ms;
            result.ping_up_ms = m.ping_up_ms;
            result.bytes_used = m.bytes_used;
            result.server = m.server.map(|c| format!("Cloudflare {c}"));
            Outcome::Done(Box::new(result))
        }
        Err(cloudflare::CfError::Cancelled) => Outcome::Cancelled,
        Err(cloudflare::CfError::Unreachable(f)) => Outcome::Failed(f.reason, f.detail),
    }
}

async fn run_ookla(
    data_dir: &Path,
    ep: &Endpoints,
    cancel: watch::Receiver<bool>,
    progress: &(dyn Fn(SpeedPhase, f64, u64, &Live) + Send + Sync),
) -> Result<ookla::OoklaMeasured, ookla::OoklaError> {
    let exe = ookla::exe_path(data_dir);
    let check = {
        let exe = exe.clone();
        let sha = ep.ookla_exe_sha.clone();
        tokio::task::spawn_blocking(move || ookla::is_installed(&exe, &sha))
    };
    if !check.await.unwrap_or(false) {
        progress(SpeedPhase::Installing, 0.0, 0, &Live::default());
        ookla::install(
            &ep.ookla_zip,
            &ep.ookla_zip_sha,
            &ep.ookla_exe_sha,
            &exe,
            cancel.clone(),
        )
        .await?;
    }
    progress(SpeedPhase::External, 0.0, 0, &Live::default());
    ookla::run(
        &exe,
        &ep.ookla_exe_sha,
        ookla::RUN_TIMEOUT,
        cancel,
        progress,
    )
    .await
}

// ---------------------------------------------------------------------------------------------
// Commands

/// Starts a test in the background; progress and the result arrive as `speedtest` events.
/// Refused while a band change or two-step apply is open, or another test runs.
#[tauri::command]
pub fn run_speed_test(
    app: AppHandle,
    state: State<'_, AppState>,
    st: State<'_, SpeedTestState>,
) -> Result<(), CmdError> {
    if app
        .try_state::<crate::autoscan::ScanState>()
        .is_some_and(|s| s.is_running())
    {
        return Err(CmdError::Message(crate::autoscan::ERR_RUNNING.into()));
    }
    let snap = state.snapshot();
    if let Some(why) = start_refusal(&snap) {
        return Err(CmdError::Message(why.into()));
    }
    if state.page_scripts_reading() {
        return Err(CmdError::Message(router_core::PAGE_SCRIPTS_RUNNING.into()));
    }
    let Some((guard, cancel_rx)) = begin(&app, &state, &st) else {
        return Err(CmdError::Message(ERR_RUNNING.into()));
    };
    let baseline = Baseline::from_snapshot(&snap);
    let req = RunRequest {
        cfg: state.speed_test_settings(),
        source: SpeedTestSource::Manual,
    };
    tauri::async_runtime::spawn(async move {
        let _ = execute(app, req, baseline, guard, cancel_rx).await;
    });
    Ok(())
}

/// Claims the test slot and tells the actor. The `false` is queued when the guard drops, so it
/// always reaches the actor after the `true`.
fn begin(
    app: &AppHandle,
    state: &AppState,
    st: &SpeedTestState,
) -> Option<(RunGuard, watch::Receiver<bool>)> {
    let end_app = app.clone();
    let on_end: Box<dyn FnOnce() + Send> = Box::new(move || {
        if let Some(state) = end_app.try_state::<AppState>() {
            state.send_now(Command::SetSpeedTestActive(false));
        }
    });
    // The page-script read shares the router's session; a test would compete with its GETs.
    if state.page_scripts_reading() {
        return None;
    }
    let claimed = st.try_begin(on_end)?;
    state.send_now(Command::SetSpeedTestActive(true));
    Some(claimed)
}

/// What to run, and who asked.
#[derive(Debug, Clone, Copy)]
pub struct RunRequest {
    pub cfg: SpeedTestSettings,
    pub source: SpeedTestSource,
}

/// Runs one test, saves a result, and sends the events the Speed test page follows. The events go
/// out for an auto-scan's tests too, so its live numbers and history use the same code.
async fn execute(
    app: AppHandle,
    req: RunRequest,
    baseline: Baseline,
    guard: RunGuard,
    cancel_rx: watch::Receiver<bool>,
) -> Result<SpeedTestResult, String> {
    let Some(data_dir) = app
        .try_state::<AppState>()
        .map(|s| s.data_dir().to_path_buf())
    else {
        return Err("the app is shutting down".into());
    };
    let RunRequest { cfg, source } = req;
    let provider = cfg.provider;
    let serving = baseline.serving.clone();
    let _ = app.emit(
        EVENT,
        SpeedTestEvent::Started(SpeedTestStart {
            provider: cfg.provider,
            size: cfg.size,
            serving: serving.clone(),
            size_applies: Some(size_applies(&cfg)),
            source: Some(source),
        }),
    );
    // Dropped however this ends, a panic included.
    let guard = guard;
    let a = app.clone();
    let emit = move |e: SpeedTestEvent| {
        let _ = a.emit(EVENT, e);
    };
    let out = run_test(
        cfg,
        serving,
        &data_dir,
        &Endpoints::default(),
        cancel_rx,
        &emit,
    )
    .await;
    let (event, result) = match out {
        Outcome::Done(r) => {
            let mut r = *r;
            let end = app.try_state::<AppState>().map(|s| s.snapshot());
            r.invalid_reason = invalid_reason(&baseline, end.as_ref());
            r.allowed_bands = baseline.config.as_ref().map(|c| c.lte_bands);
            r.source = Some(source);
            r.ip_range = baseline.ip_range.clone();
            let (a, rec) = (app.clone(), r.clone());
            let _ = tokio::task::spawn_blocking(move || crate::history::record_speedtest(&a, &rec))
                .await;
            (SpeedTestEvent::Finished(r.clone()), Ok(r))
        }
        Outcome::Cancelled => (SpeedTestEvent::Cancelled, Err("cancelled".to_string())),
        Outcome::Failed(reason, message) => {
            // Kept in the event log so a failure can be diagnosed afterwards.
            let (dir, m) = (data_dir.clone(), message.clone());
            let _ = tokio::task::spawn_blocking(move || {
                log_failure(&dir, provider, source, reason, &m)
            })
            .await;
            (
                SpeedTestEvent::Failed {
                    message: message.clone(),
                    reason,
                },
                Err(message),
            )
        }
    };
    drop(guard);
    emit(event);
    result
}

/// One test for the auto-scan, on the config the router has now.
pub async fn measure_now(app: &AppHandle) -> Result<SpeedTestResult, String> {
    let (Some(state), Some(st)) = (
        app.try_state::<AppState>(),
        app.try_state::<SpeedTestState>(),
    ) else {
        return Err("the app is shutting down".into());
    };
    let snap = state.snapshot();
    let Some((guard, cancel_rx)) = begin(app, &state, &st) else {
        return Err(ERR_RUNNING.into());
    };
    let baseline = Baseline::from_snapshot(&snap);
    let req = RunRequest {
        cfg: state.speed_test_settings(),
        source: SpeedTestSource::Scan,
    };
    execute(app.clone(), req, baseline, guard, cancel_rx).await
}

/// One automatic test, with the scheduler's provider and size. `None` when another test already
/// holds the slot: `begin()` refused, which the scheduler counts as the speed-test blocker.
pub async fn run_auto(
    app: &AppHandle,
    auto: crate::settings::AutoSpeedTestSettings,
) -> Option<Result<SpeedTestResult, String>> {
    let (Some(state), Some(st)) = (
        app.try_state::<AppState>(),
        app.try_state::<SpeedTestState>(),
    ) else {
        return Some(Err("the app is shutting down".into()));
    };
    let snap = state.snapshot();
    let (guard, cancel_rx) = begin(app, &state, &st)?;
    schedule::on_started(app);
    let req = RunRequest {
        cfg: SpeedTestSettings {
            provider: auto.provider,
            size: auto.size,
            // Ookla only with its terms accepted; without them run_test measures with Cloudflare.
            ookla_terms_accepted: state.speed_test_settings().ookla_terms_accepted,
        },
        source: SpeedTestSource::Auto,
    };
    let baseline = Baseline::from_snapshot(&snap);
    Some(execute(app.clone(), req, baseline, guard, cancel_rx).await)
}

fn log_failure(
    dir: &Path,
    provider: SpeedTestProvider,
    source: SpeedTestSource,
    reason: FailReason,
    message: &str,
) {
    let rec = router_core::EventRecord {
        at_ms: now_ms(),
        kind: router_core::event_text::kinds::SPEEDTEST_FAILED.into(),
        detail: serde_json::json!({
            "provider": provider,
            "source": source,
            "reason": reason,
            "message": message,
        }),
    };
    let _ = router_core::EventLog::new(dir.to_path_buf()).append(&rec);
}

#[tauri::command]
pub fn cancel_speed_test(st: State<'_, SpeedTestState>) {
    st.cancel();
}

#[tauri::command]
pub fn speed_test_running(st: State<'_, SpeedTestState>) -> bool {
    st.is_running()
}

/// Newest first: the newest `HISTORY_CAP`, or the last 7 days when that's more, from the history
/// database (and the fallback file while the database is unavailable or read-only).
#[tauri::command]
pub async fn speed_test_history(
    state: State<'_, AppState>,
    history: State<'_, Arc<crate::history::HistoryStore>>,
) -> Result<Vec<SpeedTestResult>, CmdError> {
    let dir = state.data_dir().to_path_buf();
    let store = history.inner().clone();
    tokio::task::spawn_blocking(move || crate::history::recent_speedtests(&store, &dir))
        .await
        .map_err(|e| CmdError::Message(e.to_string()))?
        .map_err(CmdError::Message)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum OoklaDoc {
    Eula,
    Privacy,
}

/// Opens Ookla's licence or privacy page in the browser. The URLs are fixed here, so the
/// webview needs no opener permission and can't open anything else.
#[tauri::command]
pub fn open_ookla_terms(app: AppHandle, doc: OoklaDoc) -> Result<(), CmdError> {
    use tauri_plugin_opener::OpenerExt;
    let url = match doc {
        OoklaDoc::Eula => ookla::EULA_URL,
        OoklaDoc::Privacy => ookla::PRIVACY_URL,
    };
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| CmdError::Message(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    struct Down;
    impl Respond for Down {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            let n: usize = req
                .url
                .query_pairs()
                .find(|(k, _)| k == "bytes")
                .and_then(|(_, v)| v.parse().ok())
                .unwrap_or(0);
            ResponseTemplate::new(200).set_body_bytes(vec![0u8; n])
        }
    }

    fn result(at: u64) -> SpeedTestResult {
        SpeedTestResult {
            at_ms: at,
            provider: SpeedTestProvider::Cloudflare,
            requested: SpeedTestProvider::Cloudflare,
            fallback: None,
            fallback_detail: None,
            size: SpeedTestSize::Light,
            serving: ServingCell {
                lte_band: Some(3),
                lte_pci: Some(103),
                nr_band: Some(41),
                ..Default::default()
            },
            idle_ping_ms: Some(41.0),
            download_mbps: Some(108.0),
            upload_mbps: Some(1.3),
            ping_down_ms: Some(600.0),
            ping_up_ms: Some(1450.0),
            bytes_used: 40_000_000,
            server: None,
            size_applies: Some(true),
            invalid_reason: None,
            allowed_bands: None,
            source: None,
            ip_range: None,
        }
    }

    #[test]
    fn history_keeps_the_newest_200() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_history(dir.path()).is_empty());
        for i in 0..205 {
            append_history(dir.path(), &result(i)).unwrap();
        }
        let h = load_history(dir.path());
        assert_eq!(h.len(), HISTORY_CAP);
        assert_eq!(h.first().unwrap().at_ms, 5);
        assert_eq!(h.last().unwrap().at_ms, 204);
    }

    fn view(phase: Phase, sequence: bool) -> SafeApplyView {
        let lte = |b: &[u8]| router_core::LteBandSet::from_bands(b).unwrap();
        SafeApplyView {
            phase,
            seconds_left: None,
            previous: None,
            requested: None,
            policy: None,
            sequence: sequence.then(|| router_core::ApplySequenceView {
                step: 2,
                total: 2,
                primary: 3,
                final_config: NetConfig {
                    network_mode: router_core::NetworkMode::Auto,
                    lte_bands: lte(&[1, 3]),
                    legacy_band_raw: "0".into(),
                    nr_option: None,
                },
                origin: router_core::SequenceOrigin::User,
                step1: None,
            }),
        }
    }

    #[test]
    fn only_a_settled_band_config_allows_a_test() {
        assert!(!band_change_open(&view(Phase::Idle, false)));
        let mut s = snap_with(&ServingCell::default(), None, Phase::Idle);
        assert_eq!(start_refusal(&s), None);
        s.ip_refresh = Some(running_ip_refresh());
        assert_eq!(start_refusal(&s), Some(router_core::IP_REFRESH_RUNNING));
        s.ip_refresh.as_mut().unwrap().active = false;
        assert_eq!(start_refusal(&s), None, "a finished run holds nothing");
        s.safe_apply.phase = Phase::Probation;
        assert_eq!(start_refusal(&s), Some(ERR_BUSY));
        assert!(!band_change_open(&view(Phase::Committed, false)));
        assert!(band_change_open(&view(Phase::Probation, false)));
        assert!(band_change_open(&view(Phase::Applying, false)));
        // Between the steps of a two-step apply the phase is Committed, but step 2 is due.
        assert!(band_change_open(&view(Phase::Committed, true)));
    }

    fn snap_with(serving: &ServingCell, config: Option<NetConfig>, phase: Phase) -> AppSnapshot {
        use router_core::{Duplex, LteSignal, NrBandInfo, NrSignal, Signal};
        let lte = LteSignal {
            band: serving.lte_band,
            pci: serving.lte_pci,
            ..LteSignal::default()
        };
        let nr = serving.nr_band.map(|b| NrSignal {
            band: Some(NrBandInfo {
                primary: b,
                alternates: vec![],
                duplex: Duplex::Tdd,
                dl_mhz: 0.0,
            }),
            ..NrSignal::default()
        });
        AppSnapshot {
            session: router_core::SessionView::LoggedIn,
            device: None,
            status: None,
            signal: Some(Signal {
                serving: Some(Rat::Lte),
                lte: Some(lte),
                nr,
            }),
            config,
            capabilities: None,
            safe_apply: view(phase, false),
            in_service: true,
            foreign_journal: None,
            captured_at_ms: 0,
            relock: None,
            wan: None,
            ip_refresh: None,
            data_switch_off: false,
            apn: None,
            apn_apply: None,
            ip_reattach: None,
        }
    }

    #[test]
    fn a_result_is_marked_when_the_band_or_cell_changed() {
        let net = |b: &[u8]| NetConfig {
            network_mode: router_core::NetworkMode::Auto,
            lte_bands: router_core::LteBandSet::from_bands(b).unwrap(),
            legacy_band_raw: "0".into(),
            nr_option: None,
        };
        let cell = |band, pci, nr| ServingCell {
            lte_band: Some(band),
            lte_pci: Some(pci),
            nr_band: nr,
            ..Default::default()
        };
        let start = Baseline {
            serving: cell(3, 103, Some(41)),
            config: Some(net(&[3])),
            ip_range: None,
            apn: None,
        };
        let same = snap_with(&cell(3, 103, Some(41)), Some(net(&[3])), Phase::Idle);
        assert_eq!(invalid_reason(&start, Some(&same)), None);
        // NSA 5G leaving with the load isn't a change of cell.
        let no_nr = snap_with(&cell(3, 103, None), Some(net(&[3])), Phase::Idle);
        assert_eq!(invalid_reason(&start, Some(&no_nr)), None);
        let moved = snap_with(&cell(1, 120, None), Some(net(&[3])), Phase::Idle);
        assert_eq!(
            invalid_reason(&start, Some(&moved)),
            Some(InvalidReason::ServingChanged)
        );
        let other_pci = snap_with(&cell(3, 9, Some(41)), Some(net(&[3])), Phase::Idle);
        assert_eq!(
            invalid_reason(&start, Some(&other_pci)),
            Some(InvalidReason::ServingChanged)
        );
        let reconfigured = snap_with(&cell(3, 103, Some(41)), Some(net(&[1, 3])), Phase::Idle);
        assert_eq!(
            invalid_reason(&start, Some(&reconfigured)),
            Some(InvalidReason::BandChange)
        );
        let open = snap_with(&cell(3, 103, Some(41)), Some(net(&[3])), Phase::Probation);
        assert_eq!(
            invalid_reason(&start, Some(&open)),
            Some(InvalidReason::BandChange)
        );
        assert_eq!(
            invalid_reason(&start, None),
            Some(InvalidReason::BandChange)
        );
    }

    #[test]
    fn an_apn_switch_holds_a_test_back_and_marks_its_result() {
        let cell = ServingCell {
            lte_band: Some(3),
            lte_pci: Some(103),
            ..Default::default()
        };
        let mut s = snap_with(&cell, None, Phase::Idle);
        s.apn_apply = Some(apn_change(Phase::Probation));
        assert!(apn_change_open(&s));
        assert_eq!(start_refusal(&s), Some(router_core::APN_CHANGE_RUNNING));
        s.apn_apply = Some(apn_change(Phase::Committed));
        assert!(!apn_change_open(&s));
        assert_eq!(start_refusal(&s), None, "a finished switch holds nothing");

        let profiles = |current| router_core::ApnProfiles {
            current: Some(current),
            profiles: vec![],
            auto_apn: None,
        };
        let mut start_snap = snap_with(&cell, None, Phase::Idle);
        start_snap.apn = Some(profiles(1));
        let start = Baseline::from_snapshot(&start_snap);
        assert_eq!(start.apn, Some(1));
        let mut end = start_snap.clone();
        assert_eq!(invalid_reason(&start, Some(&end)), None);
        end.apn = Some(profiles(2));
        assert_eq!(
            invalid_reason(&start, Some(&end)),
            Some(InvalidReason::ApnChange)
        );
        end.apn = None;
        assert_eq!(
            invalid_reason(&start, Some(&end)),
            None,
            "unknown says nothing"
        );
        end.apn_apply = Some(apn_change(Phase::ReattachGrace));
        assert_eq!(
            invalid_reason(&start, Some(&end)),
            Some(InvalidReason::ApnChange)
        );
    }

    #[test]
    fn a_result_is_marked_when_the_ip_range_changed() {
        let cell = ServingCell {
            lte_band: Some(3),
            lte_pci: Some(103),
            ..Default::default()
        };
        let wan = |ip: Option<[u8; 4]>| {
            Some(router_core::WanView::new(
                ip.map(std::net::Ipv4Addr::from),
                false,
            ))
        };
        let mut start_snap = snap_with(&cell, None, Phase::Idle);
        start_snap.wan = wan(Some([10, 45, 1, 2]));
        let start = Baseline::from_snapshot(&start_snap);
        assert_eq!(start.ip_range.as_deref(), Some("10.45"));

        let mut end = snap_with(&cell, None, Phase::Idle);
        end.wan = wan(Some([10, 45, 9, 9]));
        assert_eq!(invalid_reason(&start, Some(&end)), None, "same range");
        end.wan = wan(Some([10, 62, 1, 2]));
        assert_eq!(
            invalid_reason(&start, Some(&end)),
            Some(InvalidReason::IpChanged)
        );
        end.wan = wan(None);
        assert_eq!(
            invalid_reason(&start, Some(&end)),
            Some(InvalidReason::IpChanged),
            "data went off"
        );
        end.wan = None;
        assert_eq!(
            invalid_reason(&start, Some(&end)),
            None,
            "unknown says nothing"
        );
        // A cell change still wins over an address change.
        let other = ServingCell {
            lte_pci: Some(9),
            ..cell.clone()
        };
        let mut moved = snap_with(&other, None, Phase::Idle);
        moved.wan = wan(Some([10, 62, 1, 2]));
        assert_eq!(
            invalid_reason(&start, Some(&moved)),
            Some(InvalidReason::ServingChanged)
        );
    }

    #[test]
    fn only_two_numbers_count_as_an_ip_range() {
        assert_eq!(clean_ip_range(Some("10.45")).as_deref(), Some("10.45"));
        assert_eq!(clean_ip_range(Some("100.70")).as_deref(), Some("100.70"));
        for bad in [
            "10.45.1.2",
            "10.45.1",
            "10",
            "",
            "10.256",
            "a.b",
            "10.45.",
            ".45",
            "0010.4",
        ] {
            assert_eq!(clean_ip_range(Some(bad)), None, "{bad}");
        }
        assert_eq!(clean_ip_range(None), None);
    }

    /// R5: the slot is freed and the actor told even when the test task panics.
    #[tokio::test]
    async fn a_panicking_test_task_still_frees_the_slot() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let st = SpeedTestState::default();
        let ended = Arc::new(AtomicU32::new(0));
        let e = ended.clone();
        let (guard, _rx) = st
            .try_begin(Box::new(move || {
                e.fetch_add(1, Ordering::SeqCst);
            }))
            .unwrap();
        assert!(st.is_running());
        assert!(
            st.try_begin(Box::new(|| {})).is_none(),
            "one test at a time"
        );
        let task = tokio::spawn(async move {
            let _guard = guard;
            panic!("boom");
        });
        assert!(task.await.is_err());
        assert!(!st.is_running());
        assert_eq!(ended.load(Ordering::SeqCst), 1);
        // The slot can be claimed again.
        let (g, _rx) = st.try_begin(Box::new(|| {})).unwrap();
        drop(g);
        assert!(!st.is_running());
    }

    #[test]
    fn ookla_falls_back_only_when_it_never_measured() {
        use ookla::OoklaError as E;
        assert_eq!(
            ookla_fallback(E::Install("hash".into())),
            Ok((FallbackReason::InstallFailed, Some("hash".into())))
        );
        assert_eq!(
            ookla_fallback(E::NotStarted("not found".into())),
            Ok((FallbackReason::InstallFailed, Some("not found".into())))
        );
        let (reason, msg) = ookla_fallback(E::Failed("ConfigurationError".into())).unwrap_err();
        assert_eq!(reason, FailReason::Ookla);
        assert!(msg.contains("ConfigurationError"));
        let (reason, msg) = ookla_fallback(E::TimedOut).unwrap_err();
        assert_eq!(reason, FailReason::Timeout);
        assert!(msg.contains("120 s"));
        let ookla = SpeedTestSettings {
            provider: SpeedTestProvider::Ookla,
            size: SpeedTestSize::Full,
            ookla_terms_accepted: true,
        };
        assert!(!size_applies(&ookla));
        assert!(size_applies(&SpeedTestSettings {
            ookla_terms_accepted: false,
            ..ookla
        }));
        assert!(size_applies(&SpeedTestSettings::default()));
    }

    #[test]
    fn events_serialize_with_a_kind_tag() {
        let e = SpeedTestEvent::Progress(SpeedTestProgress {
            phase: SpeedPhase::IdlePing,
            fraction: 0.5,
            bytes_used: 0,
            live: Live::default(),
        });
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["kind"], "progress");
        assert_eq!(v["phase"], "idlePing");
        assert_eq!(
            serde_json::to_value(SpeedTestEvent::Cancelled).unwrap()["kind"],
            "cancelled"
        );
    }

    async fn cf_server() -> MockServer {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/__down"))
            .respond_with(Down)
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/__up"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&s)
            .await;
        s
    }

    /// Ookla without accepted terms never downloads anything and measures with Cloudflare.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn ookla_without_terms_falls_back_without_downloading() {
        let s = cf_server().await;
        Mock::given(method("GET"))
            .and(path("/cli.zip"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&s)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let ep = Endpoints {
            cloudflare: s.uri(),
            ookla_zip: format!("{}/cli.zip", s.uri()),
            ..Endpoints::default()
        };
        let cfg = SpeedTestSettings {
            provider: SpeedTestProvider::Ookla,
            size: SpeedTestSize::Light,
            ookla_terms_accepted: false,
        };
        let (_tx, rx) = watch::channel(false);
        let out = run_test(cfg, ServingCell::default(), dir.path(), &ep, rx, &|_| {}).await;
        let Outcome::Done(r) = out else {
            panic!("expected a result")
        };
        assert_eq!(r.provider, SpeedTestProvider::Cloudflare);
        assert_eq!(r.requested, SpeedTestProvider::Ookla);
        assert_eq!(r.fallback, Some(FallbackReason::TermsNotAccepted));
        assert!(!ookla::exe_path(dir.path()).exists());
    }

    /// A download that fails its hash check falls back to Cloudflare and says so.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_bad_ookla_download_falls_back_to_cloudflare() {
        let s = cf_server().await;
        Mock::given(method("GET"))
            .and(path("/cli.zip"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"not a zip".to_vec()))
            .mount(&s)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let ep = Endpoints {
            cloudflare: s.uri(),
            ookla_zip: format!("{}/cli.zip", s.uri()),
            ..Endpoints::default()
        };
        let cfg = SpeedTestSettings {
            provider: SpeedTestProvider::Ookla,
            size: SpeedTestSize::Light,
            ookla_terms_accepted: true,
        };
        let (_tx, rx) = watch::channel(false);
        let phases = Mutex::new(Vec::new());
        let emit = |e: SpeedTestEvent| {
            if let SpeedTestEvent::Progress(p) = e {
                phases.lock().unwrap().push(p.phase);
            }
        };
        let Outcome::Done(r) =
            run_test(cfg, ServingCell::default(), dir.path(), &ep, rx, &emit).await
        else {
            panic!("expected a result")
        };
        assert_eq!(r.fallback, Some(FallbackReason::InstallFailed));
        assert_eq!(r.provider, SpeedTestProvider::Cloudflare);
        assert_eq!(r.size_applies, Some(true));
        assert!(r.download_mbps.is_some());
        assert!(phases.lock().unwrap().contains(&SpeedPhase::Installing));
    }

    /// R4: once the CLI has run, a failure is reported; no second (Cloudflare) test is spent.
    /// A copy of `cmd.exe` stands in for the CLI: it starts, prints no result and exits.
    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_ookla_run_that_fails_is_reported_not_retried() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/__down"))
            .respond_with(Down)
            .expect(0)
            .mount(&s)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let cmd = std::fs::read(Path::new(&root).join("System32").join("cmd.exe")).unwrap();
        let exe = ookla::exe_path(dir.path());
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, &cmd).unwrap();
        let ep = Endpoints {
            cloudflare: s.uri(),
            ookla_exe_sha: ookla::sha256_hex(&cmd),
            ..Endpoints::default()
        };
        let cfg = SpeedTestSettings {
            provider: SpeedTestProvider::Ookla,
            size: SpeedTestSize::Light,
            ookla_terms_accepted: true,
        };
        let (_tx, rx) = watch::channel(false);
        let out = run_test(cfg, ServingCell::default(), dir.path(), &ep, rx, &|_| {}).await;
        let Outcome::Failed(reason, message) = out else {
            panic!("expected a reported failure, not a fallback")
        };
        assert_eq!(reason, FailReason::Ookla);
        assert!(message.starts_with("Ookla Speedtest"), "{message}");
        assert!(load_history(dir.path()).is_empty());
    }
}
