//! "Save router page scripts": a read-only copy of the router's own web page code, so a call
//! such as the factory reset (and later the APN writes) can be checked in the firmware's code
//! instead of being sent to find out.
//!
//! Only GETs, through `RouterDriver::web_page`, starting at `START_PAGE` with the app's logged-in
//! session. Every reference is checked by `validate_path` before it is fetched: same router, one
//! of `PREFIXES`, a `.js` or `.html` ending, plain characters only, no `..`, `//` or backslash.
//! Each file is unpacked when it is a gzip body, then redacted (`redact`) and scanned again; a
//! file the scan still flags is left out. Limits on count, size, depth and time keep a hostile
//! device at the router's address from filling the PC.

use std::collections::{BTreeSet, HashSet, VecDeque};
use std::io::Read;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use reqwest::Url;

use crate::{RouterDriver, RouterError};

/// The page the router's settings pages load from once logged in (a 307 to `/html/index.html`
/// when not logged in).
pub const START_PAGE: &str = "/html/content.html";
/// The folders the crawler may fetch from.
pub const PREFIXES: &[&str] = &["/js/", "/lib/", "/html/"];
/// `RouterDriver::web_page` refuses with this when the router redirects to its login page.
pub const PAGE_NOT_LOGGED_IN: &str = "not logged in";
/// `RouterDriver::web_page` refuses with this when the router has no such file.
pub const PAGE_NOT_FOUND: &str = "no such page";
/// `RouterDriver::web_page` refuses a path outside `PREFIXES` with this, sending nothing.
pub const PAGE_PATH_REFUSED: &str = "this page path is not allowed";
/// The default refusal: the driver can't fetch web pages.
pub const PAGES_NOT_SUPPORTED: &str = "saving the router's page scripts is not supported";

/// How far the crawler may go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_files: usize,
    /// All saved files together, after unpacking.
    pub max_total_bytes: usize,
    /// One file after unpacking (the zip-bomb cap).
    pub max_file_bytes: usize,
    /// Links followed from `START_PAGE` (which is depth 0).
    pub max_depth: u8,
    /// The whole run.
    pub budget: Duration,
    /// Between two GETs, so the router isn't flooded.
    pub gap: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_files: 400,
            max_total_bytes: 8 * 1024 * 1024,
            max_file_bytes: 4 * 1024 * 1024,
            max_depth: 3,
            budget: Duration::from_secs(180),
            gap: Duration::from_millis(50),
        }
    }
}

/// One saved file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptFile {
    /// The router path (`/js/main.js`), already validated.
    pub path: String,
    /// The redacted text, as UTF-8.
    pub bytes: Vec<u8>,
    /// Values blanked or replaced by `redact`.
    pub redactions: usize,
    /// The router sent it as a gzip body.
    pub gunzipped: bool,
    /// Bytes that weren't UTF-8 were replaced.
    pub not_utf8: bool,
}

/// Why a path (or a reference) was left out. The UI words each kind in the user's language;
/// `text` is the English for the zip's manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipKind {
    Timeout,
    Unreachable,
    /// The router answered with an error.
    Refused,
    TooLarge,
    TooLargeUnpacked,
    /// A gzip body that didn't unpack.
    BadGzip,
    OverTotal,
    StillFlagged,
    /// A reference to another site, or a path outside the allowed folders.
    NotFollowed,
    /// Anything else, with the driver's own message (English, for the manifest only).
    Other(String),
}

impl SkipKind {
    pub fn text(&self) -> String {
        match self {
            SkipKind::Timeout => "the router didn't answer in time".into(),
            SkipKind::Unreachable => "the router couldn't be reached".into(),
            SkipKind::Refused => "the router refused it".into(),
            SkipKind::TooLarge => "too large".into(),
            SkipKind::TooLargeUnpacked => "too large once unpacked".into(),
            SkipKind::BadGzip => "couldn't be unpacked".into(),
            SkipKind::OverTotal => "over the total size limit".into(),
            SkipKind::StillFlagged => "an identifier was still found after redaction".into(),
            SkipKind::NotFollowed => {
                "not followed: another site, or a path outside the allowed folders".into()
            }
            SkipKind::Other(m) => m.clone(),
        }
    }
}

/// What `collect` brought back.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScriptBundle {
    pub files: Vec<ScriptFile>,
    /// Paths (or references) left out, with the reason.
    pub skipped: Vec<(String, SkipKind)>,
    /// Paths the router answered with its error page.
    pub missing: Vec<String>,
    /// The limit that ended the run early, if any.
    pub stopped: Option<String>,
}

