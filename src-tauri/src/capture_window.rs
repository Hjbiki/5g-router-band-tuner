//! The recording window: a router's own web page in a window of its own, sealed off from the app
//! and from every other host, with what the page asks the router recorded by
//! `router_core::capture`.
//!
//! This module knows nothing of the app's state, so `examples/capture_harness.rs` can drive the
//! very same code against a mock router page.
//!
//! How the window is sealed:
//!
//! - **From the app.** It has no permission of its own, and Tauri checks its permission list for
//!   every command a remote page sends, so the router's page can call nothing.
//! - **From every other host.** Its browser is started with a proxy that can't be reached, and
//!   only the router's own address goes around that proxy. A frame, a picture, a redirect or a
//!   WebSocket to anywhere else never leaves this PC.
//! - **From the user's browser and from the main window.** It has a profile folder of its own,
//!   made for this recording and deleted after it: no cookies come in, and none stay.
//! - It can open no other window, download nothing, get no permission (camera, location and the
//!   like), and can't leave the router's address.
//!
//! The one control on the window that isn't the router's is a menu item that ends the recording.
//! It is a native menu, the app's own: the router's page can neither draw there nor press it.
//! Pressing it closes the window, and whoever opened the window decides what a closed window
//! means (the app keeps the recording for its review).
//!
//! The window also says when the person can stop, in its title and on that item: once the page
//! has logged in and stopped asking for new things (`router_core::capture::enough`). Nobody
//! should have to count seconds, and a window closed the moment Log In is pressed has lost what
//! the page was about to ask.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use router_core::capture::{self, Ask, Exchange, Limit, Recorder};
use tauri::menu::{Menu, MenuItem};
use tauri::webview::{DownloadEvent, NewWindowResponse};
use tauri::{AppHandle, Manager, Runtime, Url, WebviewUrl, WebviewWindowBuilder};
#[cfg(windows)]
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2DevToolsProtocolEventReceivedEventArgs,
    COREWEBVIEW2_PERMISSION_STATE_DENY,
};
#[cfg(windows)]
use webview2_com::{
    take_pwstr, CallDevToolsProtocolMethodCompletedHandler,
    DevToolsProtocolEventReceivedEventHandler, PermissionRequestedEventHandler,
};
#[cfg(windows)]
use windows_core::{HSTRING, PWSTR};

/// The window's label. No capability names it, which is part of the seal.
pub const LABEL: &str = "capture";
/// The menu item that ends the recording.
pub const STOP_ID: &str = "capture-stop";
/// A recording stops taking more after this long.
pub const TIME_LIMIT: Duration = Duration::from_secs(15 * 60);
const PROFILE_PREFIX: &str = "capture-profile-";
/// Nothing listens here. A proxy that can't be reached is how every other host is refused.
const DEAD_PROXY: &str = "http://127.0.0.1:9";
/// What the browser may keep of answers for the recorder to ask for, and of what the page sends.
const NETWORK_ENABLE: &str =
    r#"{"maxTotalBufferSize":67108864,"maxResourceBufferSize":8388608,"maxPostDataSize":2097152}"#;
const EVENTS: [&str; 4] = [
    "Network.requestWillBeSent",
    "Network.responseReceived",
    "Network.loadingFinished",
    "Network.loadingFailed",
];

/// The browser options that seal the window's network. `<-loopback>` takes away the browser's
/// habit of going around a proxy for this PC's own addresses, so the list is exactly one host.
/// The first option is what Tauri passes by default, and must be repeated when any is given:
/// among other things it keeps SmartScreen from sending the router's addresses to Microsoft.
pub fn browser_args(router: &Url) -> Option<String> {
    if !matches!(router.scheme(), "http" | "https") {
        return None;
    }
    let host = router.host_str()?;
    let port = router.port_or_known_default()?;
    Some(format!(
        "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
         --proxy-server={DEAD_PROXY} --proxy-bypass-list=<-loopback>;{host}:{port}"
    ))
}

/// How large the window opens, from the screen's size in the same units. A router's page is laid
/// out for a wide desktop window (a real Huawei one is 1350 wide, and scrolls sideways in less),
/// so it gets that when the screen has room, and stays on the screen when it hasn't.
pub fn window_size(screen: Option<(f64, f64)>) -> (f64, f64) {
    const WANTED: (f64, f64) = (1380.0, 800.0);
    const LEAST: (f64, f64) = (900.0, 600.0);
    let Some((screen_w, screen_h)) = screen else {
        return (1280.0, 760.0);
    };
    // Room for the window's own frame, its menu and the taskbar.
    let fit = |wanted: f64, room: f64, least: f64| wanted.min(room).max(least);
    (
        fit(WANTED.0, screen_w - 40.0, LEAST.0),
        fit(WANTED.1, screen_h - 100.0, LEAST.1),
    )
}

