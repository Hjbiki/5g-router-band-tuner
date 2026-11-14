//! Throwaway spike for the built-in capture. Not part of the app, and it never talks to a router:
//! it opens a hidden window on a mock router page served from this process, and prints what it
//! learned as JSON.
//!
//! It answers two questions before anything real is built:
//!   1. Is a window showing a router's page sealed off from the app's commands?
//!   2. Does WebView2's DevTools protocol keep every request and answer, bodies included?
//!
//! Run: cargo run -p router-band-tuner --example capture_spike

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use tauri::webview::NewWindowResponse;
use tauri::{WebviewUrl, WebviewWindowBuilder};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2DevToolsProtocolEventReceivedEventArgs,
};
use webview2_com::{
    take_pwstr, CallDevToolsProtocolMethodCompletedHandler,
    DevToolsProtocolEventReceivedEventHandler,
};
use windows_core::{HSTRING, PWSTR};

/// Set only if the app command's body runs: a router page must never get this far.
static SECRET_RAN: AtomicBool = AtomicBool::new(false);

#[tauri::command]
fn spike_secret() -> &'static str {
    SECRET_RAN.store(true, Ordering::SeqCst);
    "secret-from-rust"
}

/// `{"gz":"ok","model":"MOCK-5G-CPE"}` gzipped. Served with no Content-Encoding, as the H122 does.
const GZ: &[u8] = &[
    31, 139, 8, 0, 0, 0, 0, 0, 0, 10, 171, 86, 74, 175, 82, 178, 82, 202, 207, 86, 210, 81, 202,
    205, 79, 73, 205, 81, 178, 82, 242, 245, 119, 246, 214, 53, 117, 215, 117, 14, 112, 85, 170, 5,
    0, 184, 25, 182, 21, 33, 0, 0, 0,
];

const INDEX: &str = r#"<!doctype html><meta charset="utf-8"><title>Mock 5G CPE</title><body><h1>mock router</h1>
<form id="f" method="post" action="/login"><input name="user" value="admin"><input name="pw" value="hunter2"></form>
<script>
const within = (p, ms) => Promise.race([p, new Promise((_, no) => setTimeout(() => no(new Error('no answer in ' + ms + ' ms')), ms))]);
(async () => {
  const out = { internals: typeof window.__TAURI_INTERNALS__, ipc: typeof window.ipc,
                chromeWebview: typeof (window.chrome && window.chrome.webview) };
  try { out.app = await within(window.__TAURI_INTERNALS__.invoke('spike_secret'), 3000); }
  catch (e) { out.appError = String((e && e.message) || e); }
  try { out.plugin = await within(window.__TAURI_INTERNALS__.invoke('plugin:app|version'), 3000); }
  catch (e) { out.pluginError = String((e && e.message) || e); }
  await fetch('/ipc-result', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(out) });
  await new Promise((done) => { const x = new XMLHttpRequest(); x.open('POST', '/api/xhr');
    x.setRequestHeader('Content-Type', 'application/json'); x.onloadend = done; x.send(JSON.stringify({ hello: 'xhr' })); });
  await fetch('/api/gz').then((r) => r.arrayBuffer());
  document.getElementById('f').submit();
})();
</script>"#;

