//! Noticing a router the app doesn't support, for any user, with no login. See `router_core::probe`.
//!
//! Skipped when the demo driver is on, and when the app is already logged in to a supported model,
//! so the router it knows gets no extra requests. Runs again when the router address changes.

use std::sync::Mutex;
use std::time::Duration;

use router_core::probe::{self, ProbeView, RouterFacts, Support};
use router_core::SessionView;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::{AppState, CmdError};

pub const EVENT: &str = "router-probe";
const NEW_ISSUE: &str = "https://github.com/Hjbiki/5g-router-band-tuner/issues/new";
const FIRST_LOOK_AFTER: Duration = Duration::from_secs(12);
const TICK: Duration = Duration::from_secs(30);
/// How soon to look again when nothing answered.
const UNREACHABLE_RETRY_TICKS: u32 = 10;

#[derive(Default)]
pub struct ProbeState {
    latest: Mutex<Option<RouterFacts>>,
}

impl ProbeState {
    fn facts(&self) -> Option<RouterFacts> {
        self.latest
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn set(&self, f: Option<RouterFacts>) {
        *self.latest.lock().unwrap_or_else(|p| p.into_inner()) = f;
    }
}

/// `Some(address)` when a probe could tell something new about the router in the settings.
fn target(app: &AppHandle) -> Option<String> {
    let state = app.try_state::<AppState>()?;
    let (host, driver, _) = state.scan_identity();
    if driver == "mock" {
        return None;
    }
    // While the router's own page is being recorded, the app asks the router for nothing.
    if crate::recording::is_open(app) {
        return None;
    }
    let snap = state.snapshot();
    let logged_in_to_known = snap.session == SessionView::LoggedIn
        && snap
            .device
            .as_ref()
            .is_some_and(|d| probe::is_supported_model(&d.model));
    let settled = !crate::speedtest::band_change_open(&snap.safe_apply)
        && !app
            .try_state::<crate::autoscan::ScanState>()
            .is_some_and(|s| s.is_running());
    (!logged_in_to_known && settled).then_some(host)
}

async fn look(app: &AppHandle, host: &str) {
    let facts = probe::probe(&format!("http://{host}")).await;
    if let Some(p) = app.try_state::<ProbeState>() {
        let _ = app.emit(EVENT, facts.view());
        p.set(Some(facts));
    }
}

fn clear(app: &AppHandle) {
    if let Some(p) = app.try_state::<ProbeState>() {
        if p.facts().is_some() {
            p.set(None);
            let _ = app.emit(EVENT, Option::<ProbeView>::None);
        }
    }
}

/// Looks once per router address, and again now and then while nothing answers.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_LOOK_AFTER).await;
        let mut looked_at: Option<String> = None;
        let mut idle_ticks = 0u32;
        loop {
            let unreachable = app
                .try_state::<ProbeState>()
                .and_then(|p| p.facts())
                .is_some_and(|f| f.support() == Support::Unreachable);
            match target(&app) {
                Some(host) => {
                    let due = looked_at.as_deref() != Some(host.as_str())
                        || (unreachable && idle_ticks >= UNREACHABLE_RETRY_TICKS);
                    if due {
                        look(&app, &host).await;
                        looked_at = Some(host);
                        idle_ticks = 0;
                    } else {
                        idle_ticks += 1;
                    }
                }
                None => {
                    // Logged in to a known model, or the demo: there's nothing to report.
                    let state = app.try_state::<AppState>();
                    let snap = state.as_ref().map(|s| s.snapshot());
                    let change_open = snap
                        .as_ref()
                        .is_some_and(|s| crate::speedtest::band_change_open(&s.safe_apply));
                    if !change_open {
                        clear(&app);
                        looked_at = None;
                    }
                }
            }
            tokio::time::sleep(TICK).await;
        }
    });
}

#[tauri::command]
pub fn router_probe_status(p: State<'_, ProbeState>) -> Option<ProbeView> {
    p.facts().map(|f| f.view())
}

/// The redacted report. It waits in the same place as Export diagnostics, to be saved by the same
/// command after the user has read it.
#[tauri::command]
pub fn prepare_router_report(
    app: AppHandle,
    state: State<'_, AppState>,
    p: State<'_, ProbeState>,
) -> Result<String, CmdError> {
    let Some(facts) = p.facts() else {
        return Err(CmdError::Message("the router hasn't been read yet".into()));
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let text =
        probe::report(&facts, &app.package_info().version.to_string(), now).map_err(|leaks| {
            CmdError::Message(format!(
                "Export stopped: the privacy check found {}. Nothing was saved.",
                leaks.join(", ")
            ))
        })?;
    state.set_prepared_diagnostics(text.clone());
    Ok(text)
}

/// Opens the New router form in the browser, with what's known filled in.
#[tauri::command]
pub fn open_router_issue(app: AppHandle, p: State<'_, ProbeState>) -> Result<(), CmdError> {
    use tauri_plugin_opener::OpenerExt;
    let view = p.facts().map(|f| f.view()).unwrap_or(ProbeView {
        support: Support::Unknown,
        vendor: None,
        model: None,
        hardware: None,
        firmware: None,
    });
    app.opener()
        .open_url(probe::issue_url(&view, NEW_ISSUE), None::<&str>)
        .map_err(|e| CmdError::Message(e.to_string()))
}
