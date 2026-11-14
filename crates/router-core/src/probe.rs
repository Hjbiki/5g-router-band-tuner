//! Finds out what router this is from its public pages, with no login, so someone with an
//! unsupported router can report it.
//!
//! GET only, to the router's own address and port. No cookies, no credentials, and a fixed budget
//! of requests and bytes. It keeps extracted facts, never page contents, and the report leaves out
//! the router's address. Finding a model is best effort: many routers don't state it before login.
//!
//! On some routers a GET can change something, so the probe never asks for what a page merely
//! mentions. It follows real redirects and script files, without their query strings, and only to
//! paths that look like static pages: see `followable`. An API path it reads is only ever text.

use std::collections::{BTreeSet, HashSet, VecDeque};
use std::io::Read;
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use reqwest::Url;
use serde::Serialize;
use serde_json::json;
use ts_rs::TS;

use crate::diagnostics::{is_mac, Redactor};

pub const REPORT_FORMAT: &str = "router-band-tuner-unknown-router/1";

/// Common pages to start from. Whatever the first of them links to is followed from there.
const FIRST_PATHS: &[&str] = &[
    "/api/device/basic_information",
    "/",
    "/index.html",
    "/login.html",
    "/webpages/login.html",
    "/webpages/index.html",
];
const MAX_REQUESTS: usize = 14;
const MAX_PAGE_BYTES: usize = 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 6 * 1024 * 1024;
const MAX_API_PATHS: usize = 60;
const MAX_TITLES: usize = 5;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Support {
    /// A model this app has a tested driver for.
    Supported,
    Unknown,
    /// Nothing answered at this address, so nothing is known.
    Unreachable,
}

/// What the notice shows.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProbeView {
    pub support: Support,
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub hardware: Option<String>,
    pub firmware: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageSeen {
    pub path: String,
    pub status: u16,
    pub content_type: String,
    pub bytes: usize,
}

/// Everything the probe learned. Nothing here has been checked for leaks yet: `report` does that.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RouterFacts {
    pub reachable: bool,
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub hardware: Option<String>,
    pub firmware: Option<String>,
    pub titles: Vec<String>,
    pub server: Option<String>,
    pub auth_realm: Option<String>,
    pub pages: Vec<PageSeen>,
    pub api_paths: BTreeSet<String>,
}

impl RouterFacts {
    pub fn view(&self) -> ProbeView {
        ProbeView {
            support: self.support(),
            vendor: self.vendor.clone(),
            model: self.model.clone(),
            hardware: self.hardware.clone(),
            firmware: self.firmware.clone(),
        }
    }

    pub fn support(&self) -> Support {
        if !self.reachable {
            Support::Unreachable
        } else if self.model.as_deref().is_some_and(is_supported_model) {
            Support::Supported
        } else {
            Support::Unknown
        }
    }
}

/// The models with a tested driver.
pub fn is_supported_model(model: &str) -> bool {
    model.trim().to_ascii_uppercase().starts_with("H122")
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("a valid pattern"))
}

