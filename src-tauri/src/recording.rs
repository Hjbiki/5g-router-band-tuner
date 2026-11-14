//! The recording flow around the window: start it, watch it, stop it, look at exactly what would
//! be saved, then save it or throw it away. `capture_window` is the window, and
//! `router_core::capture` is the cleaning.
//!
//! Nothing is ever sent anywhere. What was recorded lives in memory only, until the user saves the
//! cleaned file themselves or it is thrown away: by Discard, by closing the window, or by closing
//! the app.
//!
//! A recording is refused while the app itself is talking to the router. The app's own session,
//! its polling and its band watchdog would mix into what is recorded, and the watchdog could undo
//! what the user changes by hand on the router's page. So on a supported router the user logs out
//! of the app first; on an unsupported one the app has no session to begin with.

use std::sync::{Arc, Mutex, MutexGuard};

use router_core::capture::{self, BuildOptions, Exchange, Limit};
use router_core::SessionView;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, Url};
use ts_rs::TS;

use crate::capture_window::{self, Recording};
use crate::commands::{AppState, CmdError};

/// Sent to the main window whenever the flow moves on.
pub const EVENT: &str = "capture-status";
/// How much of the file the main window is given to show. The file itself is saved whole.
const PREVIEW_BYTES: usize = 256 * 1024;
const FILE_NAME: &str = "router-band-tuner-recording.json";
const TITLE_MAX: usize = 80;

struct Stopped {
    exchanges: Arc<Vec<Exchange>>,
    limit: Option<Limit>,
    /// The cleaned file as last built, which is exactly what Save writes.
    built: Option<String>,
}

#[derive(Default)]
enum Flow {
    #[default]
    Idle,
    Open(Recording),
    Stopped(Stopped),
}

#[derive(Default)]
pub struct CaptureState {
    flow: Mutex<Flow>,
}

impl CaptureState {
    fn lock(&self) -> MutexGuard<'_, Flow> {
        self.flow
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Nothing is open, and nothing is waiting to be saved or thrown away.
    pub fn is_idle(&self) -> bool {
        matches!(*self.lock(), Flow::Idle)
    }