impl ScriptBundle {
    pub fn total_bytes(&self) -> usize {
        self.files.iter().map(|f| f.bytes.len()).sum()
    }
}

/// References kept in `skipped` at most, so a page full of odd links can't grow the manifest.
const MAX_SKIPPED_REFS: usize = 100;
/// The longest string literal looked at as a reference.
const MAX_REF_LEN: usize = 512;
/// The router's error page for an unknown name is 88 bytes; anything this short that starts
/// with `<` is no script.
const ERROR_PAGE_MAX: usize = 256;
/// The run stops after this many failed GETs in a row (the router went away).
const MAX_FAILURES_IN_A_ROW: usize = 5;

/// Whether `path` (already decoded, no query) is one the crawler may fetch: `/`-rooted, under one
/// of `PREFIXES`, `[A-Za-z0-9_./-]` only, no `..`, `//` or backslash, ending `.js` or `.html`.
/// The drivers check this again before any request.
pub fn is_allowed_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= MAX_REF_LEN
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_./-".contains(&b))
        && !path.contains("..")
        && !path.contains("//")
        && !path.contains('\\')
        && PREFIXES.iter().any(|p| path.starts_with(p))
        && has_page_ending(path)
}

fn has_page_ending(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".js") || lower.ends_with(".html")
}

/// Strict percent-decoding: a `%` not followed by two hex digits, or bytes that aren't UTF-8,
/// refuse the reference.
fn percent_decode_strict(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The router path a reference `raw` found on the page at `base` points to, or `None` when any
/// rule refuses it. The query and fragment are dropped first, then the rest is percent-decoded
/// once and must pass the character whitelist (so a scheme, `//host`, `..`, `%2e%2e` and a
/// backslash all fail). It is resolved with `Url::join` and must keep `base`'s scheme, host and
/// port and land under one of `PREFIXES`.
pub fn validate_path(raw: &str, base: &Url) -> Option<String> {
    let raw = raw.trim();
    let cut = raw.find(['?', '#']).unwrap_or(raw.len());
    let decoded = percent_decode_strict(&raw[..cut])?;
    if decoded.is_empty()
        || decoded.len() > MAX_REF_LEN
        || !decoded
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_./-".contains(&b))
        || decoded.contains("..")
        || decoded.contains("//")
        || decoded.contains('\\')
    {
        return None;
    }
    let url = base.join(&decoded).ok()?;
    if url.scheme() != base.scheme()
        || url.host_str() != base.host_str()
        || url.port_or_known_default() != base.port_or_known_default()
    {
        return None;
    }
    let path = url.path().to_string();
    is_allowed_path(&path).then_some(path)
}

/// What `extract_paths` found in one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extracted {
    /// Validated router paths, in the order first seen.
    pub paths: Vec<String>,
    /// References that looked like page code but failed `validate_path`.
    pub rejected: Vec<String>,
}

/// Quoted strings of a reasonable length on one line.
static LITERAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#""([^"\r\n]{1,512})"|'([^'\r\n]{1,512})'|`([^`\r\n]{1,512})`"#).unwrap()
});

/// References in an HTML page or a script: `src` and `href` values, and any string literal that
/// names the `js`, `lib` or `html` folder, ending `.js` or `.html` (query and fragment aside).
/// Each is checked with `validate_path` against `from`, the page the browser would resolve it
/// against.
pub fn extract_paths(text: &str, from: &Url) -> Extracted {
    let mut out = Extracted::default();
    let mut seen = HashSet::new();
    for caps in LITERAL.captures_iter(text) {
        let Some(m) = caps.get(1).or_else(|| caps.get(2)).or_else(|| caps.get(3)) else {
            continue;
        };
        let value = m.as_str().trim();
        let whole = caps.get(0).map_or(0, |w| w.start());
        // Only the few characters before the quote matter (`src=`, `href =`).
        let mut from_byte = whole.saturating_sub(16);
        while !text.is_char_boundary(from_byte) {
            from_byte -= 1;
        }
        let before = text[from_byte..whole].trim_end().to_ascii_lowercase();
        let attribute = before.ends_with("src=") || before.ends_with("href=");
        let bare = &value[..value.find(['?', '#']).unwrap_or(value.len())];
        if !has_page_ending(bare) {
            continue;
        }
        let lower = bare.to_ascii_lowercase();
        let names_folder = ["js/", "lib/", "html/"].iter().any(|f| lower.contains(f));
        if !attribute && !names_folder {
            continue;
        }
        if !seen.insert(value.to_string()) {
            continue;
        }
        match validate_path(value, from) {
            Some(p) => {
                if !out.paths.contains(&p) {
                    out.paths.push(p);
                }
            }
            None => out.rejected.push(value.to_string()),
        }
    }
    out
}