/// Deletes the profile folders earlier recordings left behind: a profile holds the router's
/// session cookie. Call it at launch, when no recording can be open.
pub fn sweep_profiles(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(PROFILE_PREFIX)
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn lock(recorder: &Mutex<Recorder>) -> MutexGuard<'_, Recorder> {
    recorder
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What the window says, in the user's language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Texts {
    /// The window's title while it records.
    pub title: String,
    /// Its Stop item while it records.
    pub stop: String,
    /// The title once enough is recorded.
    pub done_title: String,
    /// The Stop item once enough is recorded.
    pub done_stop: String,
}

/// Where a recording stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub requests: usize,
    /// The limit that stopped it taking more, if one did.
    pub limit: Option<Limit>,
    /// Enough is recorded: the window has told the person they can close it.
    pub enough: bool,
}

/// An open recording window.
pub struct Recording {
    router: Url,
    recorder: Arc<Mutex<Recorder>>,
    profile: PathBuf,
    enough: Arc<AtomicBool>,
}

impl Recording {
    /// A recording with no window, for the tests of what happens around one.
    #[cfg(test)]
    pub(crate) fn detached(router: &Url) -> Self {
        Self {
            router: router.clone(),
            recorder: Arc::new(Mutex::new(Recorder::new(router).expect("a web address"))),
            profile: PathBuf::new(),
            enough: Arc::default(),
        }
    }

    /// How many requests are recorded so far, the limit that stopped it if one did, and whether
    /// the window has said that enough is recorded.
    pub fn progress(&self) -> Progress {
        let recorder = lock(&self.recorder);
        Progress {
            requests: recorder.len(),
            limit: recorder.limit(),
            enough: self.enough.load(Ordering::SeqCst),
        }
    }

