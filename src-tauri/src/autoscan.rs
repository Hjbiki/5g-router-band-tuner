//! The app side of the auto-scan: commands, the `scan` event and the speed test for each config.
//! The scan itself is `router_core::scan`. While one runs, user band changes, speed tests and
//! router or driver changes are refused here, and reboot and factory reset in the actor.
//!
//! At startup a `scan.json` left in `Running` or `Returning` means the app or PC went away
//! mid-scan: the journal has already taken care of the open change, and this goes back to the
//! starting bands. Nothing is measured until the user presses Continue.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use router_core::scan::{self, ScanFile, ScanHost, ScanMode, ScanSpeed, ScanStatus};
use router_core::{Command, LteBandSet, SessionView};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::watch;

use crate::commands::{AppState, CmdError};
use crate::speedtest::{self, SpeedTestState};

pub const EVENT: &str = "scan";
pub const ERR_RUNNING: &str = "an auto-scan is running";
pub const ERR_BUSY: &str = "Finish or revert the current band change first.";
const ERR_NOT_READY: &str = "not connected to the router";
const ERR_NO_BANDS: &str = "pick at least one band to scan";
const ERR_SPEED_TEST: &str = "a speed test is already running";
const ERR_OTHER_ROUTER: &str = "this scan was for another router";
/// Tries to get back after a restart; each waits up to 5 minutes for the router.
const RETURN_TRIES_AFTER_RESTART: u32 = 8;
const SEE_ROUTER_WITHIN: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct ScanState {
    /// The stop switch of the running scan.
    stop: Mutex<Option<watch::Sender<bool>>>,
    view: Mutex<Option<ScanFile>>,
}

impl ScanState {
    pub fn is_running(&self) -> bool {
        self.stop
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
    }

    fn view(&self) -> Option<ScanFile> {
        self.view.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    fn set_view(&self, f: Option<ScanFile>) {
        *self.view.lock().unwrap_or_else(|p| p.into_inner()) = f;
    }

    /// Claims the single scan slot.
    fn begin(&self) -> Option<watch::Receiver<bool>> {
        let mut g = self.stop.lock().unwrap_or_else(|p| p.into_inner());
        if g.is_some() {
            return None;
        }
        let (tx, rx) = watch::channel(false);
        *g = Some(tx);
        Some(rx)
    }

    fn end(&self) {
        *self.stop.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }
}

/// Frees the slot and the actor's scan flag however the scan task ends, a panic included.
struct RunGuard(AppHandle);

impl Drop for RunGuard {
    fn drop(&mut self) {
        if let Some(s) = self.0.try_state::<ScanState>() {
            s.end();
        }
        if let Some(s) = self.0.try_state::<AppState>() {
            s.send_now(Command::SetScanActive(false));
        }
    }
}

fn publish(app: &AppHandle, dir: &Path, file: &ScanFile) {
    let _ = scan::save_file(dir, file);
    if let Some(s) = app.try_state::<ScanState>() {
        s.set_view(Some(file.clone()));
    }
    let _ = app.emit(EVENT, file);
}

struct Host {
    app: AppHandle,
    dir: PathBuf,
}

#[async_trait]
impl ScanHost for Host {
    async fn measure(&self) -> Result<ScanSpeed, String> {
        let r = speedtest::measure_now(&self.app).await?;
        Ok(ScanSpeed {
            download_mbps: r.download_mbps,
            upload_mbps: r.upload_mbps,
            idle_ping_ms: r.idle_ping_ms,
            ping_down_ms: r.ping_down_ms,
            ping_up_ms: r.ping_up_ms,
            bytes_used: r.bytes_used,
        })
    }

