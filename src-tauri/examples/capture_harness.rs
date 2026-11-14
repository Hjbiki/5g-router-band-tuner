//! Drives the real recording window (`capture_window`) and the real cleaning
//! (`router_core::capture`) against a mock router page served from this process. It never talks
//! to a router and starts none of the app: no state, no actor, no main window.
//!
//! It checks what only a real browser can show:
//!   - the router's page can call none of the app's commands;
//!   - nothing the page does reaches another host (frame, picture, fetch, redirect, WebSocket);
//!   - what the page asks the router is recorded, bodies included, and comes out cleaned.
//!
//! Build and run (the manifest is what any Tauri window needs on Windows):
//!   cargo rustc -p router-band-tuner --example capture_harness -- \
//!     -C link-arg=/MANIFEST:EMBED \
//!     -C link-arg=/MANIFESTINPUT:<full path to src-tauri\examples\common-controls.manifest>
//!   target\debug\examples\capture_harness.exe [show]
//!
//! Run it from `src-tauri`. Without `show` the window stays hidden.
//!
//! It prints one `CHECK` line per check and ends with `HARNESS PASSED` or `HARNESS FAILED`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use router_band_tuner_lib::capture_window;
use router_core::capture::{self, BuildOptions};
use tauri::Url;

/// Set only if the app command's body runs: a router's page must never get this far.
static SECRET_RAN: AtomicBool = AtomicBool::new(false);

#[tauri::command]
fn harness_secret() -> &'static str {
    SECRET_RAN.store(true, Ordering::SeqCst);
    "secret-from-rust"
}

/// `{"gz":"ok","model":"MOCK-5G-CPE"}` gzipped. Served with no Content-Encoding, as the H122 does.
const GZ: &[u8] = &[
    31, 139, 8, 0, 0, 0, 0, 0, 0, 10, 171, 86, 74, 175, 82, 178, 82, 202, 207, 86, 210, 81, 202,
    205, 79, 73, 205, 81, 178, 82, 242, 245, 119, 246, 214, 53, 117, 215, 117, 14, 112, 85, 170, 5,
    0, 184, 25, 182, 21, 33, 0, 0, 0,
];

/// `PORT` is replaced by the mock's port. `localhost:PORT` is the same server under another
/// name: to the browser it is another host, and the window must not reach it.
const INDEX: &str = r#"<!doctype html><meta charset="utf-8"><title>Mock 5G CPE</title><body><h1>mock router</h1>
<form id="f" method="post" action="/login"><input name="user" value="admin"><input name="pw" value="hunter2-secret"></form>
<script>
const within = (p, ms) => Promise.race([p, new Promise((_, no) => setTimeout(() => no(new Error('no answer in ' + ms + ' ms')), ms))]);
const post = (path, what) => fetch(path, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(what) });
(async () => {
  const ipc = { internals: typeof window.__TAURI_INTERNALS__ };
  try { ipc.app = await within(window.__TAURI_INTERNALS__.invoke('harness_secret'), 3000); }
  catch (e) { ipc.appError = String((e && e.message) || e); }
  try { ipc.plugin = await within(window.__TAURI_INTERNALS__.invoke('plugin:app|version'), 3000); }
  catch (e) { ipc.pluginError = String((e && e.message) || e); }
  await post('/note/ipc', ipc);

  await new Promise((done) => { const x = new XMLHttpRequest(); x.open('POST', '/api/xhr');
    x.setRequestHeader('Content-Type', 'application/json'); x.onloadend = done; x.send(JSON.stringify({ lte_band_lock: '800C5' })); });
  await fetch('/api/gz').then((r) => r.arrayBuffer());

  // Everything below goes to another host, and must fail without leaving this PC.
  const other = 'http://localhost:PORT/other';
  const tried = {};
  tried.fetch = await within(fetch(other + '/fetch', { mode: 'no-cors' }), 4000).then(() => 'answered', (e) => 'failed: ' + e.message);
  tried.image = await within(new Promise((ok, no) => { const i = new Image(); i.onload = () => ok('loaded'); i.onerror = () => ok('failed'); i.src = other + '/pixel.png'; }), 4000).catch((e) => 'no answer');
  tried.socket = await within(new Promise((ok) => { const s = new WebSocket('ws://localhost:PORT/other/ws'); s.onopen = () => ok('opened'); s.onerror = () => ok('failed'); }), 4000).catch((e) => 'no answer');
  const frame = document.createElement('iframe'); frame.src = other + '/frame'; document.body.appendChild(frame);
  await new Promise((r) => setTimeout(r, 1500));
  await post('/note/other', tried);
  document.getElementById('f').submit();
})();
</script>"#;