    /// Ends the recording: closes the window, deletes its profile and hands over what was
    /// recorded. Dropping that hand-over is how a recording is discarded.
    pub fn finish<R: Runtime>(self, app: &AppHandle<R>) -> (Vec<Exchange>, Option<Limit>) {
        if let Some(window) = app.get_webview_window(LABEL) {
            let _ = window.destroy();
        }
        // The browser's handlers still hold the recorder until the window is gone: take what it
        // has, and leave them an empty one that is already stopped.
        let mut stopped = Recorder::new(&self.router).expect("the address it was opened with");
        stopped.stop(Limit::Time);
        let recorder = std::mem::replace(&mut *lock(&self.recorder), stopped);
        let limit = recorder.limit();
        // The browser lets go of its files a moment after the window closes.
        let profile = self.profile;
        std::thread::spawn(move || {
            for _ in 0..20 {
                if std::fs::remove_dir_all(&profile).is_ok() || !profile.exists() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        });
        (recorder.into_exchanges(), limit)
    }
}

/// Opens the recording window on the router's own page. `profiles` is a folder for this
/// recording's browser profile; `texts` is what the window's title and its Stop item say, in
/// the user's language. The window is shown to a person; a test harness keeps it hidden.
pub fn open<R: Runtime>(
    app: &AppHandle<R>,
    router: &Url,
    profiles: &Path,
    texts: &Texts,
    visible: bool,
) -> Result<Recording, String> {
    // The recorder listens through WebView2. A window that couldn't be recorded isn't opened.
    #[cfg(not(windows))]
    return Err("recording needs Windows".to_string());
    #[cfg(windows)]
    open_sealed(app, router, profiles, texts, visible)
}

#[cfg(windows)]
fn open_sealed<R: Runtime>(
    app: &AppHandle<R>,
    router: &Url,
    profiles: &Path,
    texts: &Texts,
    visible: bool,
) -> Result<Recording, String> {
    let args = browser_args(router).ok_or("the router's address isn't a web address")?;
    let recorder = Recorder::new(router).ok_or("the router's address isn't a web address")?;
    let recorder = Arc::new(Mutex::new(recorder));
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let profile = profiles.join(format!("{PROFILE_PREFIX}{}-{stamp}", std::process::id()));
    let origin = router.origin().ascii_serialization();
    let stop = MenuItem::with_id(app, STOP_ID, &texts.stop, true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let menu = Menu::with_items(app, &[&stop]).map_err(|e| e.to_string())?;
    let screen = app.primary_monitor().ok().flatten().map(|m| {
        let size = m.size().to_logical::<f64>(m.scale_factor());
        (size.width, size.height)
    });
    let (width, height) = window_size(screen);

    let window = WebviewWindowBuilder::new(
        app,
        LABEL,
        WebviewUrl::External("about:blank".parse().expect("a valid address")),
    )
    .title(&texts.title)
    .visible(visible)
    .inner_size(width, height)
    .menu(menu)
    .on_menu_event(|window, event| {
        if event.id().as_ref() == STOP_ID {
            let _ = window.close();
        }
    })
    .data_directory(profile.clone())
    .additional_browser_args(&args)
    .devtools(false)
    .on_navigation(move |url| {
        url.as_str() == "about:blank" || url.origin().ascii_serialization() == origin
    })
    .on_new_window(|_, _| NewWindowResponse::Deny)
    // Nothing the router offers is saved to this PC.
    .on_download(|_, event| !matches!(event, DownloadEvent::Requested { .. }))
    .build()
    .map_err(|e| e.to_string())?;

    let for_browser = recorder.clone();
    let first = router.to_string();
    window
        .with_webview(move |platform| {
            // SAFETY: on the webview's own thread, with the controller Tauri hands out.
            match unsafe { platform.controller().CoreWebView2() } {
                Ok(core) => attach(core, for_browser, first),
                Err(e) => eprintln!("capture: the window has no browser: {e}"),
            }
        })
        .map_err(|e| e.to_string())?;

    // Once a second: has the page logged in and stopped asking for new things? Then the window
    // says so, where the person is looking. A limit says the same: nothing more will be taken.
    let enough = Arc::new(AtomicBool::new(false));
    {
        let (recorder, enough, texts) = (recorder.clone(), enough.clone(), texts.clone());
        std::thread::spawn(move || {
            let mut distinct = 0;
            let mut changed = Instant::now();
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let (seen, full) = {
                    let rec = lock(&recorder);
                    (rec.seen(), rec.limit().is_some())
                };
                if seen.distinct != distinct {
                    distinct = seen.distinct;
                    changed = Instant::now();
                }
                // `full` is also how a finished recording looks from here: the window is gone
                // then, and telling it anything does nothing.
                if full || capture::enough(seen, changed.elapsed()) {
                    enough.store(true, Ordering::SeqCst);
                    let _ = window.set_title(&texts.done_title);
                    let _ = stop.set_text(&texts.done_stop);
                    return;
                }
            }
        });
    }

    Ok(Recording {
        router: router.clone(),
        recorder,
        profile,
        enough,
    })
}

#[cfg(windows)]
fn event_json(args: Option<ICoreWebView2DevToolsProtocolEventReceivedEventArgs>) -> String {
    let Some(args) = args else {
        return String::new();
    };
    let mut text = PWSTR::null();
    // SAFETY: `text` receives a string the browser allocated, which `take_pwstr` frees.
    match unsafe { args.ParameterObjectAsJson(&mut text) } {
        Ok(()) => take_pwstr(text),
        Err(_) => String::new(),
    }
}

#[cfg(windows)]
/// Asks the browser for something, and gives the answer to `then`. A failed call still answers,
/// with nothing, so the recorder can note that the body isn't there.
fn call(core: &ICoreWebView2, method: &str, params: &str, then: impl FnOnce(String) + 'static) {
    let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
        move |result, json: String| {
            then(if result.is_ok() { json } else { String::new() });
            Ok(())
        },
    ));
    // SAFETY: on the webview's own thread, with live COM pointers.
    let sent = unsafe {
        core.CallDevToolsProtocolMethod(&HSTRING::from(method), &HSTRING::from(params), &handler)
    };
    if let Err(e) = sent {
        eprintln!("capture: {method} could not be sent: {e}");
    }
}