    fn publish(&self, file: &ScanFile) {
        publish(&self.app, &self.dir, file);
    }

    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// A band change, a Refresh IP run or an APN switch holds the router.
fn snapshot_refusal(snap: &router_core::AppSnapshot) -> Option<&'static str> {
    if speedtest::band_change_open(&snap.safe_apply) {
        Some(ERR_BUSY)
    } else if speedtest::ip_refresh_active(snap) {
        Some(router_core::IP_REFRESH_RUNNING)
    } else if speedtest::apn_change_open(snap) {
        Some(router_core::APN_CHANGE_RUNNING)
    } else {
        None
    }
}

/// Everything that must hold before a scan may touch the router.
fn check_ready(state: &AppState, st: &SpeedTestState) -> Result<(), CmdError> {
    let snap = state.snapshot();
    if let Some(why) = snapshot_refusal(&snap) {
        return Err(CmdError::Message(why.into()));
    }
    if st.is_running() {
        return Err(CmdError::Message(ERR_SPEED_TEST.into()));
    }
    if state.page_scripts_reading() {
        return Err(CmdError::Message(router_core::PAGE_SCRIPTS_RUNNING.into()));
    }
    if snap.session != SessionView::LoggedIn || snap.config.is_none() || snap.capabilities.is_none()
    {
        return Err(CmdError::Message(ERR_NOT_READY.into()));
    }
    Ok(())
}

fn spawn_run(app: AppHandle, mut file: ScanFile, stop: watch::Receiver<bool>, grace: Duration) {
    tauri::async_runtime::spawn(async move {
        let guard = RunGuard(app.clone());
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        let dir = state.data_dir().to_path_buf();
        let mut ctl = state.change_ctl();
        let host = Host {
            app: app.clone(),
            dir,
        };
        scan::run(&mut ctl, &mut file, grace, &host, stop).await;
        drop(guard);
    });
}

/// The record of the running or finished scan, if any.
#[tauri::command]
pub fn get_scan(scan: State<'_, ScanState>) -> Option<ScanFile> {
    scan.view()
}

/// `combos` are band-number lists the user asked to test together, after the single bands. It's
/// optional so an older caller still works.
// A Tauri command takes its states and arguments one by one.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub fn start_scan(
    app: AppHandle,
    state: State<'_, AppState>,
    st: State<'_, SpeedTestState>,
    scan: State<'_, ScanState>,
    mode: ScanMode,
    bands: Vec<u8>,
    repeats: u32,
    combos: Option<Vec<Vec<u8>>>,
) -> Result<(), CmdError> {
    check_ready(&state, &st)?;
    let snap = state.snapshot();
    let (Some(start), Some(caps)) = (snap.config, snap.capabilities) else {
        return Err(CmdError::Message(ERR_NOT_READY.into()));
    };
    let bands: Vec<u8> = bands
        .into_iter()
        .filter(|b| caps.supported_lte.contains(*b))
        .collect();
    // Combinations the user asked for, as band numbers so the UI needs no hex.
    let mut sets: Vec<LteBandSet> = Vec::new();
    for combo in combos.unwrap_or_default() {
        let Ok(set) = LteBandSet::from_bands(&combo) else {
            continue;
        };
        if scan::valid_custom(set, caps.supported_lte) && !sets.contains(&set) {
            sets.push(set);
        }
    }
    // A downlink-only band needs another band to add to.
    if bands.iter().all(|b| scan::downlink_only(*b)) && sets.is_empty() {
        return Err(CmdError::Message(ERR_NO_BANDS.into()));
    }
    let Some(stop) = scan.begin() else {
        return Err(CmdError::Message(ERR_RUNNING.into()));
    };
    let (host, driver, grace) = state.scan_identity();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mut file = ScanFile::new(&host, &driver, mode, start, &bands, now);
    file.add_custom(&sets);
    file.repeats = repeats.clamp(1, scan::MAX_REPEATS);
    file.model = snap.device.map(|d| d.model);
    publish(&app, state.data_dir(), &file);
    spawn_run(app.clone(), file, stop, grace);
    Ok(())
}

/// Runs the configs a stopped or interrupted scan didn't reach.
#[tauri::command]
pub fn continue_scan(
    app: AppHandle,
    state: State<'_, AppState>,
    st: State<'_, SpeedTestState>,
    scan: State<'_, ScanState>,
) -> Result<(), CmdError> {
    check_ready(&state, &st)?;
    let Some(mut file) = scan.view() else {
        return Err(CmdError::Message(ERR_NO_BANDS.into()));
    };
    let (host, driver, grace) = state.scan_identity();
    let model = state.snapshot().device.map(|d| d.model);
    let other_model = file.model.is_some() && model.is_some() && file.model != model;
    if file.host != host || file.driver != driver || other_model {
        return Err(CmdError::Message(ERR_OTHER_ROUTER.into()));
    }
    let Some(stop) = scan.begin() else {
        return Err(CmdError::Message(ERR_RUNNING.into()));
    };
    // The user may have changed bands since it stopped: those are what it returns to now.
    if let Some(current) = state.snapshot().config {
        for r in &mut file.results {
            r.is_start = r.bands == current.lte_bands;
        }
        file.start = current;
    }
    spawn_run(app.clone(), file, stop, grace);
    Ok(())
}