/// `bytes` unpacked when they start with the gzip magic (1f 8b), whatever the headers said.
/// Returns the bytes and whether they were unpacked. More than `cap` bytes once unpacked, or a
/// body that doesn't unpack, is refused.
pub fn maybe_gunzip(bytes: Vec<u8>, cap: usize) -> Result<(Vec<u8>, bool), SkipKind> {
    if !bytes.starts_with(&[0x1f, 0x8b]) {
        if bytes.len() > cap {
            return Err(SkipKind::TooLarge);
        }
        return Ok((bytes, false));
    }
    let mut out = Vec::new();
    let limit = u64::try_from(cap).unwrap_or(u64::MAX).saturating_add(1);
    flate2::read::MultiGzDecoder::new(bytes.as_slice())
        .take(limit)
        .read_to_end(&mut out)
        .map_err(|_| SkipKind::BadGzip)?;
    if out.len() > cap {
        return Err(SkipKind::TooLargeUnpacked);
    }
    Ok((out, true))
}

/// The router's answer for a name it doesn't have: a short HTML page where a script was asked.
pub fn is_error_page(path: &str, bytes: &[u8]) -> bool {
    path.to_ascii_lowercase().ends_with(".js")
        && bytes.len() < ERROR_PAGE_MAX
        && bytes
            .iter()
            .find(|b| !b.is_ascii_whitespace())
            .is_some_and(|b| *b == b'<')
}

static TOKEN_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<(?:meta|input)\b[^>]*>").unwrap());
static TOKEN_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(?:name|id)\s*=\s*["']?\s*(?:csrf_token|__RequestVerificationToken)\b"#)
        .unwrap()
});
static TOKEN_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(content|value)\s*=\s*("[^"]*"|'[^']*'|[^\s>"']+)"#).unwrap()
});
/// `__RequestVerificationToken: "…"` or `= '…'` with a value written into a page.
static TOKEN_ASSIGN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(["']?(?:__RequestVerificationToken|csrf_token)["']?\s*[:=]\s*)("[^"\r\n]+"|'[^'\r\n]+')"#)
        .unwrap()
});

/// Blanks the CSRF token values a logged-in page carries (`<meta name="csrf_token" content=…>`,
/// any `__RequestVerificationToken` value), then replaces every token the diagnostics leak scan
/// flags (`diagnostics::redact_identifier_tokens`). Returns the text and how many values went.
pub fn redact(text: &str) -> (String, usize) {
    let mut count = 0;
    let tags = TOKEN_TAG.replace_all(text, |c: &regex::Captures| {
        let tag = &c[0];
        if !TOKEN_NAME.is_match(tag) {
            return tag.to_string();
        }
        TOKEN_VALUE
            .replace_all(tag, |v: &regex::Captures| {
                let value = v[2].trim_matches(['"', '\'']);
                if !value.is_empty() {
                    count += 1;
                }
                format!("{}=\"\"", &v[1])
            })
            .into_owned()
    });
    let assigned = TOKEN_ASSIGN.replace_all(&tags, |c: &regex::Captures| {
        let value = c[2].trim_matches(['"', '\'']);
        // A literal that only names the header (`"__RequestVerificationToken"`) isn't a value.
        if value.eq_ignore_ascii_case("__RequestVerificationToken")
            || value.eq_ignore_ascii_case("csrf_token")
        {
            return c[0].to_string();
        }
        count += 1;
        format!("{}\"\"", &c[1])
    });
    let (out, hits) = crate::diagnostics::redact_identifier_tokens(&assigned);
    (out, count + hits.len())
}

/// The last check on a redacted file: the leak scan with escape sequences read as spaces, so a
/// number hidden after a `\n` in a string literal still counts. Any hit leaves the file out.
pub fn still_flagged(text: &str) -> Vec<String> {
    let plain = text
        .replace("\\n", " ")
        .replace("\\t", " ")
        .replace("\\r", " ");
    crate::diagnostics::identifier_hits(&plain)
}

fn skip_reason(e: &RouterError) -> SkipKind {
    match e {
        RouterError::Timeout => SkipKind::Timeout,
        RouterError::Unreachable => SkipKind::Unreachable,
        RouterError::Validation { message } => SkipKind::Other(message.clone()),
        RouterError::Parse { message } => SkipKind::Other(message.clone()),
        _ => SkipKind::Refused,
    }
}