const AFTER: &str = r#"<!doctype html><meta charset="utf-8"><title>after login</title><body>
<script>
(async () => {
  await fetch('/api/after', { method: 'POST', body: 'a=1&b=2',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' } }).then((r) => r.text());
  const fr = document.createElement('iframe'); fr.src = 'http://localhost:PORT/frame'; document.body.appendChild(fr);
  await new Promise((r) => setTimeout(r, 800));
  let popup; try { popup = window.open('/popup') ? 'opened' : 'null'; } catch (e) { popup = 'threw ' + e; }
  await fetch('/note', { method: 'POST', body: JSON.stringify({ popup }) });
  setTimeout(() => { location.href = '/redir-out'; }, 100);
  setTimeout(() => { fetch('/done?stayed=1'); }, 1500);
})();
</script>"#;

struct Mock {
    port: u16,
    /// Each request as the server saw it: the truth the recording is compared with.
    seen: Mutex<Vec<String>>,
    notes: Mutex<Vec<String>>,
    done: Mutex<Option<mpsc::Sender<String>>>,
}

fn respond(stream: &mut TcpStream, status: &str, headers: &[(&str, String)], body: &[u8]) {
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
}

fn serve(mock: &Mock, mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let length = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
        })
        .unwrap_or(0);
    while buf.len() < head_end + length {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let body = String::from_utf8_lossy(&buf[head_end..]).to_string();
    let mut first = head.lines().next().unwrap_or("").split(' ');
    let method = first.next().unwrap_or("");
    let target = first.next().unwrap_or("");
    let host = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("host:")
                .map(|v| v.trim().to_string())
        })
        .unwrap_or_default();
    mock.seen
        .lock()
        .unwrap()
        .push(format!("{method} {host}{target} body={body}"));

    let html = |s: &str| {
        (
            "200 OK",
            vec![("Content-Type", "text/html; charset=utf-8".to_string())],
            s.as_bytes().to_vec(),
        )
    };
    let json = |s: &str| {
        (
            "200 OK",
            vec![("Content-Type", "application/json".to_string())],
            s.as_bytes().to_vec(),
        )
    };
    let path = target.split('?').next().unwrap_or("");
    let (status, headers, bytes) = match (method, path) {
        ("GET", "/") => html(INDEX),
        ("POST", "/ipc-result") | ("POST", "/note") => {
            mock.notes.lock().unwrap().push(body.clone());
            json("{}")
        }
        ("POST", "/api/xhr") => json(r#"{"ok":1,"answer":"xhr-body"}"#),
        // Gzip bytes under a JavaScript type and no Content-Encoding, like the H122's /js files.
        ("GET", "/api/gz") => (
            "200 OK",
            vec![("Content-Type", "application/javascript".to_string())],
            GZ.to_vec(),
        ),
        ("POST", "/login") => (
            "302 Found",
            vec![("Location", "/after".to_string())],
            b"login-answer-body".to_vec(),
        ),
        ("GET", "/after") => html(&AFTER.replace("PORT", &mock.port.to_string())),
        ("POST", "/api/after") => (
            "200 OK",
            vec![("Content-Type", "text/xml".to_string())],
            b"<response><Model>MOCK</Model></response>".to_vec(),
        ),
        ("GET", "/frame") => html("<script>fetch('/frame-api')</script>frame"),
        ("GET", "/frame-api") => json(r#"{"frame":1}"#),
        ("GET", "/popup") => html("popup"),
        ("GET", "/redir-out") => (
            "302 Found",
            vec![(
                "Location",
                format!("http://localhost:{}/elsewhere", mock.port),
            )],
            Vec::new(),
        ),
        ("GET", "/elsewhere") => html("<script>fetch('/done?leaked=1')</script>elsewhere"),
        ("GET", "/done") => {
            if let Some(tx) = mock.done.lock().unwrap().as_ref() {
                let _ = tx.send(target.to_string());
            }
            json("{}")
        }
        _ => ("404 Not Found", Vec::new(), b"no".to_vec()),
    };
    respond(&mut stream, status, &headers, &bytes);
}

/// What the DevTools protocol reported, in order: `(event or method, JSON)`.
type Recording = Arc<Mutex<Vec<(String, String)>>>;

fn event_json(args: Option<ICoreWebView2DevToolsProtocolEventReceivedEventArgs>) -> String {
    let Some(args) = args else {
        return String::new();
    };
    let mut p = PWSTR::null();
    // SAFETY: `p` receives a CoTaskMem string that `take_pwstr` frees.
    match unsafe { args.ParameterObjectAsJson(&mut p) } {
        Ok(()) => take_pwstr(p),
        Err(e) => format!("{{\"spikeError\":\"{e}\"}}"),
    }
}

fn call(
    core: &ICoreWebView2,
    method: &'static str,
    params: String,
    rec: Recording,
    then: Option<Box<dyn FnOnce()>>,
) {
    let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
        move |result, json: String| {
            let text = match result {
                Ok(()) => json,
                Err(e) => format!("{{\"spikeError\":\"{}\"}}", e.message()),
            };
            rec.lock().unwrap().push((format!("answer:{method}"), text));
            if let Some(then) = then {
                then();
            }
            Ok(())
        },
    ));
    // SAFETY: called on the webview's own thread, with live COM pointers.
    let sent = unsafe {
        core.CallDevToolsProtocolMethod(
            &HSTRING::from(method),
            &HSTRING::from(params.as_str()),
            &handler,
        )
    };
    if let Err(e) = sent {
        eprintln!("spike: {method} could not be sent: {e}");
    }
}

