//! Checking GitHub for a newer release. Notify only: nothing is downloaded or installed.
//!
//! The automatic check runs only if the user allowed it (`Settings::update_check`), at most once a
//! day, and never while a band change, scan or speed test is open. Any failure there is silent.
//! The Check button works either way and reports a failure, because the user just asked.
//!
//! The only address is `API_URL`, requested from Rust; the webview's CSP is unchanged. The response
//! is untrusted: the release page URL is opened only if it is under this repo's releases, and the
//! notes reach the UI as a short plain-text summary.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use ts_rs::TS;

use crate::commands::{AppState, CmdError};

pub const EVENT: &str = "update";
const API_URL: &str = "https://api.github.com/repos/Hjbiki/5g-router-band-tuner/releases/latest";
const RELEASES_PAGE: &str = "https://github.com/Hjbiki/5g-router-band-tuner/releases";
const STATE_FILE: &str = "update-check.json";
pub const ERR_UNREACHABLE: &str = "couldn't reach GitHub";

const DAY_MS: u64 = 24 * 60 * 60 * 1000;
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(30);
/// How often a due check is looked at, so an app left in the tray still checks daily, and a
/// check that was skipped or failed tries again.
const TICK: Duration = Duration::from_secs(60 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_SUMMARY_CHARS: usize = 300;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Release {
    version: String,
    summary: String,
    url: String,
}

/// `update-check.json`: its own file, so a settings save from the UI can't overwrite it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Stored {
    last_check_ms: u64,
    latest: Option<Release>,
    dismissed: Option<String>,
}

fn load(dir: &Path) -> Stored {
    std::fs::read_to_string(dir.join(STATE_FILE))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save(dir: &Path, s: &Stored) {
    if let Ok(json) = serde_json::to_string_pretty(s) {
        let tmp = dir.join("update-check.json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(tmp, dir.join(STATE_FILE));
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AvailableUpdate {
    pub version: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpdateView {
    pub current: String,
    pub available: Option<AvailableUpdate>,
}

fn view(current: &str, s: &Stored) -> UpdateView {
    let available = s.latest.as_ref().and_then(|r| {
        (is_newer(&r.version, current) && s.dismissed.as_deref() != Some(r.version.as_str())).then(
            || AvailableUpdate {
                version: r.version.clone(),
                summary: r.summary.clone(),
            },
        )
    });
    UpdateView {
        current: current.to_string(),
        available,
    }
}

/// `1.2.3` or `v1.2.3` as numbers. Anything else, a prerelease included, is `None`.
fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.strip_prefix('v').unwrap_or(v);
    let mut parts = v.split('.');
    let mut next = || -> Option<u64> {
        let p = parts.next()?;
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        p.parse().ok()
    };
    let triple = (next()?, next()?, next()?);
    parts.next().is_none().then_some(triple)
}

/// Whether `candidate` is strictly newer. An unreadable version never counts as newer.
fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// The URL to open: GitHub's only if it is a page under this repo's releases.
fn safe_release_url(url: &str) -> &str {
    let prefix = format!("{RELEASES_PAGE}/");
    let plain = url
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/:.-_".contains(c));
    if url.starts_with(&prefix) && plain && !url.contains("..") {
        url
    } else {
        RELEASES_PAGE
    }
}

/// The first paragraph as plain text: headings skipped, Markdown markers and links removed,
/// control characters dropped, and cut to a short length.
fn summarize(body: &str) -> String {
    let mut paragraph: Vec<String> = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            if paragraph.is_empty() {
                continue;
            }
            break;
        }
        if line.starts_with('#') {
            if paragraph.is_empty() {
                continue;
            }
            break;
        }
        let line = line.trim_start_matches(['-', '*', '+', '>', ' ']);
        paragraph.push(strip_markdown(line));
    }
    let text = paragraph.join(" ");
    let mut out: String = text
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_SUMMARY_CHARS)
        .collect();
    if text.chars().count() > MAX_SUMMARY_CHARS {
        out.push('…');
    }
    out.trim().to_string()
}

/// `[text](url)` becomes `text`, and emphasis and code markers go. Angle brackets go too: this
/// is shown as text, but nothing that looks like markup should survive a later change.
fn strip_markdown(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        let Some(mid) = rest[open..].find("](") else {
            break;
        };
        let Some(close) = rest[open + mid..].find(')') else {
            break;
        };
        out.push_str(&rest[..open]);
        out.push_str(&rest[open + 1..open + mid]);
        rest = &rest[open + mid + close + 1..];
    }
    out.push_str(rest);
    out.chars()
        .filter(|c| !"*_`<>".contains(*c))
        .collect::<String>()
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: Option<String>,
    body: Option<String>,
    html_url: Option<String>,
}