/// Stop = cancel what's running and go back to the starting bands. The results stay.
#[tauri::command]
pub fn stop_scan(st: State<'_, SpeedTestState>, scan: State<'_, ScanState>) {
    if let Some(tx) = scan.stop.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
        let _ = tx.send(true);
    }
    st.cancel();
}

/// Forgets the results. The speed tests and band history they added stay.
#[tauri::command]
pub fn dismiss_scan(
    app: AppHandle,
    state: State<'_, AppState>,
    scan: State<'_, ScanState>,
) -> Result<(), CmdError> {
    if scan.is_running() {
        return Err(CmdError::Message(ERR_RUNNING.into()));
    }
    scan::remove_file(state.data_dir()).map_err(|e| CmdError::Message(e.to_string()))?;
    scan.set_view(None);
    let _ = app.emit(EVENT, Option::<ScanFile>::None);
    Ok(())
}

/// Loads the last scan's results; one that was cut off goes back to the starting bands.
pub fn on_start(app: &AppHandle) {
    let (Some(state), Some(scan_state)) =
        (app.try_state::<AppState>(), app.try_state::<ScanState>())
    else {
        return;
    };
    let Some(mut file) = scan::load_file(state.data_dir()) else {
        return;
    };
    let (host, driver, grace) = state.scan_identity();
    let same_router = file.host == host && file.driver == driver;
    if !file.was_interrupted() || !same_router {
        if file.was_interrupted() {
            file.status = ScanStatus::Interrupted;
            let _ = scan::save_file(state.data_dir(), &file);
        }
        scan_state.set_view(Some(file));
        return;
    }
    let Some(stop) = scan_state.begin() else {
        return;
    };
    let dir = state.data_dir().to_path_buf();
    let app = app.clone();
    scan_state.set_view(Some(file.clone()));
    tauri::async_runtime::spawn(async move {
        let guard = RunGuard(app.clone());
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        let mut ctl = state.change_ctl();
        ctl.send(Command::SetScanActive(true)).await;
        file.status = ScanStatus::Returning;
        publish(&app, &dir, &file);
        let mut pressed = scan::stop_pressed_again(&stop);
        // Another router at this address never had these bands.
        let connected = ctl
            .wait_or_stop(SEE_ROUTER_WITHIN, &mut pressed, |s| {
                s.session == SessionView::LoggedIn && s.device.is_some()
            })
            .await;
        let seen = connected.and_then(|s| s.device).map(|d| d.model);
        let same_model = file.model.is_none() || seen.is_none() || file.model == seen;
        let start = file.start.clone();
        file.return_failed = same_model
            && !scan::return_to_start(
                &mut ctl,
                &start,
                grace + Duration::from_secs(120),
                RETURN_TRIES_AFTER_RESTART,
                &mut pressed,
            )
            .await;
        file.status = ScanStatus::Interrupted;
        publish(&app, &dir, &file);
        drop(guard);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scan_waits_for_a_band_change_or_a_refresh_ip_run() {
        let mut s = router_core::AppSnapshot {
            ip_refresh: Some(speedtest::running_ip_refresh()),
            ..serde_json::from_value(serde_json::json!({
                "session": {"kind": "loggedIn"},
                "device": null, "status": null, "signal": null, "config": null,
                "capabilities": null,
                "safeApply": {"phase": {"kind": "idle"}, "secondsLeft": null,
                              "previous": null, "requested": null},
                "inService": true, "foreignJournal": null, "capturedAtMs": 0,
            }))
            .unwrap()
        };
        assert_eq!(snapshot_refusal(&s), Some(router_core::IP_REFRESH_RUNNING));
        s.ip_refresh = None;
        assert_eq!(snapshot_refusal(&s), None);
        s.apn_apply = Some(speedtest::apn_change(router_core::Phase::Probation));
        assert_eq!(snapshot_refusal(&s), Some(router_core::APN_CHANGE_RUNNING));
        s.apn_apply = None;
        s.safe_apply.phase = router_core::Phase::Probation;
        assert_eq!(snapshot_refusal(&s), Some(ERR_BUSY));
    }
}