/// Whether the run can't go on: the session is gone.
fn ends_run(e: &RouterError) -> bool {
    matches!(e, RouterError::SessionLost { .. })
        || matches!(e, RouterError::Validation { message } if message == PAGE_NOT_LOGGED_IN)
}

/// Fetches `START_PAGE` and what it links to, breadth first, within `limits`. GETs only.
/// `origin` is the router's base address (`http://192.168.8.1`). An error means not even the
/// start page could be read (for example: not logged in) and nothing was collected.
pub async fn collect(
    driver: &dyn RouterDriver,
    origin: &str,
    limits: &Limits,
) -> Result<ScriptBundle, RouterError> {
    let invalid = || RouterError::Validation {
        message: "invalid router address".to_string(),
    };
    let origin = Url::parse(origin).map_err(|_| invalid())?;
    if !matches!(origin.scheme(), "http" | "https") || origin.host_str().is_none() {
        return Err(invalid());
    }
    let started = tokio::time::Instant::now();
    let mut bundle = ScriptBundle::default();
    let mut total = 0usize;
    let mut failures = 0usize;
    let mut first = true;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut rejected: BTreeSet<String> = BTreeSet::new();
    // (path, depth, the page its links resolve against)
    let mut queue: VecDeque<(String, u8, String)> = VecDeque::new();
    seen.insert(START_PAGE.to_string());
    queue.push_back((START_PAGE.to_string(), 0, START_PAGE.to_string()));

    while let Some((path, depth, page)) = queue.pop_front() {
        if bundle.files.len() + bundle.missing.len() >= limits.max_files {
            bundle.stopped = Some(format!("the {} file limit", limits.max_files));
            break;
        }
        if !first {
            tokio::time::sleep(limits.gap).await;
        }
        if started.elapsed() >= limits.budget {
            bundle.stopped = Some("the time limit".to_string());
            break;
        }
        let body = driver.web_page(&path).await;
        let body = match body {
            Ok(b) => {
                failures = 0;
                b
            }
            Err(e) if first => return Err(e),
            Err(RouterError::Validation { message }) if message == PAGE_NOT_FOUND => {
                bundle.missing.push(path);
                continue;
            }
            Err(e) => {
                bundle.skipped.push((path, skip_reason(&e)));
                failures += 1;
                if ends_run(&e) {
                    bundle.stopped = Some("the router session ended".to_string());
                    break;
                }
                if failures >= MAX_FAILURES_IN_A_ROW {
                    bundle.stopped = Some("the router stopped answering".to_string());
                    break;
                }
                continue;
            }
        };
        first = false;
        let (bytes, gunzipped) = match maybe_gunzip(body, limits.max_file_bytes) {
            Ok(v) => v,
            Err(why) => {
                bundle.skipped.push((path, why));
                continue;
            }
        };
        if is_error_page(&path, &bytes) {
            bundle.missing.push(path);
            continue;
        }
        if total + bytes.len() > limits.max_total_bytes {
            bundle.skipped.push((path, SkipKind::OverTotal));
            bundle.stopped = Some(format!(
                "the {} MB size limit",
                limits.max_total_bytes / (1024 * 1024)
            ));
            break;
        }
        let not_utf8 = std::str::from_utf8(&bytes).is_err();
        let text = String::from_utf8_lossy(&bytes);
        if depth < limits.max_depth {
            let base_page = if path.to_ascii_lowercase().ends_with(".html") {
                path.clone()
            } else {
                page.clone()
            };
            if let Ok(base) = origin.join(&base_page) {
                let found = extract_paths(&text, &base);
                for p in found.paths {
                    if seen.insert(p.clone()) {
                        queue.push_back((p, depth + 1, base_page.clone()));
                    }
                }
                rejected.extend(found.rejected);
            }
        }
        let (redacted, redactions) = redact(&text);
        if !still_flagged(&redacted).is_empty() {
            bundle.skipped.push((path, SkipKind::StillFlagged));
            continue;
        }
        total += redacted.len();
        bundle.files.push(ScriptFile {
            path,
            bytes: redacted.into_bytes(),
            redactions,
            gunzipped,
            not_utf8,
        });
    }

    for r in rejected.into_iter().take(MAX_SKIPPED_REFS) {
        // A reference is only text in the manifest, but it is redacted like a file all the same.
        let shown: String = r
            .chars()
            .filter(|c| c.is_ascii_graphic())
            .take(200)
            .collect();
        let (shown, _) = redact(&shown);
        if still_flagged(&shown).is_empty() {
            bundle.skipped.push((shown, SkipKind::NotFollowed));
        }
    }
    Ok(bundle)
}