#[cfg(windows)]
/// Switches the recording on, then asks for the router's page: nothing the page does is missed.
fn attach(core: ICoreWebView2, recorder: Arc<Mutex<Recorder>>, first: String) {
    let started = Instant::now();
    for name in EVENTS {
        let recorder = recorder.clone();
        let handler =
            DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |sender, args| {
                let json = event_json(args);
                let ask = {
                    let mut rec = lock(&recorder);
                    if started.elapsed() > TIME_LIMIT {
                        rec.stop(Limit::Time);
                    }
                    rec.event(name, &json)
                };
                let (Some(core), Some(ask)) = (sender, ask) else {
                    return Ok(());
                };
                // A body is asked for the moment it is ready: after the page moves on it may be gone.
                let recorder = recorder.clone();
                match ask {
                    Ask::ResponseBody(id) => {
                        let params = serde_json::json!({ "requestId": id }).to_string();
                        call(&core, "Network.getResponseBody", &params, move |answer| {
                            lock(&recorder).response_body(&id, &answer);
                        });
                    }
                    Ask::RequestBody(id) => {
                        let params = serde_json::json!({ "requestId": id }).to_string();
                        call(
                            &core,
                            "Network.getRequestPostData",
                            &params,
                            move |answer| {
                                lock(&recorder).request_body(&id, &answer);
                            },
                        );
                    }
                }
                Ok(())
            }));
        let mut token = 0i64;
        // SAFETY: on the webview's own thread, with live COM pointers.
        let added = unsafe {
            core.GetDevToolsProtocolEventReceiver(&HSTRING::from(name))
                .and_then(|receiver| {
                    receiver.add_DevToolsProtocolEventReceived(&handler, &mut token)
                })
        };
        if let Err(e) = added {
            eprintln!("capture: no receiver for {name}: {e}");
        }
    }

    // The router's page gets no permission: no camera, microphone, location or notifications.
    let deny = PermissionRequestedEventHandler::create(Box::new(|_, args| {
        if let Some(args) = args {
            // SAFETY: inside the browser's own event, with its arguments.
            unsafe { args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)? };
        }
        Ok(())
    }));
    let mut token = 0i64;
    // SAFETY: on the webview's own thread, with live COM pointers.
    if let Err(e) = unsafe { core.add_PermissionRequested(&deny, &mut token) } {
        eprintln!("capture: permission prompts could not be refused: {e}");
    }

    let page = core.clone();
    call(&core, "Network.enable", NETWORK_ENABLE, move |_| {
        // SAFETY: on the webview's own thread.
        if let Err(e) = unsafe { page.Navigate(&HSTRING::from(first.as_str())) } {
            eprintln!("capture: the router's page could not be opened: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_is_as_wide_as_a_routers_page_when_the_screen_has_room() {
        // Seen on a real H122: its page is 1350 wide, and a narrower window scrolls sideways.
        assert_eq!(window_size(Some((2560.0, 1440.0))), (1380.0, 800.0));
        assert_eq!(window_size(Some((1920.0, 1080.0))), (1380.0, 800.0));
        // On a small laptop it stays on the screen, with room for the taskbar.
        assert_eq!(window_size(Some((1366.0, 768.0))), (1326.0, 668.0));
        // Never too small to use, and a sensible size when the screen can't be asked.
        assert_eq!(window_size(Some((800.0, 600.0))), (900.0, 600.0));
        assert_eq!(window_size(None), (1280.0, 760.0));
    }

    #[test]
    fn only_the_routers_own_address_goes_around_the_dead_proxy() {
        let args =
            browser_args(&Url::parse("http://192.168.8.1/html/index.html").unwrap()).unwrap();
        assert!(args.contains("--proxy-server=http://127.0.0.1:9"));
        assert!(args.ends_with("--proxy-bypass-list=<-loopback>;192.168.8.1:80"));
        // Tauri's own default must still be there: it switches SmartScreen off.
        assert!(args.contains("msSmartScreenProtection"));
        let tls = browser_args(&Url::parse("https://router.lan:8443/").unwrap()).unwrap();
        assert!(tls.ends_with(";router.lan:8443"));
        for not_web in ["file:///C:/x.html", "about:blank", "data:text/html,x"] {
            assert_eq!(
                browser_args(&Url::parse(not_web).unwrap()),
                None,
                "{not_web}"
            );
        }
    }

    #[test]
    fn old_recording_profiles_are_swept_and_nothing_else_is() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join("capture-profile-123-456");
        std::fs::create_dir_all(stale.join("EBWebView")).unwrap();
        std::fs::write(stale.join("EBWebView/Cookies"), "session").unwrap();
        let other = dir.path().join("history.db");
        std::fs::write(&other, "keep").unwrap();
        let folder = dir.path().join("journal-archive");
        std::fs::create_dir_all(&folder).unwrap();
        sweep_profiles(dir.path());
        assert!(!stale.exists());
        assert!(other.exists() && folder.exists());
        // A folder that isn't there is nothing to sweep.
        sweep_profiles(&dir.path().join("missing"));
    }
}
