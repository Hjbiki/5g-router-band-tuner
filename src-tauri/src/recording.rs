//! The recording flow around the window: start it, watch it, stop it, look at exactly what would
//! be saved, then save it or throw it away. `capture_window` is the window, and
//! `router_core::capture` is the cleaning.
//!
//! Nothing is ever sent anywhere. What was recorded lives in memory only, until the user saves the
//! cleaned file themselves or it is thrown away: by Discard, or by closing the app.
//!
//! A recording ends from either window: Stop in the main window, or the recording window's own
//! Stop item or its X. Each leads to the review. Closing the window is how most people say
//! "done", so it must not cost them what they recorded.
//!
//! A recording is refused while the app itself is talking to the router. The app's own session,
//! its polling and its band watchdog would mix into what is recorded, and the watchdog could undo
//! what the user changes by hand on the router's page. So on a supported router the user logs out
//! of the app first; on an unsupported one the app has no session to begin with.

use std::sync::{Arc, Mutex, MutexGuard};

use router_core::capture::{self, BuildOptions, Exchange, Limit};
use router_core::SessionView;
use serde::{Deserialize, Serialize};
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
/// How many of the file's readable values the main window is given, and how much of each.
const READABLE_MAX: usize = 300;
const READABLE_BYTES: usize = 120;
/// What the window's Stop item says when the main window gave it no text.
const STOP_TEXT: &str = "Stop";
/// The most the box of steps on the recording window may hold.
const STEPS_MAX: usize = 400;

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

    /// Keeps a cleaned file, but only for the recording it was built from: cleaning takes
    /// seconds, and by then that one may be gone and another waiting.
    fn keep_built(&self, of: &Arc<Vec<Exchange>>, text: Option<String>) -> bool {
        match &mut *self.lock() {
            Flow::Stopped(s) if Arc::ptr_eq(&s.exchanges, of) => {
                s.built = text;
                true
            }
            _ => false,
        }
    }

    /// Forgets the recording whose file was just saved, and only that one: the save dialog
    /// stays open as long as the person likes.
    fn forget_saved(&self, of: &Arc<Vec<Exchange>>) -> bool {
        let mut flow = self.lock();
        let same = matches!(&*flow, Flow::Stopped(s) if Arc::ptr_eq(&s.exchanges, of));
        if same {
            *flow = Flow::Idle;
        }
        same
    }

    /// The window of an open recording is gone: `finish` ends it, and what it hands over waits
    /// for the review. False when no recording was open (Stop or Discard already took it).
    fn keep_closed(
        &self,
        finish: impl FnOnce(Recording) -> (Vec<Exchange>, Option<Limit>),
    ) -> bool {
        let recording = {
            let mut flow = self.lock();
            match std::mem::take(&mut *flow) {
                Flow::Open(recording) => recording,
                other => {
                    *flow = other;
                    return false;
                }
            }
        };
        let (exchanges, limit) = finish(recording);
        *self.lock() = Flow::Stopped(Stopped {
            exchanges: Arc::new(exchanges),
            limit,
            built: None,
        });
        true
    }

    fn status(&self) -> CaptureStatus {
        match &*self.lock() {
            Flow::Idle => CaptureStatus {
                phase: CapturePhase::Idle,
                requests: 0,
                limit: None,
                enough: false,
            },
            Flow::Open(recording) => {
                let now = recording.progress();
                CaptureStatus {
                    phase: CapturePhase::Recording,
                    requests: u32::try_from(now.requests).unwrap_or(u32::MAX),
                    limit: now.limit,
                    enough: now.enough,
                }
            }
            Flow::Stopped(s) => CaptureStatus {
                phase: CapturePhase::Stopped,
                requests: u32::try_from(s.exchanges.len()).unwrap_or(u32::MAX),
                limit: s.limit,
                enough: false,
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
    /// The router's page has logged in and finished loading: the window has told the person
    /// they can close it.
    pub enough: bool,
}

/// What the recording window says, from the main window, in the user's language.
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CaptureTexts {
    /// The window's title while it records.
    pub title: String,
    /// Its Stop item while it records.
    pub stop: String,
    /// The title once enough is recorded.
    pub done_title: String,
    /// The Stop item once enough is recorded.
    pub done_stop: String,
    /// The steps, shown in a box on the window when it opens.
    pub steps: String,
    /// The language reads right to left, so the box is laid out that way.
    pub rtl: bool,
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
    /// The values still readable in the file, as the router's page had them: what a person
    /// checks instead of reading the file. The first of them when there are very many.
    pub readable: Vec<String>,
    /// How many readable values the file has in all.
    pub readable_total: u32,
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

/// The readable values for the screen: the first of them, each cut to a line, and their count.
fn listed(readable: Vec<String>) -> (Vec<String>, u32) {
    let total = u32::try_from(readable.len()).unwrap_or(u32::MAX);
    let shown = readable
        .into_iter()
        .take(READABLE_MAX)
        .map(|value| head(&value, READABLE_BYTES).to_string())
        .collect();
    (shown, total)
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
    let of = exchanges.clone();
    let built = tokio::task::spawn_blocking(move || {
        let opts = BuildOptions {
            app_version: &version,
            generated_at_ms: now,
            include_code,
            limit,
        };
        capture::build_with_readable(&exchanges, &opts)
    })
    .await
    .map_err(|e| CmdError::Message(e.to_string()))?;

    let (text, readable, problems) = match built {
        Ok(built) => (Some(built.text), built.readable, Vec::new()),
        Err(found) => (None, Vec::new(), found),
    };
    let (readable, readable_total) = listed(readable);
    let shown = text
        .as_deref()
        .map(|t| head(t, PREVIEW_BYTES).to_string())
        .unwrap_or_default();
    let bytes = text.as_ref().map_or(0, String::len);
    // The flow may have moved on while this ran (Discard, then another recording): only the
    // recording this was built from keeps it.
    cap.keep_built(&of, text);
    Ok(CapturePreview {
        truncated: shown.len() < bytes,
        text: shown,
        bytes: u32::try_from(bytes).unwrap_or(u32::MAX),
        requests,
        code_included: include_code,
        limit,
        problems,
        readable,
        readable_total,
    })
}

/// A window's title or a menu item's text from the main window: one short line.
fn short_line(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(TITLE_MAX)
        .collect()
}

/// What the main window sent, made fit to show: one short line for a title or the Stop item, the
/// steps with their line breaks and nothing else that isn't text, and never an empty button.
fn window_words(said: &CaptureTexts) -> (capture_window::Texts, String) {
    let or_stop = |text: &str| {
        let line = short_line(text);
        if line.trim().is_empty() {
            STOP_TEXT.to_string()
        } else {
            line
        }
    };
    let steps: String = said
        .steps
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .take(STEPS_MAX)
        .collect();
    (
        capture_window::Texts {
            title: short_line(&said.title),
            stop: or_stop(&said.stop),
            done_title: short_line(&said.done_title),
            done_stop: or_stop(&said.done_stop),
        },
        steps,
    )
}

/// Opens the router's own page in the recording window. `texts` is what that window says, in the
/// user's language. `Ok(Some(why))` when it can't start now.
#[tauri::command]
pub async fn capture_start(
    app: AppHandle,
    texts: CaptureTexts,
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
    // No recording is open, so any profile folder is a leftover: its browser held on to it
    // past the wait when it closed. It holds the router's session cookie.
    capture_window::sweep_profiles(state.data_dir());
    let (words, steps) = window_words(&texts);
    let recording = capture_window::open(&app, &router, state.data_dir(), &words, true)
        .map_err(CmdError::Message)?;
    *cap.lock() = Flow::Open(recording);
    emit(&app, &cap);
    // The steps, in a box on the window itself.
    if !steps.trim().is_empty() {
        capture_window::show_steps(&app, &words.title, &steps, texts.rtl);
    }
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
    let Some((text, of)) = (match &*cap.lock() {
        Flow::Stopped(s) => s.built.clone().map(|text| (text, s.exchanges.clone())),
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
    if saved && cap.forget_saved(&of) {
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

/// The recording window was closed while its recording was open: by its Stop item, by its X or
/// by Windows. That ends the recording and keeps it for the review, as Stop in the main window
/// does, and brings the main window back, because the review is there. Nothing is cleaned here:
/// this runs on the window's thread, and the review asks for the cleaning.
pub fn window_closed(app: &AppHandle) {
    let Some(cap) = app.try_state::<CaptureState>() else {
        return;
    };
    if !cap.keep_closed(|recording| recording.finish(app)) {
        return;
    }
    emit(app, &cap);
    crate::tray::show_main(app);
}

/// The app is closing: a recording still open is thrown away now, so its window going away
/// isn't taken for "stop and review".
pub fn app_exiting(app: &AppHandle) {
    let Some(cap) = app.try_state::<CaptureState>() else {
        return;
    };
    let taken = std::mem::take(&mut *cap.lock());
    if let Flow::Open(recording) = taken {
        drop(recording.finish(app));
    }
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
    fn the_readable_values_are_shown_whole_up_to_a_limit_and_counted_in_full() {
        let few = vec!["H122-373".to_string(), "Zain KSA".to_string()];
        assert_eq!(listed(few.clone()), (few, 2));
        // A file can keep thousands of distinct values: the screen gets the first of them and
        // is told how many there are.
        let many: Vec<String> = (0..READABLE_MAX + 50).map(|n| format!("v{n}")).collect();
        let (shown, total) = listed(many);
        assert_eq!(shown.len(), READABLE_MAX);
        assert_eq!(total as usize, READABLE_MAX + 50);
        // One long value is cut, never inside a character.
        let (shown, _) = listed(vec!["ع".repeat(READABLE_BYTES)]);
        assert!(shown[0].len() <= READABLE_BYTES && shown[0].chars().all(|c| c == 'ع'));
    }

    #[test]
    fn nothing_is_open_at_first_and_nothing_can_be_saved() {
        let cap = CaptureState::default();
        assert_eq!(
            cap.status(),
            CaptureStatus {
                phase: CapturePhase::Idle,
                requests: 0,
                limit: None,
                enough: false,
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

    #[test]
    fn closing_the_window_keeps_the_recording_for_its_review() {
        // The window's Stop item and its X both end here. What was recorded must not be lost.
        let router = Url::parse("http://192.168.8.1/").unwrap();
        let cap = CaptureState::default();
        *cap.lock() = Flow::Open(Recording::detached(&router));
        let mut finished = 0;
        assert!(cap.keep_closed(|_| {
            finished += 1;
            (Vec::new(), Some(Limit::Time))
        }));
        assert_eq!(finished, 1);
        assert_eq!(
            cap.status(),
            CaptureStatus {
                phase: CapturePhase::Stopped,
                requests: 0,
                limit: Some(Limit::Time),
                enough: false,
            }
        );
        assert!(!cap.is_idle());
    }

    #[test]
    fn a_window_that_closes_after_stop_or_discard_changes_nothing() {
        // Stop and Discard close the window themselves: its going away must not undo them.
        let cap = CaptureState::default();
        assert!(!cap.keep_closed(|_| unreachable!("nothing is open")));
        assert!(cap.is_idle());
        *cap.lock() = Flow::Stopped(Stopped {
            exchanges: Arc::new(Vec::new()),
            limit: None,
            built: Some("the cleaned file".into()),
        });
        assert!(!cap.keep_closed(|_| unreachable!("nothing is open")));
        assert!(matches!(&*cap.lock(), Flow::Stopped(s) if s.built.is_some()));
    }

    #[test]
    fn the_windows_words_are_short_lines_and_the_box_keeps_its_steps() {
        let said = CaptureTexts {
            title: "Log in\nthen wait".into(),
            stop: "  ".into(),
            done_title: "Done\u{7}".into(),
            done_stop: String::new(),
            steps: "1. Log in.\r\n2. Wait.\u{7}\n3. Close.".into(),
            rtl: true,
        };
        let (window, steps) = window_words(&said);
        assert_eq!(window.title, "Log inthen wait");
        assert_eq!(window.done_title, "Done");
        // Nothing to say is never an empty button or an empty box.
        assert_eq!(window.stop, STOP_TEXT);
        assert_eq!(window.done_stop, STOP_TEXT);
        // The steps keep their lines, and lose anything else that isn't text.
        assert_eq!(steps, "1. Log in.\n2. Wait.\n3. Close.");
        let long = CaptureTexts {
            steps: "x".repeat(5000),
            ..said
        };
        assert_eq!(window_words(&long).1.len(), STEPS_MAX);
    }

    #[test]
    fn a_recording_says_when_enough_is_recorded() {
        let router = Url::parse("http://192.168.8.1/").unwrap();
        let cap = CaptureState::default();
        *cap.lock() = Flow::Open(Recording::detached(&router));
        // Nothing was recorded, so not yet. The window's own watch is what sets it.
        assert_eq!(
            cap.status(),
            CaptureStatus {
                phase: CapturePhase::Recording,
                requests: 0,
                limit: None,
                enough: false,
            }
        );
    }

    #[test]
    fn a_file_belongs_to_the_recording_it_was_built_from() {
        // Cleaning takes seconds, and a save dialog stays open as long as the person likes.
        // Meanwhile the recording can be thrown away and another made.
        let waiting = |cap: &CaptureState| {
            let exchanges = Arc::new(Vec::new());
            *cap.lock() = Flow::Stopped(Stopped {
                exchanges: exchanges.clone(),
                limit: None,
                built: None,
            });
            exchanges
        };
        let cap = CaptureState::default();
        let old = waiting(&cap);
        let new = waiting(&cap);
        // A file built from the old one is not kept as the new one's, so Save can't write it.
        assert!(!cap.keep_built(&old, Some("the old recording's file".into())));
        assert!(matches!(&*cap.lock(), Flow::Stopped(s) if s.built.is_none()));
        // A save of the old one that ends now forgets nothing of the new one.
        assert!(!cap.forget_saved(&old));
        assert!(!cap.is_idle());
        assert!(cap.keep_built(&new, Some("the new recording's file".into())));
        assert!(cap.forget_saved(&new));
        assert!(cap.is_idle());
    }

    #[test]
    fn what_the_main_window_sends_for_the_window_is_one_short_line() {
        assert_eq!(short_line("Stop and\nreview\u{7}"), "Stop andreview");
        assert_eq!(short_line(&"x".repeat(500)).len(), TITLE_MAX);
        assert_eq!(short_line("أوقف وراجع"), "أوقف وراجع");
    }
}