/// The newest full release. `Ok(None)` when there isn't a usable one: a 404 means no release yet,
/// or a private repo.
async fn fetch_latest(url: &str, app_version: &str) -> Result<Option<Release>, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(REQUEST_TIMEOUT)
        .user_agent(format!("5g-router-band-tuner/{app_version}"))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let mut r = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    if r.status().as_u16() == 404 {
        return Ok(None);
    }
    if !r.status().is_success() {
        return Err(format!("HTTP {}", r.status().as_u16()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = r.chunk().await.map_err(|e| e.without_url().to_string())? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_RESPONSE_BYTES {
            return Err("the response is too large".into());
        }
    }
    let api: ApiRelease = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let Some(tag) = api.tag_name else {
        return Ok(None);
    };
    let Some(numbers) = parse_version(&tag) else {
        return Ok(None);
    };
    Ok(Some(Release {
        version: format!("{}.{}.{}", numbers.0, numbers.1, numbers.2),
        summary: summarize(api.body.as_deref().unwrap_or("")),
        url: safe_release_url(api.html_url.as_deref().unwrap_or("")).to_string(),
    }))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn current_version(app: &AppHandle) -> String {
    app.package_info().version.to_string()
}

/// A band change, scan or speed test may have the internet down, or be measuring it.
fn busy(app: &AppHandle) -> bool {
    let change = app
        .try_state::<AppState>()
        .is_none_or(|s| crate::speedtest::band_change_open(&s.snapshot().safe_apply));
    let scanning = app
        .try_state::<crate::autoscan::ScanState>()
        .is_some_and(|s| s.is_running());
    let testing = app
        .try_state::<crate::speedtest::SpeedTestState>()
        .is_some_and(|s| s.is_running());
    change || scanning || testing
}

/// Starts the background check. Harmless when the user hasn't allowed it: each tick looks at the
/// setting first, and sends nothing without it.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_AFTER).await;
        loop {
            automatic_check(&app).await;
            tokio::time::sleep(TICK).await;
        }
    });
}

async fn automatic_check(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    if state.update_check() != Some(true) || busy(app) {
        return;
    }
    let dir = state.data_dir().to_path_buf();
    let mut stored = load(&dir);
    if now_ms().saturating_sub(stored.last_check_ms) < DAY_MS {
        return;
    }
    let current = current_version(app);
    // Silent on failure, and not marked as checked, so the next tick tries again.
    let Ok(latest) = fetch_latest(API_URL, &current).await else {
        return;
    };
    stored.last_check_ms = now_ms();
    if latest.is_some() {
        stored.latest = latest;
    }
    save(&dir, &stored);
    let _ = app.emit(EVENT, view(&current, &stored));
}

/// What's known already, with no network call.
#[tauri::command]
pub fn update_status(app: AppHandle, state: State<'_, AppState>) -> UpdateView {
    view(&current_version(&app), &load(state.data_dir()))
}