fn attach(core: ICoreWebView2, rec: Recording, first_url: String) {
    for name in [
        "Network.requestWillBeSent",
        "Network.responseReceived",
        "Network.loadingFinished",
        "Network.loadingFailed",
    ] {
        let rec = rec.clone();
        let handler =
            DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |sender, args| {
                let json = event_json(args);
                if name == "Network.loadingFinished" {
                    if let (Some(core), Ok(v)) = (
                        sender.as_ref(),
                        serde_json::from_str::<serde_json::Value>(&json),
                    ) {
                        let id = v["requestId"].as_str().unwrap_or("").to_string();
                        let params = serde_json::json!({ "requestId": id }).to_string();
                        let tagged = rec.clone();
                        let tag_rec = rec.clone();
                        // The body is asked for the moment the load finishes: later it may be gone.
                        let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
                            move |result, body: String| {
                                let text = match result {
                                    Ok(()) => body,
                                    Err(e) => format!("{{\"spikeError\":\"{}\"}}", e.message()),
                                };
                                tagged.lock().unwrap().push((format!("body:{id}"), text));
                                Ok(())
                            },
                        ));
                        // SAFETY: on the webview's thread, inside its own event.
                        if let Err(e) = unsafe {
                            core.CallDevToolsProtocolMethod(
                                &HSTRING::from("Network.getResponseBody"),
                                &HSTRING::from(params.as_str()),
                                &handler,
                            )
                        } {
                            tag_rec
                                .lock()
                                .unwrap()
                                .push(("body-not-asked".into(), e.message()));
                        }
                    }
                }
                rec.lock().unwrap().push((name.to_string(), json));
                Ok(())
            }));
        let mut token = 0i64;
        // SAFETY: on the webview's thread, with live COM pointers.
        let added = unsafe {
            core.GetDevToolsProtocolEventReceiver(&HSTRING::from(name))
                .and_then(|r| r.add_DevToolsProtocolEventReceived(&handler, &mut token))
        };
        if let Err(e) = added {
            eprintln!("spike: no receiver for {name}: {e}");
        }
    }
    // Recording is switched on before the first page is asked for, so nothing is missed.
    let nav = core.clone();
    call(
        &core,
        "Network.enable",
        "{}".into(),
        rec,
        Some(Box::new(move || {
            // SAFETY: on the webview's thread.
            if let Err(e) = unsafe { nav.Navigate(&HSTRING::from(first_url.as_str())) } {
                eprintln!("spike: could not navigate: {e}");
            }
        })),
    );
}

fn main() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind a local port");
    let port = listener.local_addr().unwrap().port();
    let (done_tx, done_rx) = mpsc::channel::<String>();
    let mock = Arc::new(Mock {
        port,
        seen: Mutex::default(),
        notes: Mutex::default(),
        done: Mutex::new(Some(done_tx)),
    });
    {
        let mock = mock.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mock = mock.clone();
                std::thread::spawn(move || serve(&mock, stream));
            }
        });
    }

    let rec: Recording = Arc::default();
    let navigations: Arc<Mutex<Vec<String>>> = Arc::default();
    let popups: Arc<Mutex<Vec<String>>> = Arc::default();
    let profile = std::env::temp_dir().join(format!("rbt-capture-spike-{}", std::process::id()));
    println!("SPIKE profile {}", profile.display());

    let mut context = tauri::generate_context!();
    // No app window: this spike must not start the real UI or anything that talks to a router.
    context.config_mut().app.windows.clear();

    let allowed = format!("http://127.0.0.1:{port}");
    let first_url = format!("{allowed}/");
    let (rec2, nav2, pop2, mock2) = (
        rec.clone(),
        navigations.clone(),
        popups.clone(),
        mock.clone(),
    );

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![spike_secret])
        .setup(move |app| {
            let nav_log = navigations.clone();
            let pop_log = popups.clone();
            let origin = allowed.clone();
            let window = WebviewWindowBuilder::new(app, "capture", WebviewUrl::External("about:blank".parse().unwrap()))
                .title("capture spike")
                .visible(false)
                .data_directory(profile.clone())
                .on_navigation(move |url| {
                    let ok = url.as_str() == "about:blank" || url.origin().ascii_serialization() == origin;
                    nav_log.lock().unwrap().push(format!("{} {url}", if ok { "allow" } else { "BLOCK" }));
                    ok
                })
                .on_new_window(move |url, _features| {
                    pop_log.lock().unwrap().push(format!("deny {url}"));
                    NewWindowResponse::Deny
                })
                .build()?;

            let rec = rec.clone();
            let first = first_url.clone();
            window.with_webview(move |platform| {
                // SAFETY: on the webview's thread, with the controller Tauri hands out.
                match unsafe { platform.controller().CoreWebView2() } {
                    Ok(core) => attach(core, rec, first),
                    Err(e) => eprintln!("spike: no CoreWebView2: {e}"),
                }
            })?;

            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let ended = done_rx.recv_timeout(Duration::from_secs(25)).unwrap_or_else(|_| "TIMEOUT".into());
                // Let late answers and a leak (if the block failed) arrive.
                std::thread::sleep(Duration::from_millis(2500));
                let later: Vec<String> = done_rx.try_iter().collect();
                let events: Vec<serde_json::Value> = rec2
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| serde_json::json!({ "kind": k, "data": serde_json::from_str::<serde_json::Value>(v).unwrap_or(serde_json::Value::String(v.clone())) }))
                    .collect();
                let out = serde_json::json!({
                    "ended": ended,
                    "laterDone": later,
                    "secretRan": SECRET_RAN.load(Ordering::SeqCst),
                    "pageNotes": *mock2.notes.lock().unwrap(),
                    "navigations": *nav2.lock().unwrap(),
                    "popups": *pop2.lock().unwrap(),
                    "serverSaw": *mock2.seen.lock().unwrap(),
                    "events": events,
                });
                println!("SPIKE-RESULT {out}");
                handle.exit(0);
            });
            Ok(())
        })
        .run(context)
        .expect("spike app");
}