/// `<tag>value</tag>` or `key: "value"` / `key = 'value'` for any of these key names.
fn field(
    text: &str,
    tags: &'static OnceLock<Regex>,
    keys: &str,
    assigns: &'static OnceLock<Regex>,
) -> Option<String> {
    let tag = re(tags, &format!(r"(?i)<({keys})>\s*([^<]{{2,48}}?)\s*</"));
    let assign = re(
        assigns,
        &format!(r#"(?i)["']?\b({keys})["']?\s*[:=]\s*["']([^"'<>{{}}%$\\]{{2,48}})["']"#),
    );
    let first = tag.captures(text).or_else(|| assign.captures(text))?;
    let value = first.get(2)?.as_str().trim();
    let placeholder = ["undefined", "null", "none", "unknown", "model", "router"];
    let usable = value.bytes().any(|b| b.is_ascii_digit())
        && !placeholder.contains(&value.to_ascii_lowercase().as_str())
        && !value.chars().any(char::is_control);
    usable.then(|| value.to_string())
}

pub(crate) fn model_field(text: &str) -> Option<String> {
    static T: OnceLock<Regex> = OnceLock::new();
    static A: OnceLock<Regex> = OnceLock::new();
    field(
        text,
        &T,
        "devicename|device_name|modelname|model_name|devicemodel|productname|product_name|productclass",
        &A,
    )
}

pub(crate) fn hardware_field(text: &str) -> Option<String> {
    static T: OnceLock<Regex> = OnceLock::new();
    static A: OnceLock<Regex> = OnceLock::new();
    field(
        text,
        &T,
        "hardwareversion|hardware_version|hwversion|hw_version|hw_ver|hardver",
        &A,
    )
}

pub(crate) fn firmware_field(text: &str) -> Option<String> {
    static T: OnceLock<Regex> = OnceLock::new();
    static A: OnceLock<Regex> = OnceLock::new();
    field(
        text,
        &T,
        "softwareversion|software_version|firmwareversion|firmware_version|softversion|fw_version|fw_ver|sw_ver",
        &A,
    )
}

/// Router model names as they usually appear in a title or login realm.
pub(crate) fn model_in_text(text: &str) -> Option<String> {
    static M: OnceLock<Regex> = OnceLock::new();
    let m = re(
        &M,
        r"(?i)\b(TL-[A-Z]{2,3}\d{3,4}[A-Z0-9]*|Archer\s+[A-Z]{0,3}\d{2,4}[A-Z0-9]*|Deco\s+[A-Z]{0,3}\d{2,4}[A-Z0-9-]*|NE\d{3}|NX\d{3,4}|H\d{3}-\d{3}|B\d{3}[A-Za-z]?-\d{2,3}[A-Za-z]?|E\d{4}[A-Za-z]{0,2}-\d{2,3}[A-Za-z]?|MF\d{3}[A-Z]?|MC\d{3,4}[A-Z]?)\b",
    );
    m.find(text).map(|x| x.as_str().to_string())
}

/// A vendor named in page text or implied by its API paths. The first match wins.
pub(crate) fn vendor_in(text: &str) -> Option<&'static str> {
    const VENDORS: &[(&str, &[&str])] = &[
        (
            "TP-Link",
            &["tp-link", "tplink", "tl-mr", "cgi_gdpr", "archer mr"],
        ),
        ("Huawei", &["huawei", "hilink", "emui.", "/api/webserver/"]),
        ("ZTE", &["zte", "goform_get_cmd_process"]),
        ("Nokia", &["nokia"]),
        ("Netgear", &["netgear"]),
        ("D-Link", &["d-link", "dlink"]),
        ("Tenda", &["tenda"]),
        ("Xiaomi", &["xiaomi", "miwifi"]),
        ("Zyxel", &["zyxel"]),
        ("Alcatel", &["alcatel"]),
        ("Teltonika", &["teltonika"]),
        ("GL.iNet", &["gl.inet", "gl-inet"]),
        ("OpenWrt", &["openwrt", "luci"]),
        ("MikroTik", &["mikrotik", "routeros"]),
    ];
    let low = text.to_ascii_lowercase();
    VENDORS
        .iter()
        .find(|(_, keys)| keys.iter().any(|k| low.contains(k)))
        .map(|(name, _)| *name)
}

pub(crate) fn title_of(html: &str) -> Option<String> {
    static T: OnceLock<Regex> = OnceLock::new();
    let t = re(&T, r"(?is)<title[^>]*>(.*?)</title>");
    let text = t.captures(html)?.get(1)?.as_str().trim();
    let clean: String = text.chars().filter(|c| !c.is_control()).take(80).collect();
    (!clean.is_empty()).then_some(clean)
}

/// API-looking paths quoted in a page or script: they show which protocol the router speaks.
pub(crate) fn api_paths_in(text: &str, out: &mut BTreeSet<String>) {
    static P: OnceLock<Regex> = OnceLock::new();
    let p = re(
        &P,
        r#"["'](/?(?:api|cgi|cgi-bin|cgi_gdpr|goform|webapi|rpc|ubus|jsonrpc|ajax)(?:[/?][A-Za-z0-9_./-]{0,60})?|[A-Za-z0-9_./-]{1,50}\.cgi)["'?]"#,
    );
    for c in p.captures_iter(text) {
        if out.len() >= MAX_API_PATHS {
            return;
        }
        if let Some(m) = c.get(1) {
            out.insert(m.as_str().to_string());
        }
    }
}