const AFTER: &str = r#"<!doctype html><meta charset="utf-8"><title>after login</title><body>
<script>
(async () => {
  await fetch('/api/status').then((r) => r.json());
  let popup; try { popup = window.open('/popup') ? 'opened' : 'refused'; } catch (e) { popup = 'threw'; }
  await fetch('/note/popup', { method: 'POST', body: JSON.stringify({ popup }) });
  setTimeout(() => { location.href = '/redirect-away'; }, 100);
  setTimeout(() => { fetch('/done?stayed=1'); }, 2500);
})();
</script>"#;

struct Mock {
    port: u16,
    /// Every request as the server saw it: `host path`.
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
    let header = |name: &str| {
        head.lines()
            .find_map(|l| {
                l.to_ascii_lowercase()
                    .strip_prefix(name)
                    .map(|v| v.trim().to_string())
            })
            .unwrap_or_default()
    };
    let length = header("content-length:").parse::<usize>().unwrap_or(0);
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
    mock.seen
        .lock()
        .unwrap()
        .push(format!("{} {target}", header("host:")));

    let page = |s: &str| {
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
    let port = mock.port.to_string();
    let (status, headers, bytes) = match (method, path) {
        ("GET", "/") => page(&INDEX.replace("PORT", &port)),
        ("POST", p) if p.starts_with("/note/") => {
            mock.notes.lock().unwrap().push(format!("{p} {body}"));
            json("{}")
        }
        ("POST", "/api/xhr") => json(r#"{"result":"success"}"#),
        ("GET", "/api/gz") => (
            "200 OK",
            vec![("Content-Type", "application/javascript".to_string())],
            GZ.to_vec(),
        ),
        ("POST", "/login") => (
            "302 Found",
            vec![("Location", "/after".to_string())],
            b"moved".to_vec(),
        ),
        ("GET", "/after") => page(AFTER),
        ("GET", "/api/status") => json(
            r#"{"lte_rsrp":"-97","band":"3","WifiSsid":"Mock-Home-Net","model_name":"MOCK-5G","firmware_version":"1.2.3","imei":"356789012345678"}"#,
        ),
        ("GET", "/popup") => page("popup"),
        ("GET", "/redirect-away") => (
            "302 Found",
            vec![("Location", format!("http://localhost:{port}/other/landing"))],
            Vec::new(),
        ),
        ("GET", "/done") => {
            if let Some(tx) = mock.done.lock().unwrap().as_ref() {
                let _ = tx.send(target.to_string());
            }
            json("{}")
        }
        // The other host's pages. If one is ever asked for, the seal has a hole.
        (_, p) if p.starts_with("/other/") => {
            page("<script>fetch('/done?leaked=1')</script>other host")
        }
        _ => ("404 Not Found", Vec::new(), b"no".to_vec()),
    };
    respond(&mut stream, status, &headers, &bytes);
}

fn main() {
    let show = std::env::args().any(|a| a == "show");
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

    let profiles = std::env::temp_dir().join(format!("rbt-capture-harness-{}", std::process::id()));
    std::fs::create_dir_all(&profiles).expect("a folder for the profile");
    let router = Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap();

    let mut context = tauri::generate_context!();
    // No app window: the harness must not start the real UI or anything that talks to a router.
    context.config_mut().app.windows.clear();

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![harness_secret])
        .setup(move |app| {
            let recording =
                capture_window::open(app.handle(), &router, &profiles, "capture harness", show)?;
            let handle = app.handle().clone();
            let other = format!("localhost:{port}");
            std::thread::spawn(move || {
                let ended = done_rx
                    .recv_timeout(Duration::from_secs(45))
                    .unwrap_or_else(|_| "TIMEOUT".into());
                // Let late answers, and a leak if the seal failed, arrive.
                std::thread::sleep(Duration::from_millis(2500));
                let later: Vec<String> = done_rx.try_iter().collect();
                let (seen_so_far, _) = recording.progress();
                let (exchanges, limit) = recording.finish(&handle);
                let built = capture::build(
                    &exchanges,
                    &BuildOptions {
                        app_version: "harness",
                        generated_at_ms: 0,
                        include_code: false,
                        limit,
                    },
                );

                let mut failed = 0;
                let mut check = |name: &str, ok: bool, detail: String| {
                    println!(
                        "CHECK {} {name}{}",
                        if ok { "ok  " } else { "FAIL" },
                        if ok {
                            String::new()
                        } else {
                            format!(": {detail}")
                        }
                    );
                    failed += usize::from(!ok);
                };
                let notes = mock.notes.lock().unwrap().clone();
                let seen = mock.seen.lock().unwrap().clone();
                let note = |which: &str| {
                    notes
                        .iter()
                        .find(|n| n.starts_with(which))
                        .cloned()
                        .unwrap_or_default()
                };

                check(
                    "the page ran to its end",
                    ended.contains("stayed=1") && later.is_empty(),
                    format!("{ended} {later:?}"),
                );
                check(
                    "the app command never ran",
                    !SECRET_RAN.load(Ordering::SeqCst),
                    "its body ran".into(),
                );
                let ipc = note("/note/ipc");
                check(
                    "the app command was refused",
                    ipc.contains("appError") && !ipc.contains("secret-from-rust"),
                    ipc.clone(),
                );
                check(
                    "the plugin command was refused",
                    ipc.contains("pluginError"),
                    ipc,
                );
                let reached: Vec<&String> = seen.iter().filter(|s| s.starts_with(&other)).collect();
                check(
                    "no request reached another host",
                    reached.is_empty(),
                    format!("{reached:?}"),
                );
                let tried = note("/note/other");
                check(
                    "the page saw each of them fail",
                    tried.contains("\"fetch\":\"failed")
                        && tried.contains("\"image\":\"failed\"")
                        && tried.contains("\"socket\":\"failed\""),
                    tried,
                );
                check(
                    "a pop-up was refused",
                    note("/note/popup").contains("refused"),
                    note("/note/popup"),
                );

                let paths: Vec<String> = exchanges
                    .iter()
                    .map(|x| format!("{} {}", x.method, x.path))
                    .collect();
                for wanted in [
                    "GET /",
                    "POST /api/xhr",
                    "GET /api/gz",
                    "POST /login",
                    "GET /after",
                    "GET /api/status",
                ] {
                    check(
                        &format!("recorded {wanted}"),
                        paths.iter().any(|p| p == wanted),
                        format!("{paths:?}"),
                    );
                }
                check(
                    "the recorder counted while it ran",
                    seen_so_far >= 6,
                    seen_so_far.to_string(),
                );
                check(
                    "nothing of another host was recorded",
                    !paths.iter().any(|p| p.contains("/other/")),
                    format!("{paths:?}"),
                );
                let body_of = |path: &str| {
                    exchanges
                        .iter()
                        .find(|x| x.path == path)
                        .and_then(|x| x.body.clone())
                        .unwrap_or_default()
                };
                check(
                    "an answer's body was kept",
                    body_of("/api/xhr") == r#"{"result":"success"}"#,
                    body_of("/api/xhr"),
                );
                check(
                    "gzip sent without saying so was unpacked",
                    body_of("/api/gz").contains("MOCK-5G-CPE"),
                    body_of("/api/gz"),
                );
                let login = exchanges.iter().find(|x| x.path == "/login");
                check(
                    "the form the page sent was recorded",
                    login.is_some_and(|x| {
                        x.request_body
                            .as_deref()
                            .is_some_and(|b| b.contains("pw=hunter2-secret"))
                    }),
                    format!("{login:?}"),
                );

                match &built {
                    Ok(text) => {
                        let v: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
                        let list = v["exchanges"].as_array().cloned().unwrap_or_default();
                        let entry = |path: &str| {
                            list.iter()
                                .find(|x| x["path"] == path)
                                .cloned()
                                .unwrap_or_default()
                        };
                        check("the cleaned file was made", true, String::new());
                        for gone in ["hunter2-secret", "Mock-Home-Net", "356789012345678"] {
                            check(
                                &format!("the file doesn't hold {gone}"),
                                !text.contains(gone),
                                "it does".into(),
                            );
                        }
                        check(
                            "the login is marked and hidden",
                            entry("/login")["login"] == true
                                && entry("/login")["request"]["pw"] == "(hidden)",
                            entry("/login").to_string(),
                        );
                        check(
                            "the band command is readable",
                            entry("/api/xhr")["request"]["lte_band_lock"] == "800C5",
                            entry("/api/xhr").to_string(),
                        );
                        check(
                            "the reading is readable",
                            entry("/api/status")["response"]["lte_rsrp"] == "-97",
                            entry("/api/status").to_string(),
                        );
                        check(
                            "the model was found",
                            v["router"]["model"] == "MOCK-5G" && v["router"]["firmware"] == "1.2.3",
                            v["router"].to_string(),
                        );
                    }
                    Err(found) => check("the cleaned file was made", false, format!("{found:?}")),
                }

                // The profile holds the mock's cookies: it must be gone soon after the window.
                let gone = (0..24).any(|_| {
                    std::thread::sleep(Duration::from_millis(500));
                    std::fs::read_dir(&profiles)
                        .map(|d| d.count() == 0)
                        .unwrap_or(true)
                });
                check(
                    "the browser profile was deleted",
                    gone,
                    profiles.display().to_string(),
                );
                let _ = std::fs::remove_dir_all(&profiles);

                println!(
                    "{}",
                    if failed == 0 {
                        "HARNESS PASSED"
                    } else {
                        "HARNESS FAILED"
                    }
                );
                handle.exit(i32::from(failed != 0));
            });
            Ok(())
        })
        .build(context)
        .expect("harness app")
        .run(|_, event| {
            // Closing the recording window leaves the harness with no window, and Tauri would
            // end the process there, before the checks are printed. Only `exit` ends it.
            if let tauri::RunEvent::ExitRequested {
                api, code: None, ..
            } = event
            {
                api.prevent_exit();
            }
        });
}