    fn status(&self) -> CaptureStatus {
        match &*self.lock() {
            Flow::Idle => CaptureStatus {
                phase: CapturePhase::Idle,
                requests: 0,
                limit: None,
            },
            Flow::Open(recording) => {
                let (requests, limit) = recording.progress();
                CaptureStatus {
                    phase: CapturePhase::Recording,
                    requests: u32::try_from(requests).unwrap_or(u32::MAX),
                    limit,
                }
            }
            Flow::Stopped(s) => CaptureStatus {
                phase: CapturePhase::Stopped,
                requests: u32::try_from(s.exchanges.len()).unwrap_or(u32::MAX),
                limit: s.limit,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CapturePhase {
    Idle,
    Recording,
    /// Stopped, with what was recorded waiting to be saved or thrown away.
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CaptureStatus {
    pub phase: CapturePhase,
    pub requests: u32,
    /// Why the recording stopped taking more, when it did.
    pub limit: Option<Limit>,
}

/// Why a recording can't start now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CaptureRefusal {
    /// The app is logged in to the router, or trying to be. Log out of the app first.
    LoggedIn,
    /// A band change, a scan, a speed test or a page-script save is running.
    Busy,
    /// The demo router has no page to record.
    Demo,
    /// A recording is open, or one is waiting to be saved or thrown away.
    AlreadyOpen,
    /// The router's address in the settings isn't one a browser can open.
    BadAddress,
}

/// What the user is shown before anything is saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CapturePreview {
    /// The start of the cleaned file, exactly as it would be saved. Text, never markup.
    pub text: String,
    /// The whole file's size.
    pub bytes: u32,
    /// `text` is only the file's start.
    pub truncated: bool,
    pub requests: u32,
    pub code_included: bool,
    pub limit: Option<Limit>,
    /// What the last check found in the cleaned file. Not empty means nothing can be saved.
    pub problems: Vec<String>,
}

/// Why a recording can't start now, if it can't. The app must not be talking to the router
/// itself while the router's page is recorded.
pub fn refusal(
    session: &SessionView,
    demo: bool,
    busy: bool,
    open: bool,
) -> Option<CaptureRefusal> {
    if open {
        return Some(CaptureRefusal::AlreadyOpen);
    }
    if demo {
        return Some(CaptureRefusal::Demo);
    }
    if busy {
        return Some(CaptureRefusal::Busy);
    }
    let talking = matches!(
        session,
        SessionView::LoggedIn | SessionView::Connecting | SessionView::Reconnecting { .. }
    );
    talking.then_some(CaptureRefusal::LoggedIn)
}

/// Whether a recording is open in this app, or one is waiting for its review.
pub fn is_open(app: &AppHandle) -> bool {
    app.try_state::<CaptureState>()
        .is_some_and(|cap| !cap.is_idle())
}

/// The other half of "the app isn't talking to the router while its page is recorded": the check
/// at the start of a recording is no use if the app may log in a minute later. Used by Connect.
pub fn refuse_while_open(app: &AppHandle) -> Result<(), CmdError> {
    if is_open(app) {
        return Err(CmdError::Message(
            "A recording of the router's page is open. Stop or discard it first.".into(),
        ));
    }
    Ok(())
}

/// The start of `text`, at most `max` bytes, never cut inside a character.
fn head(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn emit(app: &AppHandle, cap: &CaptureState) {
    let _ = app.emit(EVENT, cap.status());
}

/// Cleans what was recorded, off the window's thread, and keeps the result for Save.
async fn preview(
    app: &AppHandle,
    cap: &CaptureState,
    include_code: bool,
) -> Result<CapturePreview, CmdError> {
    let (exchanges, limit) = match &*cap.lock() {
        Flow::Stopped(s) => (s.exchanges.clone(), s.limit),
        _ => return Err(CmdError::Message("no recording is waiting".into())),
    };
    let version = app.package_info().version.to_string();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let requests = u32::try_from(exchanges.len()).unwrap_or(u32::MAX);
    let built = tokio::task::spawn_blocking(move || {
        let opts = BuildOptions {
            app_version: &version,
            generated_at_ms: now,
            include_code,
            limit,
        };
        capture::build(&exchanges, &opts)
    })
    .await
    .map_err(|e| CmdError::Message(e.to_string()))?;

    let (text, problems) = match built {
        Ok(text) => (Some(text), Vec::new()),
        Err(found) => (None, found),
    };
    let shown = text
        .as_deref()
        .map(|t| head(t, PREVIEW_BYTES).to_string())
        .unwrap_or_default();
    let bytes = text.as_ref().map_or(0, String::len);
    // The flow may have moved on while this ran (Discard): only a waiting recording keeps it.
    if let Flow::Stopped(s) = &mut *cap.lock() {
        s.built = text;
    }
    Ok(CapturePreview {
        truncated: shown.len() < bytes,
        text: shown,
        bytes: u32::try_from(bytes).unwrap_or(u32::MAX),
        requests,
        code_included: include_code,
        limit,
        problems,
    })
}

/// Opens the router's own page in the recording window. `title` is the window's title, in the
/// user's language. `Ok(Some(why))` when it can't start now.
#[tauri::command]
pub async fn capture_start(
    app: AppHandle,
    title: String,
    state: State<'_, AppState>,
    cap: State<'_, CaptureState>,
    scan: State<'_, crate::autoscan::ScanState>,
    st: State<'_, crate::speedtest::SpeedTestState>,
) -> Result<Option<CaptureRefusal>, CmdError> {
    let (host, driver, _) = state.scan_identity();
    let snapshot = state.snapshot();
    let busy = crate::speedtest::band_change_open(&snapshot.safe_apply)
        || scan.is_running()
        || st.is_running()
        || state.page_scripts_running();
    let open = !matches!(*cap.lock(), Flow::Idle);
    if let Some(why) = refusal(&snapshot.session, driver == "mock", busy, open) {
        return Ok(Some(why));
    }
    let Ok(router) = Url::parse(&format!("http://{host}/")) else {
        return Ok(Some(CaptureRefusal::BadAddress));
    };
    let title: String = title
        .chars()
        .filter(|c| !c.is_control())
        .take(TITLE_MAX)
        .collect();
    let recording = capture_window::open(&app, &router, state.data_dir(), &title, true)
        .map_err(CmdError::Message)?;
    *cap.lock() = Flow::Open(recording);
    emit(&app, &cap);
    Ok(None)
}

#[tauri::command]
pub fn capture_status(cap: State<'_, CaptureState>) -> CaptureStatus {
    cap.status()
}

/// Ends the recording and shows what would be saved. Nothing is written yet.
#[tauri::command]
pub async fn capture_stop(
    app: AppHandle,
    cap: State<'_, CaptureState>,
) -> Result<CapturePreview, CmdError> {
    let recording = {
        let mut flow = cap.lock();
        match std::mem::take(&mut *flow) {
            Flow::Open(recording) => recording,
            other => {
                *flow = other;
                return Err(CmdError::Message("no recording is open".into()));
            }
        }
    };
    let (exchanges, limit) = recording.finish(&app);
    *cap.lock() = Flow::Stopped(Stopped {
        exchanges: Arc::new(exchanges),
        limit,
        built: None,
    });
    emit(&app, &cap);
    preview(&app, &cap, false).await
}

/// Builds the file again, with or without the router's own scripts. The user's choice: they are
/// the router maker's code, and the file is meant for a public issue.
#[tauri::command]
pub async fn capture_preview(
    app: AppHandle,
    include_code: bool,
    cap: State<'_, CaptureState>,
) -> Result<CapturePreview, CmdError> {
    preview(&app, &cap, include_code).await
}

/// Saves the cleaned file where the user says, exactly as it was last shown. `Ok(false)` when
/// they closed the dialog. After a save nothing of the recording is kept.
#[tauri::command]
pub async fn capture_save(app: AppHandle, cap: State<'_, CaptureState>) -> Result<bool, CmdError> {
    use tauri_plugin_dialog::DialogExt;
    let Some(text) = (match &*cap.lock() {
        Flow::Stopped(s) => s.built.clone(),
        _ => None,
    }) else {
        return Err(CmdError::Message("there is no cleaned file to save".into()));
    };
    let dialog = app.clone();
    let saved = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        let Some(target) = dialog
            .dialog()
            .file()
            .set_file_name(FILE_NAME)
            .add_filter("JSON", &["json"])
            .blocking_save_file()
        else {
            return Ok(false);
        };
        let path = target.into_path().map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| e.to_string())?;
        Ok(true)
    })
    .await
    .map_err(|e| CmdError::Message(e.to_string()))?
    .map_err(CmdError::Message)?;
    if saved {
        *cap.lock() = Flow::Idle;
        emit(&app, &cap);
    }
    Ok(saved)
}

/// Throws the recording away, open or stopped.
#[tauri::command]
pub fn capture_discard(app: AppHandle, cap: State<'_, CaptureState>) {
    let taken = std::mem::take(&mut *cap.lock());
    if let Flow::Open(recording) = taken {
        drop(recording.finish(&app));
    }
    emit(&app, &cap);
}

/// The recording window was closed. If a recording was open in it, closing it by hand throws
/// the recording away: nothing may outlive the window without the user asking for it.
pub fn window_closed(app: &AppHandle) {
    let Some(cap) = app.try_state::<CaptureState>() else {
        return;
    };
    let taken = {
        let mut flow = cap.lock();
        if !matches!(*flow, Flow::Open(_)) {
            return;
        }
        std::mem::take(&mut *flow)
    };
    if let Flow::Open(recording) = taken {
        drop(recording.finish(app));
    }
    emit(app, &cap);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recording_starts_only_while_the_app_is_not_talking_to_the_router() {
        let idle = [
            SessionView::NotConfigured,
            SessionView::TakenOver,
            SessionView::BadCredentials,
            SessionView::AlgorithmMismatch,
            SessionView::LockedOut { wait_s: 30 },
            SessionView::LoginRejected { code: 7 },
        ];
        for session in &idle {
            assert_eq!(refusal(session, false, false, false), None, "{session:?}");
        }
        let talking = [
            SessionView::LoggedIn,
            SessionView::Connecting,
            SessionView::Reconnecting {
                reason: "timeout".into(),
                retry_in_s: 5,
            },
        ];
        for session in &talking {
            assert_eq!(
                refusal(session, false, false, false),
                Some(CaptureRefusal::LoggedIn),
                "{session:?}"
            );
        }
    }

    #[test]
    fn the_first_reason_that_applies_is_the_one_given() {
        let out = SessionView::NotConfigured;
        assert_eq!(
            refusal(&out, false, false, true),
            Some(CaptureRefusal::AlreadyOpen)
        );
        assert_eq!(
            refusal(&out, true, true, true),
            Some(CaptureRefusal::AlreadyOpen)
        );
        assert_eq!(refusal(&out, true, true, false), Some(CaptureRefusal::Demo));
        assert_eq!(
            refusal(&out, false, true, false),
            Some(CaptureRefusal::Busy)
        );
        assert_eq!(
            refusal(&SessionView::LoggedIn, false, true, false),
            Some(CaptureRefusal::Busy)
        );
    }

    #[test]
    fn the_preview_is_the_files_start_and_never_half_a_character() {
        assert_eq!(head("abc", 10), "abc");
        assert_eq!(head("abcdef", 4), "abcd");
        // "ع" is two bytes: a cut in its middle moves back to before it.
        assert_eq!(head("abع", 3), "ab");
        assert_eq!(head("عع", 2), "ع");
        assert_eq!(head("", 0), "");
    }

    #[test]
    fn nothing_is_open_at_first_and_nothing_can_be_saved() {
        let cap = CaptureState::default();
        assert_eq!(
            cap.status(),
            CaptureStatus {
                phase: CapturePhase::Idle,
                requests: 0,
                limit: None
            }
        );
        assert!(matches!(*cap.lock(), Flow::Idle));
        assert!(cap.is_idle());
    }

    #[test]
    fn a_recording_waiting_for_its_review_still_counts_as_open() {
        // Until it is saved or thrown away the app stays away from the router: Connect asks this.
        let cap = CaptureState::default();
        *cap.lock() = Flow::Stopped(Stopped {
            exchanges: Arc::new(Vec::new()),
            limit: None,
            built: None,
        });
        assert!(!cap.is_idle());
        assert_eq!(cap.status().phase, CapturePhase::Stopped);
        *cap.lock() = Flow::Idle;
        assert!(cap.is_idle());
    }
}