/// Meta refreshes and the scripts a page loads. Relative or absolute; the caller checks each with
/// `followable`. `location = ...` in a script is left out: it may sit in a Reboot button's handler.
fn links_in(html: &str) -> Vec<String> {
    static S: OnceLock<Regex> = OnceLock::new();
    static R: OnceLock<Regex> = OnceLock::new();
    let script = re(&S, r#"(?i)<script[^>]+src\s*=\s*["']([^"']{1,200})["']"#);
    let redirect = re(
        &R,
        r#"(?i)http-equiv=["']?refresh["']?[^>]*url=["']?([^"'>\s)]{1,200})"#,
    );
    let mut out: Vec<String> = redirect
        .captures_iter(html)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect();
    out.extend(
        script
            .captures_iter(html)
            .filter_map(|c| c.get(1).map(|m| m.as_str().to_string())),
    );
    out
}

/// One round of percent-decoding. A `%` not followed by two hex digits stays as it is.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%')
            .then(|| bytes.get(i + 1..i + 3))
            .flatten()
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match hex {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Decoded until nothing changes, so double-encoding can't hide a name either. Each round that
/// changes anything shortens the text, so this ends.
pub(crate) fn fully_decoded(path: &str) -> String {
    let mut s = path.to_string();
    loop {
        let next = percent_decode(&s);
        if next == s {
            return s;
        }
        s = next;
    }
}

/// A path a GET can't plausibly change anything on: a page or script, named harmlessly.
fn harmless_path(path: &str) -> bool {
    const ACTIONS: &[&str] = &[
        "reboot", "restart", "reset", "restore", "factory", "logout", "logoff", "upgrade",
        "update", "delete", "remove", "save", "apply", "cgi", "goform", "/api",
    ];
    // Judged as the server will read it: "/%72eboot.html" is "/reboot.html".
    let low = fully_decoded(path).to_ascii_lowercase();
    let last = low.rsplit('/').next().unwrap_or("");
    let static_kind = match last.rsplit_once('.') {
        Some((_, ext)) => ["html", "htm", "js"].contains(&ext),
        None => true,
    };
    // "set" only at the start of a name: "assets" and "settings" are fine.
    let sets_something = low
        .split('/')
        .any(|s| s.starts_with("set") && !s.starts_with("setting"));
    static_kind && !sets_something && !ACTIONS.iter().any(|a| low.contains(a))
}

/// Where a link from `from` may take the probe: its own address, no query, a harmless path.
fn followable(base: &Url, from: &Url, link: &str) -> Option<String> {
    let mut to = from.join(link).ok()?;
    to.set_query(None);
    to.set_fragment(None);
    (same_origin(base, &to) && harmless_path(to.path())).then(|| to.to_string())
}

fn gunzip(bytes: &[u8]) -> Option<String> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .take(MAX_PAGE_BYTES as u64)
        .read_to_end(&mut out)
        .ok()?;
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// Routers often gzip whatever the request asked for (the H122 does).
pub(crate) fn decode(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        if let Some(text) = gunzip(bytes) {
            return text;
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

struct Fetched {
    status: u16,
    content_type: String,
    location: Option<String>,
    server: Option<String>,
    realm: Option<String>,
    body: String,
    bytes: usize,
}

async fn fetch(client: &reqwest::Client, url: Url, budget: usize) -> Result<Fetched, ()> {
    let mut r = client.get(url).send().await.map_err(|_| ())?;
    let header = |name: &str| {
        r.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let (location, server, auth) = (
        header("location"),
        header("server"),
        header("www-authenticate"),
    );
    let content_type = header("content-type")
        .map(|c| c.split(';').next().unwrap_or("").trim().to_string())
        .unwrap_or_default();
    let status = r.status().as_u16();
    let limit = budget.min(MAX_PAGE_BYTES);
    let mut bytes = Vec::new();
    while let Ok(Some(chunk)) = r.chunk().await {
        bytes.extend_from_slice(&chunk);
        if bytes.len() >= limit {
            break;
        }
    }
    static REALM: OnceLock<Regex> = OnceLock::new();
    let realm = auth.and_then(|a| {
        re(&REALM, r#"(?i)realm="([^"]{1,80})""#)
            .captures(&a)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
    });
    Ok(Fetched {
        status,
        content_type,
        location,
        server,
        realm,
        body: decode(&bytes),
        bytes: bytes.len(),
    })
}

/// Reads the router's public pages. `base` is `http://host` or `http://host:port`.
pub async fn probe(base: &str) -> RouterFacts {
    let mut facts = RouterFacts::default();
    let Ok(base) = Url::parse(base) else {
        return facts;
    };
    // No redirects followed by the client, no cookie store: the probe decides where it goes.
    let Ok(client) = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .user_agent("5g-router-band-tuner")
        .build()
    else {
        return facts;
    };
    let mut queue: VecDeque<String> = FIRST_PATHS.iter().map(|p| (*p).to_string()).collect();
    let mut seen = HashSet::new();
    let mut spent = 0usize;
    let mut requests = 0usize;

    while let Some(next) = queue.pop_front() {
        if requests >= MAX_REQUESTS || spent >= MAX_TOTAL_BYTES {
            break;
        }
        let Ok(url) = base.join(&next) else { continue };
        if !same_origin(&base, &url) || !seen.insert(url.path().to_string()) {
            continue;
        }
        let path = url.path().to_string();
        requests += 1;
        let Ok(page) = fetch(&client, url.clone(), MAX_TOTAL_BYTES - spent).await else {
            continue;
        };
        facts.reachable = true;
        spent += page.bytes;
        facts.pages.push(PageSeen {
            path: path.clone(),
            status: page.status,
            content_type: page.content_type.clone(),
            bytes: page.bytes,
        });
        if facts.server.is_none() {
            facts.server = page.server.clone();
        }
        if facts.auth_realm.is_none() {
            facts.auth_realm = page.realm.clone();
        }
        // Follow a redirect, relative to the page it came from.
        if let Some(to) = &page.location {
            if let Some(t) = followable(&base, &url, to) {
                queue.push_front(t);
            }
        }
        let is_script = path.ends_with(".js") || page.content_type.contains("javascript");
        api_paths_in(&page.body, &mut facts.api_paths);
        if facts.vendor.is_none() {
            let known = [
                page.server.as_deref().unwrap_or(""),
                page.realm.as_deref().unwrap_or(""),
                &page.body,
            ]
            .join(" ");
            facts.vendor = vendor_in(&known).map(str::to_string);
        }
        // Scripts often list models and defaults that aren't this router, so only pages count.
        if !is_script && page.status < 400 {
            let title = title_of(&page.body);
            let named = [
                page.realm.as_deref().unwrap_or(""),
                title.as_deref().unwrap_or(""),
                page.body.as_str(),
            ];
            if facts.model.is_none() {
                facts.model =
                    model_field(&page.body).or_else(|| named.iter().find_map(|t| model_in_text(t)));
            }
            if facts.hardware.is_none() {
                facts.hardware = hardware_field(&page.body);
            }
            if facts.firmware.is_none() {
                facts.firmware = firmware_field(&page.body);
            }
            if let Some(t) = title {
                if facts.titles.len() < MAX_TITLES && !facts.titles.contains(&t) {
                    facts.titles.push(t);
                }
            }
            // Redirects and scripts go first: they're what this router really uses.
            for link in links_in(&page.body).into_iter().rev() {
                if let Some(t) = followable(&base, &url, &link) {
                    queue.push_front(t);
                }
            }
        }
        // A supported model is all this needs to know.
        if facts.support() == Support::Supported {
            break;
        }
    }
    facts
}

/// A version number or model name, which leak scans would mistake for an IP address.
pub(crate) fn is_plain_label(s: &str) -> bool {
    s.len() <= 48
        && !is_mac(s)
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || " ._()/+-".contains(c))
        && !s
            .split(|c: char| !c.is_ascii_digit())
            .any(|run| run.len() >= 14)
}

/// A page's path, without what can identify a session: `;stok=...`, `key=value` and long or random
/// looking segments become `*`.
pub(crate) fn clean_path(path: &str) -> String {
    let segment = |s: &str| {
        let digits = s.bytes().filter(u8::is_ascii_digit).count();
        let random = s.len() >= 16 && digits >= 4;
        if s.len() > 32 || random || s.contains([';', '=', '%', '~']) {
            "*"
        } else {
            s
        }
        .to_string()
    };
    path.split('/').map(segment).collect::<Vec<_>>().join("/")
}

/// The redacted report. Each free-text value that looks like an identifier is dropped and counted.
/// `Err` lists what the final scan found in the finished file: nothing may be written then.
pub fn report(
    facts: &RouterFacts,
    app_version: &str,
    generated_at_ms: u64,
) -> Result<String, Vec<String>> {
    let scan = Redactor::new();
    let mut dropped = 0usize;
    let mut keep = |s: &str| -> Option<String> {
        if scan.leaks(s).is_empty() {
            Some(s.to_string())
        } else {
            dropped += 1;
            None
        }
    };
    let titles: Vec<String> = facts.titles.iter().filter_map(|t| keep(t)).collect();
    let server = facts.server.as_deref().and_then(&mut keep);
    let realm = facts.auth_realm.as_deref().and_then(&mut keep);
    let api_paths: Vec<String> = facts.api_paths.iter().filter_map(|p| keep(p)).collect();
    let pages: Vec<_> = facts
        .pages
        .iter()
        .filter_map(|p| keep(&clean_path(&p.path)).map(|path| PageSeen { path, ..p.clone() }))
        .collect();
    let model = facts.model.as_deref().and_then(&mut keep);
    let vendor = facts.vendor.as_deref().and_then(&mut keep);
    let body = json!({
        "format": REPORT_FORMAT,
        "appVersion": app_version,
        "generatedAtMs": generated_at_ms,
        "support": facts.support(),
        "vendor": vendor,
        "model": model,
        "titles": titles,
        "server": server,
        "authRealm": realm,
        "pages": pages,
        "apiPaths": api_paths,
        "droppedForPrivacy": dropped,
    });
    let scanned = serde_json::to_string_pretty(&body).map_err(|e| vec![e.to_string()])?;
    let leaks = scan.leaks(&scanned);
    if !leaks.is_empty() {
        return Err(leaks);
    }
    // Versions look like IP addresses, so they join after the scan, with their own rule.
    let label = |s: &Option<String>| {
        s.as_deref()
            .filter(|v| is_plain_label(v))
            .map(str::to_string)
    };
    let mut body = body;
    body["hardware"] = json!(label(&facts.hardware));
    body["firmware"] = json!(label(&facts.firmware));
    serde_json::to_string_pretty(&body).map_err(|e| vec![e.to_string()])
}

/// The address of the New router form, with what's known filled in. Values go through the URL
/// encoder, and only ones that pass the label rule.
pub fn issue_url(view: &ProbeView, form: &str) -> String {
    let mut url = Url::parse(form).expect("a valid form address");
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("template", "new-router.yml");
        let model = view.model.as_deref().filter(|v| is_plain_label(v));
        let vendor = view.vendor.as_deref().filter(|v| is_plain_label(v));
        let name = [vendor, model]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        if !name.is_empty() {
            q.append_pair("title", &format!("Router support: {name}"));
        }
        if let Some(m) = model {
            q.append_pair("model", m);
        }
        if let Some(h) = view.hardware.as_deref().filter(|v| is_plain_label(v)) {
            q.append_pair("hardware", h);
        }
        if let Some(f) = view.firmware.as_deref().filter(|v| is_plain_label(v)) {
            q.append_pair("firmware", f);
        }
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_model_and_versions_from_tags_and_assignments() {
        assert_eq!(
            model_field("<devicename>H122-373</devicename>").as_deref(),
            Some("H122-373")
        );
        assert_eq!(
            model_field(r#"var modelName = "TL-MR6400";"#).as_deref(),
            Some("TL-MR6400")
        );
        assert_eq!(
            model_field(r#"{"model_name":"NE200"}"#).as_deref(),
            Some("NE200")
        );
        assert_eq!(
            hardware_field(r#"hardwareVersion: 'MR6400 v5.0'"#).as_deref(),
            Some("MR6400 v5.0")
        );
        assert_eq!(
            firmware_field("<SoftwareVersion>11.0.2.21</SoftwareVersion>").as_deref(),
            Some("11.0.2.21")
        );
        // Placeholders and templates aren't values.
        assert_eq!(model_field(r#"modelName = "undefined""#), None);
        assert_eq!(model_field(r#"modelName = "{{model}}""#), None);
        assert_eq!(model_field(r#"modelName = someVariable + 1"#), None);
        assert_eq!(model_field("<devicename></devicename>"), None);
    }

    #[test]
    fn spots_model_names_in_text() {
        assert_eq!(
            model_in_text("TP-Link TL-MR6400 login").as_deref(),
            Some("TL-MR6400")
        );
        assert_eq!(
            model_in_text("Archer MR600 v2").as_deref(),
            Some("Archer MR600")
        );
        assert_eq!(
            model_in_text("HUAWEI H158-381").as_deref(),
            Some("H158-381")
        );
        assert_eq!(model_in_text("Welcome to your router"), None);
    }

    #[test]
    fn names_a_vendor_from_text_or_its_api() {
        assert_eq!(vendor_in("x cgi_gdpr y"), Some("TP-Link"));
        assert_eq!(vendor_in("HUAWEI"), Some("Huawei"));
        assert_eq!(vendor_in("nothing here"), None);
    }

    #[test]
    fn only_the_h122_family_counts_as_supported() {
        assert!(is_supported_model("H122-373"));
        assert!(is_supported_model(" h122-373 "));
        assert!(!is_supported_model("H158-381"));
        let mut f = RouterFacts::default();
        assert_eq!(f.support(), Support::Unreachable);
        f.reachable = true;
        assert_eq!(f.support(), Support::Unknown);
        f.model = Some("TL-MR6400".into());
        assert_eq!(f.support(), Support::Unknown);
        f.model = Some("H122-373".into());
        assert_eq!(f.support(), Support::Supported);
    }

    fn facts() -> RouterFacts {
        RouterFacts {
            reachable: true,
            vendor: Some("TP-Link".into()),
            model: Some("TL-MR6400".into()),
            hardware: Some("MR6400 v5.0".into()),
            firmware: Some("1.2.0.84".into()),
            titles: vec!["TP-Link".into()],
            api_paths: ["/cgi_gdpr".to_string()].into_iter().collect(),
            ..RouterFacts::default()
        }
    }

    #[test]
    fn the_report_has_the_facts_and_a_version_is_not_taken_for_an_address() {
        let text = report(&facts(), "0.2.0", 7).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["format"], REPORT_FORMAT);
        assert_eq!(v["support"], "unknown");
        assert_eq!(v["model"], "TL-MR6400");
        assert_eq!(v["hardware"], "MR6400 v5.0");
        assert_eq!(v["firmware"], "1.2.0.84");
        assert_eq!(v["apiPaths"][0], "/cgi_gdpr");
        assert_eq!(v["droppedForPrivacy"], 0);
    }

    /// A public page can hold a MAC, an IMEI or the WAN address. None of it gets out.
    #[test]
    fn values_that_look_like_identifiers_are_dropped_and_counted() {
        let mac = "AA:BB:CC:DD:EE:01";
        let imei = format!("35{}", "1".repeat(13));
        let mut f = facts();
        f.titles = vec![
            format!("Router {mac}"),
            format!("IMEI {imei}"),
            "Fine".into(),
        ];
        f.server = Some("203.0.113.9".into());
        f.firmware = Some(format!("build {imei}"));
        let text = report(&f, "0.2.0", 7).unwrap();
        assert!(!text.contains(mac));
        assert!(!text.contains(&imei));
        assert!(!text.contains("203.0.113.9"));
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["titles"], json!(["Fine"]));
        assert_eq!(v["droppedForPrivacy"], 3);
        assert!(v["firmware"].is_null());
    }

    #[test]
    fn links_a_page_only_mentions_are_never_followed() {
        let page = r#"<script src="/js/app.js?v=1"></script>
            <button onclick="location.href='/cgi-bin/reboot.cgi?do=1'">Reboot</button>
            <script>location.replace('/logout.html'); window.location = '/x.html'</script>
            <meta http-equiv="refresh" content="0;url=/login.html">"#;
        let links = links_in(page);
        assert!(links.contains(&"/js/app.js?v=1".to_string()));
        assert!(links.contains(&"/login.html".to_string()));
        assert_eq!(links.len(), 2, "{links:?}");
    }

    #[test]
    fn only_harmless_paths_on_the_same_router_are_followed() {
        let base = Url::parse("http://192.168.0.1").unwrap();
        let go = |link: &str| followable(&base, &base, link);
        assert_eq!(
            go("/js/app.js?v=1#x").as_deref(),
            Some("http://192.168.0.1/js/app.js")
        );
        for fine in [
            "/login.html",
            "/assets/main.js",
            "/settings.html",
            "/webpages/",
            "/caf%C3%A9.html",
            "/100%.html",
        ] {
            assert!(go(fine).is_some(), "{fine}");
        }
        for risky in [
            "/%72eboot.html",
            "/%2572eboot.html",
            "/re%42oot.html",
            "/%61pi/device/control",
            "/%73et_mode.html",
            "/page%2Ecgi",
            "/cgi-bin/reboot.cgi",
            "/reboot.html",
            "/factory_reset",
            "/logout",
            "/goform/x",
            "/api/device/control",
            "/setconfig",
            "/backup.bin",
            "http://evil.example/login.html",
        ] {
            assert_eq!(go(risky), None, "{risky}");
        }
    }

    #[test]
    fn a_session_in_a_path_stays_out_of_the_report() {
        let mut f = facts();
        f.pages = vec![PageSeen {
            path: "/;stok=abc123/admin/a1b2c3d4e5f6a7b8/index.html".into(),
            status: 200,
            content_type: "text/html".into(),
            bytes: 1,
        }];
        let v: serde_json::Value = serde_json::from_str(&report(&f, "1", 7).unwrap()).unwrap();
        assert_eq!(v["pages"][0]["path"], "/*/admin/*/index.html");
    }

    #[test]
    fn a_mac_is_not_a_label() {
        let mut f = facts();
        f.hardware = Some("AA-BB-CC-DD-EE-01".into());
        let v: serde_json::Value = serde_json::from_str(&report(&f, "1", 7).unwrap()).unwrap();
        assert!(v["hardware"].is_null());
    }

    #[test]
    fn the_issue_link_is_encoded_and_carries_only_clean_labels() {
        let form = "https://github.com/Hjbiki/5g-router-band-tuner/issues/new";
        let url = issue_url(&facts().view(), form);
        assert!(url.starts_with(form));
        assert!(url.contains("template=new-router.yml"));
        assert!(url.contains("model=TL-MR6400"));
        assert!(url.contains("firmware=1.2.0.84"));
        assert!(url.contains("title=Router+support%3A+TP-Link+TL-MR6400"));

        let mut odd = facts().view();
        odd.model = Some("x&body=<script>".into());
        let url = issue_url(&odd, form);
        assert!(!url.contains("script"));
        assert!(!url.contains("model="));
    }

    #[test]
    fn gzip_bodies_are_read() {
        use std::io::Write;
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(b"<title>Gz</title>").unwrap();
        assert_eq!(decode(&e.finish().unwrap()), "<title>Gz</title>");
        assert_eq!(decode(b"plain"), "plain");
    }
}