/// The Check button: ignores the daily limit, and shows a dismissed version again.
#[tauri::command]
pub async fn check_for_updates(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<UpdateView, CmdError> {
    let dir = state.data_dir().to_path_buf();
    let current = current_version(&app);
    let latest = fetch_latest(API_URL, &current)
        .await
        .map_err(|_| CmdError::Message(ERR_UNREACHABLE.into()))?;
    let mut stored = load(&dir);
    stored.last_check_ms = now_ms();
    stored.dismissed = None;
    if latest.is_some() {
        stored.latest = latest;
    }
    save(&dir, &stored);
    let v = view(&current, &stored);
    let _ = app.emit(EVENT, v.clone());
    Ok(v)
}

/// Hides the notice until a newer version than this one appears.
#[tauri::command]
pub fn dismiss_update(app: AppHandle, state: State<'_, AppState>, version: String) -> UpdateView {
    let dir = state.data_dir();
    let mut stored = load(dir);
    stored.dismissed = Some(version);
    save(dir, &stored);
    let v = view(&current_version(&app), &stored);
    let _ = app.emit(EVENT, v.clone());
    v
}

/// Opens the release page in the browser. The URL comes from the stored, checked value.
#[tauri::command]
pub fn open_release_page(app: AppHandle, state: State<'_, AppState>) -> Result<(), CmdError> {
    use tauri_plugin_opener::OpenerExt;
    let stored = load(state.data_dir());
    let url = stored
        .latest
        .as_ref()
        .map_or(RELEASES_PAGE, |r| safe_release_url(&r.url));
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| CmdError::Message(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn versions_compare_as_numbers_and_only_when_readable() {
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.9.9", "0.9.9"));
        assert!(!is_newer("0.9.8", "0.9.9"));
        for odd in [
            "",
            "v",
            "1.2",
            "1.2.3.4",
            "1.2.x",
            "1.2.3-beta",
            "latest",
            "1..3",
        ] {
            assert!(!is_newer(odd, "0.0.1"), "{odd}");
        }
        assert!(!is_newer("9.9.9", "not a version"));
    }

    #[test]
    fn only_this_repos_release_pages_are_opened() {
        let good = "https://github.com/Hjbiki/5g-router-band-tuner/releases/tag/v1.2.3";
        assert_eq!(safe_release_url(good), good);
        for bad in [
            "https://evil.example/Hjbiki/5g-router-band-tuner/releases/tag/v1",
            "https://github.com/someone/else/releases/tag/v1",
            "https://github.com/Hjbiki/5g-router-band-tuner/releases/../../../evil",
            "https://github.com/Hjbiki/5g-router-band-tuner/releases/tag/v1?x=y",
            "http://github.com/Hjbiki/5g-router-band-tuner/releases/tag/v1",
            "file:///C:/Windows/System32/calc.exe",
            "",
        ] {
            assert_eq!(safe_release_url(bad), RELEASES_PAGE, "{bad}");
        }
    }

    #[test]
    fn notes_become_a_short_plain_summary() {
        let body = "## What's new\n\nFaster **scans** and [a fix](https://example.com/x) for `B3`.\nSecond line.\n\nMore later.";
        assert_eq!(
            summarize(body),
            "Faster scans and a fix for B3. Second line."
        );
        assert_eq!(summarize("- First point\n- Second"), "First point Second");
        assert_eq!(
            summarize("<script>alert(1)</script>"),
            "scriptalert(1)/script"
        );
        assert_eq!(summarize(""), "");
        let long = "a".repeat(400);
        let s = summarize(&long);
        assert_eq!(s.chars().count(), MAX_SUMMARY_CHARS + 1);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn the_notice_follows_the_version_and_the_dismissal() {
        let release = |version: &str| Release {
            version: version.into(),
            summary: "s".into(),
            url: RELEASES_PAGE.into(),
        };
        let mut s = Stored {
            latest: Some(release("1.0.0")),
            ..Stored::default()
        };
        assert_eq!(view("0.9.0", &s).available.unwrap().version, "1.0.0");
        assert!(view("1.0.0", &s).available.is_none(), "already on it");
        assert!(view("1.1.0", &s).available.is_none(), "ahead of it");
        s.dismissed = Some("1.0.0".into());
        assert!(view("0.9.0", &s).available.is_none());
        s.latest = Some(release("1.0.1"));
        assert_eq!(view("0.9.0", &s).available.unwrap().version, "1.0.1");
    }

    #[test]
    fn the_stored_state_survives_and_a_bad_file_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()), Stored::default());
        let s = Stored {
            last_check_ms: 5,
            ..Stored::default()
        };
        save(dir.path(), &s);
        assert_eq!(load(dir.path()), s);
        std::fs::write(dir.path().join(STATE_FILE), "not json").unwrap();
        assert_eq!(load(dir.path()), Stored::default());
    }

    async fn server(t: ResponseTemplate) -> MockServer {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(header("User-Agent", "5g-router-band-tuner/0.1.0"))
            .and(header("Accept", "application/vnd.github+json"))
            .respond_with(t)
            .mount(&s)
            .await;
        s
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reads_a_release_and_distrusts_its_contents() {
        let s = server(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "tag_name": "v1.2.3",
            "body": "Better **bands**.",
            "html_url": "https://evil.example/phish",
        })))
        .await;
        let r = fetch_latest(&s.uri(), "0.1.0").await.unwrap().unwrap();
        assert_eq!(r.version, "1.2.3");
        assert_eq!(r.summary, "Better bands.");
        assert_eq!(r.url, RELEASES_PAGE);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_release_and_bad_responses_are_handled() {
        // No release yet, or a private repo.
        let s = server(ResponseTemplate::new(404)).await;
        assert_eq!(fetch_latest(&s.uri(), "0.1.0").await, Ok(None));
        // An odd tag isn't an update.
        let s = server(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"tag_name": "nightly"})),
        )
        .await;
        assert_eq!(fetch_latest(&s.uri(), "0.1.0").await, Ok(None));
        // Rate limited.
        let s = server(ResponseTemplate::new(403)).await;
        assert!(fetch_latest(&s.uri(), "0.1.0").await.is_err());
        // Not JSON.
        let s = server(ResponseTemplate::new(200).set_body_string("<html>")).await;
        assert!(fetch_latest(&s.uri(), "0.1.0").await.is_err());
        // Far too large.
        let s =
            server(ResponseTemplate::new(200).set_body_bytes(vec![b' '; MAX_RESPONSE_BYTES + 1]))
                .await;
        assert_eq!(
            fetch_latest(&s.uri(), "0.1.0").await,
            Err("the response is too large".to_string())
        );
        // Offline.
        assert!(fetch_latest("http://127.0.0.1:9", "0.1.0").await.is_err());
    }
}
