//! The built-in capture: what a router's own web page asked the router, cleaned for sharing.
//!
//! A driver for a new router needs to see how that router's page logs in, reads the signal and
//! changes bands. `Recorder` takes the browser's network events as they happen (the DevTools
//! protocol's JSON, nothing else) and keeps only what the page asked the router itself. `build`
//! turns that into the file a person can attach to an issue.
//!
//! The file is meant to be public, and the router's page chooses every name and value in it, so
//! this is the part that has to be right:
//!
//! - A header's value is never stored, only its name. The one exception is the media type of
//!   `Content-Type`, kept when it has a media type's shape.
//! - A request to any other host is never stored.
//! - A login exchange keeps its names and hides every value, the length too
//!   (`Rules::ShapeOnly`). A password's hash can be as sensitive as the password.
//! - Every other body is parsed and goes through `Rules::Capture`: a value stays only under a name
//!   known to be technical, and only when it reads like a reading or a state. A name that doesn't
//!   read like a field's name is treated as data. A body that is neither XML, JSON nor a form is
//!   left out.
//! - A page's markup is never kept: routers write live values into it. A script is the router
//!   maker's code: left out unless the user asks, and then only a `.js` file that isn't part of a
//!   login and holds nothing that was hidden elsewhere.
//! - The finished file is scanned, and a hit means nothing is written.
//!
//! What this can't promise: a short private word under a technical name (`"result": "hibi"`)
//! reads exactly like a state. The person sees the file before saving it.

use std::collections::{BTreeSet, HashMap};
use std::fmt;

use base64::Engine as _;
use reqwest::Url;
use serde::Serialize;
use serde_json::{json, Map, Value};
use ts_rs::TS;

use crate::diagnostics::{identifier_hits, redact_identifier_tokens, Redactor, Rules};
use crate::probe;

/// Format tag of the saved file.
pub const FORMAT: &str = "router-band-tuner-capture/1";
pub const MAX_EXCHANGES: usize = 500;
pub const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: usize = 20 * 1024 * 1024;
/// A longer address than this isn't kept: only that there was a request.
const MAX_URL_BYTES: usize = 8 * 1024;
const MAX_HEADERS: usize = 64;
const MAX_HEADER_NAME: usize = 64;

const NOTE_STATIC: &str = "a static file: no body kept";
const NOTE_REDIRECT: &str = "a redirect: its own body isn't available";
const NOTE_UNFINISHED: &str = "the page left before this answer finished";
const NOTE_MISSING: &str = "the body wasn't available";
const NOTE_TOO_LARGE: &str = "larger than 2 MiB: left out";
const NOTE_PAGE: &str = "a page: its markup is never kept";
const NOTE_CODE: &str = "the router maker's code: left out";
const NOTE_CODE_HELD: &str = "left out: it holds a value that was hidden elsewhere";
const OTHER_HOST: &str = "(another address)";
const TOO_LONG: &str = "(too long)";
const MAX_API_PATHS: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// A document the window showed.
    Page,
    Script,
    /// Whatever the page asked for by itself: where the router's API is.
    Api,
    /// A stylesheet, picture, font or media file.
    Static,
}

impl Kind {
    fn from_cdp(kind: &str) -> Self {
        match kind {
            "Document" => Kind::Page,
            "Script" => Kind::Script,
            "Stylesheet" | "Image" | "Media" | "Font" | "TextTrack" | "Manifest" => Kind::Static,
            _ => Kind::Api,
        }
    }
}

/// Why a recording stopped taking more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Limit {
    Requests,
    Size,
    Time,
}

/// What the recorder needs the browser to fetch for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    ResponseBody(String),
    RequestBody(String),
}

/// One request the page made to the router, and the router's answer, as recorded. Nothing here is
/// cleaned yet, except that header values were never taken.
#[derive(Clone, PartialEq, Eq)]
pub struct Exchange {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub kind: Kind,
    /// Names only.
    pub request_headers: Vec<String>,
    pub request_type: Option<String>,
    pub request_body: Option<String>,
    /// The page sent a body, and it never reached the recorder.
    pub request_body_missing: bool,
    pub status: Option<u16>,
    pub content_type: Option<String>,
    /// Names only.
    pub response_headers: Vec<String>,
    pub body: Option<String>,
    /// Why there is no body, when there is none.
    pub note: Option<&'static str>,
    pub redirect_to: Option<String>,
}

/// What was recorded holds passwords until `build` cleans it, so it never prints its contents.
impl fmt::Debug for Exchange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Exchange")
            .field("method", &self.method)
            .field("path", &self.path)
            .field("kind", &self.kind)
            .field("status", &self.status)
            .field("query_fields", &self.query.len())
            .field("sent_bytes", &self.request_body.as_ref().map(String::len))
            .field("answer_bytes", &self.body.as_ref().map(String::len))
            .field("note", &self.note)
            .finish()
    }
}

fn header_names(headers: &Value) -> Vec<String> {
    let mut names: Vec<String> = headers
        .as_object()
        .map(|m| {
            m.keys()
                .filter(|k| k.len() <= MAX_HEADER_NAME)
                .take(MAX_HEADERS)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    names.sort_by_key(|n| n.to_ascii_lowercase());
    names
}

/// A media type like `text/xml`, lowercased, without its parameters. Anything of another shape
/// is page-chosen text, and isn't kept.
fn media_type(raw: &str) -> Option<String> {
    let kind = raw.split(';').next()?.trim().to_ascii_lowercase();
    let part = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || ".+-".contains(c))
    };
    let (main, sub) = kind.split_once('/')?;
    (kind.len() <= 64 && part(main) && part(sub)).then_some(kind)
}

fn content_type(headers: &Value) -> Option<String> {
    let (_, v) = headers
        .as_object()?
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))?;
    media_type(v.as_str()?)
}

/// The usual methods as they are; anything else is page-chosen text.
fn method(raw: &str) -> String {
    const USUAL: &[&str] = &["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"];
    let upper = raw.to_ascii_uppercase();
    if USUAL.contains(&upper.as_str()) {
        upper
    } else {
        "OTHER".to_string()
    }
}

/// Keeps what the router's own page asked the router. Feed it every `Network.*` event.
pub struct Recorder {
    origin: String,
    list: Vec<Exchange>,
    /// A request id's newest exchange. A redirect keeps the id and starts a new exchange.
    open: HashMap<String, usize>,
    /// The exchange a sent body was asked for: a redirect may move the id on before it arrives.
    awaiting_sent: HashMap<String, usize>,
    bytes: usize,
    limit: Option<Limit>,
}

impl Recorder {
    /// `None` for an address that isn't a web address: its origin would match anything opaque.
    pub fn new(router: &Url) -> Option<Self> {
        if !matches!(router.scheme(), "http" | "https") || router.host_str().is_none() {
            return None;
        }
        Some(Self {
            origin: router.origin().ascii_serialization(),
            list: Vec::new(),
            open: HashMap::new(),
            awaiting_sent: HashMap::new(),
            bytes: 0,
            limit: None,
        })
    }

    fn ours(&self, url: &str) -> Option<Url> {
        let url = Url::parse(url).ok()?;
        let web = matches!(url.scheme(), "http" | "https");
        (web && url.origin().ascii_serialization() == self.origin).then_some(url)
    }

    /// One `Network.*` event, as the JSON the browser gave. Returns what to ask the browser for.
    pub fn event(&mut self, name: &str, json: &str) -> Option<Ask> {
        let v: Value = serde_json::from_str(json).ok()?;
        let id = v["requestId"].as_str()?.to_string();
        match name {
            "Network.requestWillBeSent" => self.sent(id, &v),
            "Network.responseReceived" => {
                let x = self.list.get_mut(*self.open.get(&id)?)?;
                let response = &v["response"];
                x.status = response["status"]
                    .as_u64()
                    .and_then(|s| u16::try_from(s).ok());
                x.content_type = response["mimeType"].as_str().and_then(media_type);
                x.response_headers = header_names(&response["headers"]);
                None
            }
            "Network.loadingFinished" => {
                let x = self.list.get_mut(*self.open.get(&id)?)?;
                if x.kind == Kind::Static {
                    x.note = Some(NOTE_STATIC);
                    return None;
                }
                (x.body.is_none() && x.note.is_none()).then_some(Ask::ResponseBody(id))
            }
            "Network.loadingFailed" => {
                let x = self.list.get_mut(*self.open.get(&id)?)?;
                if x.body.is_none() {
                    x.note = Some(NOTE_UNFINISHED);
                }
                None
            }
            _ => None,
        }
    }

    fn sent(&mut self, id: String, v: &Value) -> Option<Ask> {
        let request = &v["request"];
        let to = request["url"].as_str()?;
        // A redirect reuses the request's id: the exchange so far was answered by the redirect.
        if let Some(redirect) = v.get("redirectResponse").filter(|r| r.is_object()) {
            if let Some(x) = self.open.get(&id).and_then(|i| self.list.get_mut(*i)) {
                x.status = redirect["status"]
                    .as_u64()
                    .and_then(|s| u16::try_from(s).ok());
                x.response_headers = header_names(&redirect["headers"]);
                x.note = Some(NOTE_REDIRECT);
                x.redirect_to = Some(match Url::parse(to) {
                    _ if to.len() > MAX_URL_BYTES => TOO_LONG.to_string(),
                    Ok(u) if u.origin().ascii_serialization() == self.origin => {
                        u.path().to_string()
                    }
                    _ => OTHER_HOST.to_string(),
                });
            }
        }
        self.open.remove(&id);
        // Another host: nothing about it is kept.
        let url = self.ours(to)?;
        if self.limit.is_some() {
            return None;
        }
        if self.list.len() >= MAX_EXCHANGES {
            self.limit = Some(Limit::Requests);
            return None;
        }
        let (path, query) = if to.len() > MAX_URL_BYTES {
            (TOO_LONG.to_string(), Vec::new())
        } else {
            let query = url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            (url.path().to_string(), query)
        };
        let mut x = Exchange {
            method: method(request["method"].as_str().unwrap_or("GET")),
            path,
            query,
            kind: Kind::from_cdp(v["type"].as_str().unwrap_or("")),
            request_headers: header_names(&request["headers"]),
            request_type: content_type(&request["headers"]),
            request_body: None,
            request_body_missing: false,
            status: None,
            content_type: None,
            response_headers: Vec::new(),
            body: None,
            note: None,
            redirect_to: None,
        };
        // The address and the names count against the size limit too.
        let held = x.path.len()
            + x.query
                .iter()
                .map(|(k, v)| k.len() + v.len())
                .sum::<usize>()
            + x.request_headers.iter().map(String::len).sum::<usize>();
        if !self.charge(held) {
            return None;
        }
        let mut ask = None;
        if let Some(body) = request["postData"].as_str() {
            x.request_body = self.take(body.to_string());
            x.request_body_missing = x.request_body.is_none();
        } else if request["hasPostData"].as_bool() == Some(true) {
            x.request_body_missing = true;
            self.awaiting_sent.insert(id.clone(), self.list.len());
            ask = Some(Ask::RequestBody(id.clone()));
        }
        self.open.insert(id, self.list.len());
        self.list.push(x);
        ask
    }

    /// Counts bytes against the whole recording's limit. `false` when it is full.
    fn charge(&mut self, len: usize) -> bool {
        if self.limit.is_some() {
            return false;
        }
        if self.bytes + len > MAX_TOTAL_BYTES {
            self.limit = Some(Limit::Size);
            return false;
        }
        self.bytes += len;
        true
    }

    /// Counts a body against the limits. `None` when it is too large or the recording is full.
    fn take(&mut self, text: String) -> Option<String> {
        (text.len() <= MAX_BODY_BYTES && self.charge(text.len())).then_some(text)
    }

    /// The browser's answer to `Network.getResponseBody` for this request.
    pub fn response_body(&mut self, id: &str, json: &str) {
        let Some(&i) = self.open.get(id) else { return };
        let (text, large) = match serde_json::from_str::<Value>(json) {
            Ok(v) => match v["body"].as_str() {
                // Checked before decoding: base64 is a third longer than what it holds.
                Some(body) if body.len() > MAX_BODY_BYTES / 3 * 4 + 4 => (None, true),
                Some(body) if v["base64Encoded"].as_bool() == Some(true) => {
                    let bytes = base64::engine::general_purpose::STANDARD.decode(body).ok();
                    // Routers send gzip without saying so (the H122 does), and the browser hands
                    // the raw bytes over.
                    (bytes.map(|b| probe::decode(&b)), false)
                }
                Some(body) => (Some(body.to_string()), false),
                None => (None, false),
            },
            Err(_) => (None, false),
        };
        let (body, note) = match text {
            None if large => (None, NOTE_TOO_LARGE),
            None => (None, NOTE_MISSING),
            Some(t) if t.len() > MAX_BODY_BYTES => (None, NOTE_TOO_LARGE),
            Some(t) => (self.take(t), NOTE_MISSING),
        };
        if let Some(x) = self.list.get_mut(i) {
            x.note = body.is_none().then_some(note);
            x.body = body;
        }
    }

    /// The browser's answer to `Network.getRequestPostData` for this request.
    pub fn request_body(&mut self, id: &str, json: &str) {
        let Some(i) = self.awaiting_sent.remove(id) else {
            return;
        };
        let Some(text) = serde_json::from_str::<Value>(json)
            .ok()
            .and_then(|v| v["postData"].as_str().map(str::to_string))
        else {
            return;
        };
        let kept = self.take(text);
        if let Some(x) = self.list.get_mut(i) {
            x.request_body_missing = kept.is_none();
            x.request_body = kept;
        }
    }

    /// Stops taking more: the caller's clock ran out.
    pub fn stop(&mut self, limit: Limit) {
        self.limit.get_or_insert(limit);
    }

    pub fn limit(&self) -> Option<Limit> {
        self.limit
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn into_exchanges(self) -> Vec<Exchange> {
        self.list
    }
}

pub struct BuildOptions<'a> {
    pub app_version: &'a str,
    pub generated_at_ms: u64,
    /// The user's choice: scripts are the router maker's code.
    pub include_code: bool,
    pub limit: Option<Limit>,
}

/// The names a request sends, and what it sends under a name that picks a command.
#[derive(Default)]
struct Sent {
    names: Vec<String>,
    commands: Vec<String>,
    /// A body that is neither a form, JSON nor XML: judged as text.
    unread: String,
}

/// A name whose value says which command this is: `goformId`, `method`, `action`, `cmd`.
fn picks_command(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        "goform", "cmd", "method", "action", "command", "func", "module", "object",
    ]
    .iter()
    .any(|p| n.contains(p))
}

fn json_parts(v: &Value, key: &str, sent: &mut Sent) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                sent.names.push(k.clone());
                json_parts(x, k, sent);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| json_parts(x, key, sent)),
        // JSON-RPC names its object and its method in a bare list.
        Value::String(s) if picks_command(key) || key == "params" => sent.commands.push(s.clone()),
        _ => {}
    }
}

fn sent_parts(x: &Exchange) -> Sent {
    let mut sent = Sent::default();
    for (k, v) in &x.query {
        sent.names.push(k.clone());
        if picks_command(k) {
            sent.commands.push(v.clone());
        }
    }
    let Some(body) = x.request_body.as_deref() else {
        return sent;
    };
    if let Some(fields) = form_fields(body) {
        for (k, v) in fields {
            if let (true, Some(v)) = (picks_command(&k), v.as_str()) {
                sent.commands.push(v.to_string());
            }
            sent.names.push(k);
        }
    } else if let Ok(v) = serde_json::from_str::<Value>(body) {
        json_parts(&v, "", &mut sent);
    } else if body.trim_start().starts_with('<') {
        // Element names are enough: what an element holds is judged by the redactor.
        for tag in body.split('<').skip(1) {
            let name: String = tag
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || "_:.-".contains(*c))
                .collect();
            if !name.is_empty() {
                sent.names.push(name);
            }
        }
    } else {
        sent.unread = body.to_string();
    }
    sent
}

/// A login exchange. Judged widely on purpose: a wrong yes only hides values, a wrong no could
/// publish a password's hash. It is judged by the path, by the *names* the request sends, and by
/// the command it picks, never by the other values: a status poll that asks for a field called
/// `loginfo` is not a login, and a band command that carries a token is not one either (the
/// token is hidden by its own name). A sent body that never arrived can't be judged, so it counts
/// as a login.
fn login_shaped(x: &Exchange) -> bool {
    const IN_PATH: &[&str] = &[
        "login",
        "logon",
        "signin",
        "sign_in",
        "auth",
        "passw",
        "challenge",
        "nonce",
        "proof",
        "salt",
        "token",
        "sestok",
        "session",
        "credential",
        "pubkey",
        "publickey",
        "captcha",
    ];
    const IN_NAME: &[&str] = &[
        "pass",
        "pwd",
        "psw",
        "psd",
        "login",
        "logon",
        "signin",
        "nonce",
        "proof",
        "salt",
        "challenge",
        "credential",
        "auth",
        "hash",
        "digest",
        "secret",
        "otp",
    ];
    const IN_COMMAND: &[&str] = &[
        "login",
        "logon",
        "signin",
        "sign_in",
        "auth",
        "passw",
        "challenge",
    ];
    if x.request_body_missing {
        return true;
    }
    let path = probe::fully_decoded(&x.path).to_ascii_lowercase();
    if IN_PATH.iter().any(|w| path.contains(w)) {
        return true;
    }
    let sent = sent_parts(x);
    let named = sent.names.iter().any(|name| {
        let n = name.to_ascii_lowercase();
        n == "pw" || IN_NAME.iter().any(|w| n.contains(w))
    });
    // A list of field names (`cmd=rsrp,loginfo`) asks for readings; one word picks a command.
    let picked = sent.commands.iter().filter(|c| !c.contains(',')).any(|c| {
        let c = c.to_ascii_lowercase();
        IN_COMMAND.iter().any(|w| c.contains(w))
    });
    let unread = sent.unread.to_ascii_lowercase();
    named || picked || IN_NAME.iter().chain(IN_COMMAND).any(|w| unread.contains(w))
}

/// `a=1&b=2`, by its look alone: what a request declares itself to be is the page's word (jQuery
/// calls everything a form). XML and JSON are read as what they are, every pair needs its `=`,
/// and every key must read like a form's field, because a key is copied to the file as a name.
fn form_fields(body: &str) -> Option<Map<String, Value>> {
    let text = body.trim();
    if text.is_empty() || text.starts_with(['{', '[', '<']) {
        return None;
    }
    let pairs: Vec<&str> = text.split('&').collect();
    let looks = !text.contains(char::is_whitespace) && pairs.iter().all(|pair| pair.contains('='));
    // One word and a trailing `=` is a bare value, or base64 with its padding.
    let bare = pairs.len() == 1 && text.ends_with('=');
    let field = |key: &str| {
        (1..=64).contains(&key.len())
            && key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_.-[]".contains(c))
    };
    let keys_fit = pairs
        .iter()
        .all(|pair| field(pair.split('=').next().unwrap_or("")));
    if !looks || bare || !keys_fit {
        return None;
    }
    // The URL parser undoes the percent-encoding the same way a server would.
    let url = Url::parse(&format!("http://form.invalid/?{text}")).ok()?;
    Some(
        url.query_pairs()
            .map(|(k, v)| (k.into_owned(), Value::String(v.into_owned())))
            .collect(),
    )
}

/// A header's name as the file shows it. The page chooses its headers' names, so one that
/// doesn't read like a header's name, or that was hidden as a value, is starred.
fn shown_header(red: &Redactor, name: &str) -> String {
    let fits = (1..=MAX_HEADER_NAME).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if fits && identifier_hits(name).is_empty() && !red.holds_secret(name) {
        name.to_string()
    } else {
        "*".to_string()
    }
}

/// A path as the file shows it, a segment at a time: no session and no random token
/// (`clean_path`), no folder named in capitals only (how some routers put the session in the
/// path), no long number, and nothing that was hidden as a value somewhere in the recording
/// (`/api/hosts/Hibi-Laptop`).
fn shown_path(red: &Redactor, path: &str) -> String {
    let cleaned = probe::clean_path(path);
    let segment = |s: &str| {
        let folder = s.len() >= 12 && s.chars().all(|c| c.is_ascii_uppercase());
        let digits = s.bytes().filter(u8::is_ascii_digit).count();
        let number = s.len() >= 5 && digits == s.len();
        // Three digits among letters with nothing joining them is a token, not a route's word.
        let token = digits >= 3 && digits < s.len() && !s.contains(['_', '-', '.']);
        let stem = s.rsplit_once('.').map_or(s, |(stem, _)| stem);
        if folder || number || token || red.hidden_word(s) || red.hidden_word(stem) {
            "*".to_string()
        } else {
            s.to_string()
        }
    };
    let shown = cleaned
        .split('/')
        .map(segment)
        .collect::<Vec<_>>()
        .join("/");
    redact_identifier_tokens(&shown).0
}

/// What the recording says the router is. Joined to the file after the scan, by the label rule,
/// because versions look like addresses.
#[derive(Default)]
struct Facts {
    vendor: Option<&'static str>,
    model: Option<String>,
    hardware: Option<String>,
    firmware: Option<String>,
}

impl Facts {
    fn read(&mut self, x: &Exchange) {
        let Some(text) = x.body.as_deref() else {
            return;
        };
        if self.vendor.is_none() {
            self.vendor = probe::vendor_in(text);
        }
        if x.kind == Kind::Script {
            return;
        }
        if self.model.is_none() {
            // A list of the devices on the network has "device name" fields too, so that name
            // counts only when its value is shaped like a router's model.
            let loose = probe::model_field(text).filter(|m| probe::model_in_text(m).is_some());
            // A bare "model" is in lists of household devices too ("iPhone 12"), so it counts
            // only in an answer that also gives a version.
            let versioned = probe::hardware_field(text).or_else(|| probe::firmware_field(text));
            let bare = probe::bare_model_field(text).filter(|_| versioned.is_some());
            self.model = probe::strict_model_field(text).or(bare).or(loose);
        }
        if self.hardware.is_none() {
            self.hardware = probe::hardware_field(text);
        }
        if self.firmware.is_none() {
            self.firmware = probe::firmware_field(text);
        }
    }

    /// A model or a version: a plain label, and nothing the scan would call an identifier except
    /// that a version reads like an IPv4 address.
    fn label(value: &Option<String>) -> Option<String> {
        value
            .as_deref()
            .filter(|v| probe::is_plain_label(v))
            .filter(|v| identifier_hits(v).iter().all(|hit| hit.contains("IPv4")))
            .map(str::to_string)
    }
}

/// The names at the top of a JSON answer: the fields a request's `cmd` list may name.
fn answer_names(x: &Exchange) -> BTreeSet<String> {
    x.body
        .as_deref()
        .and_then(|b| serde_json::from_str::<Value>(b).ok())
        .and_then(|v| v.as_object().map(|m| m.keys().cloned().collect()))
        .unwrap_or_default()
}

/// A script the router keeps as a file: nothing after its name, and a header that says when it
/// was last changed. A script made on the spot (JSONP, a generated `status.js`) holds live values.
fn kept_file(x: &Exchange) -> bool {
    let dated = x
        .response_headers
        .iter()
        .any(|h| h.eq_ignore_ascii_case("etag") || h.eq_ignore_ascii_case("last-modified"));
    x.kind == Kind::Script
        && x.path.to_ascii_lowercase().ends_with(".js")
        && x.query.is_empty()
        && dated
}

/// One exchange for the file, and its script's text when that may still be included.
fn entry<'a>(
    red: &mut Redactor,
    n: usize,
    x: &'a Exchange,
    include_code: bool,
) -> (Value, Option<&'a str>) {
    let label = format!("x{n}");
    let login = login_shaped(x);
    red.set_rules(if login {
        Rules::ShapeOnly
    } else {
        Rules::Capture
    });
    red.set_answer_names(answer_names(x));

    let mut out = Map::new();
    out.insert("n".into(), json!(n));
    out.insert("method".into(), json!(x.method));
    out.insert("path".into(), json!(shown_path(red, &x.path)));
    out.insert("kind".into(), json!(x.kind));
    if login {
        out.insert("login".into(), json!(true));
    }
    // What the page sends.
    red.set_sent(true);
    if !x.query.is_empty() {
        let fields: Map<String, Value> = x
            .query
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        out.insert(
            "query".into(),
            red.json(&Value::Object(fields), &format!("{label}.query")),
        );
    }
    let headers = |red: &Redactor, list: &[String]| -> Vec<String> {
        list.iter().map(|h| shown_header(red, h)).collect()
    };
    out.insert(
        "requestHeaders".into(),
        json!(headers(red, &x.request_headers)),
    );
    if let Some(kind) = &x.request_type {
        out.insert("requestType".into(), json!(kind));
    }
    if let Some(body) = &x.request_body {
        let at = format!("{label}.request");
        let sent = match form_fields(body) {
            Some(fields) => red.json(&Value::Object(fields), &at),
            None => red.body(body, &at),
        };
        out.insert("request".into(), sent);
    }
    // What the router answers.
    red.set_sent(false);
    out.insert("status".into(), json!(x.status));
    if let Some(kind) = &x.content_type {
        out.insert("type".into(), json!(kind));
    }
    out.insert(
        "responseHeaders".into(),
        json!(headers(red, &x.response_headers)),
    );
    if let Some(to) = &x.redirect_to {
        out.insert("redirectTo".into(), json!(shown_path(red, to)));
    }

    let mut note = x.note;
    let mut code = None;
    match (x.kind, x.body.as_deref()) {
        (Kind::Api, Some(body)) => {
            out.insert(
                "response".into(),
                red.body(body, &format!("{label}.response")),
            );
        }
        (Kind::Page | Kind::Script, Some(text)) => {
            let mut paths = BTreeSet::new();
            probe::api_paths_in(text, &mut paths);
            let paths: Vec<String> = paths
                .iter()
                .take(MAX_API_PATHS)
                .map(|p| shown_path(red, p))
                .collect();
            out.insert("apiPaths".into(), json!(paths));
            note = Some(if x.kind == Kind::Page {
                NOTE_PAGE
            } else {
                NOTE_CODE
            });
            if include_code && kept_file(x) && !login {
                code = Some(text);
            }
        }
        _ => {}
    }
    if let Some(note) = note {
        out.insert("note".into(), json!(note));
    }
    red.set_rules(Rules::Capture);
    (Value::Object(out), code)
}

/// The cleaned file. `Err` lists what the last scan found in it: nothing may be written then.
///
/// The recording is cleaned twice. The first pass only learns what gets hidden; the second,
/// knowing that, never keeps in one place a value that is hidden in another, in whatever order
/// the page asked for things. So the same text is hidden everywhere, instead of the save being
/// refused because of it.
pub fn build(exchanges: &[Exchange], opts: &BuildOptions<'_>) -> Result<String, Vec<String>> {
    let mut first = Redactor::with_rules(Rules::Capture);
    for (i, x) in exchanges.iter().enumerate() {
        let _ = entry(&mut first, i + 1, x, false);
    }
    build_knowing(exchanges, opts, &first)
}

/// The second pass, and the scan of what it made.
fn build_knowing(
    exchanges: &[Exchange],
    opts: &BuildOptions<'_>,
    first: &Redactor,
) -> Result<String, Vec<String>> {
    // What in an exchange comes from the router's page. The rest of the file is this app's own
    // words ("static", "router", "response"), which a hidden value may well be too.
    const FROM_ROUTER: &[&str] = &[
        "path",
        "query",
        "requestHeaders",
        "request",
        "responseHeaders",
        "redirectTo",
        "response",
        "apiPaths",
        "text",
    ];
    let mut red = Redactor::knowing(Rules::Capture, first);
    let mut facts = Facts::default();
    let mut list = Vec::with_capacity(exchanges.len());
    let mut code = Vec::new();
    for (i, x) in exchanges.iter().enumerate() {
        facts.read(x);
        let (value, text) = entry(&mut red, i + 1, x, opts.include_code);
        list.push(value);
        if let Some(text) = text {
            code.push((i, text));
        }
    }
    // Only now is everything that was hidden known: a script holding any of it stays out.
    for (i, text) in code {
        if red.holds_hidden(text) {
            list[i]["note"] = json!(NOTE_CODE_HELD);
        } else if let Some(x) = list[i].as_object_mut() {
            x.remove("note");
            x.insert("text".into(), json!(redact_identifier_tokens(text).0));
        }
    }
    let from_router: Vec<&Value> = list
        .iter()
        .flat_map(|x| FROM_ROUTER.iter().filter_map(move |key| x.get(*key)))
        .collect();
    let from_router = serde_json::to_string(&from_router).map_err(|e| vec![e.to_string()])?;
    let mut file = json!({
        "format": FORMAT,
        "appVersion": opts.app_version,
        "generatedAtMs": opts.generated_at_ms,
        "codeIncluded": opts.include_code,
        "limitHit": opts.limit,
        "router": {},
        "exchanges": list,
    });
    let text = serde_json::to_string_pretty(&file).map_err(|e| vec![e.to_string()])?;
    let mut leaks = red.pattern_leaks(&text);
    if red.holds_secret(&from_router) {
        leaks.insert(0, "an original identifier is still present".to_string());
    }
    if !leaks.is_empty() {
        return Err(leaks);
    }
    // These join after the scan, by their own rules: the vendor is one of a fixed list of
    // names, and a version looks like an IP address.
    file["router"]["vendor"] = json!(facts.vendor);
    file["router"]["model"] = json!(Facts::label(&facts.model));
    file["router"]["hardware"] = json!(Facts::label(&facts.hardware));
    file["router"]["firmware"] = json!(Facts::label(&facts.firmware));
    serde_json::to_string_pretty(&file).map_err(|e| vec![e.to_string()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::HIDDEN;

    const ROUTER: &str = "http://192.168.8.1";

    fn recorder() -> Recorder {
        Recorder::new(&Url::parse(ROUTER).unwrap()).expect("a web address")
    }

    /// A request event. `extra` is merged into the event (for `redirectResponse` and the like).
    fn sent(
        rec: &mut Recorder,
        id: &str,
        method: &str,
        url: &str,
        kind: &str,
        request: Value,
        extra: Value,
    ) -> Option<Ask> {
        let mut req = json!({ "url": url, "method": method, "headers": {} });
        for (k, v) in request.as_object().cloned().unwrap_or_default() {
            req[k] = v;
        }
        let mut ev = json!({ "requestId": id, "type": kind, "request": req });
        for (k, v) in extra.as_object().cloned().unwrap_or_default() {
            ev[k] = v;
        }
        rec.event("Network.requestWillBeSent", &ev.to_string())
    }

    fn answered(rec: &mut Recorder, id: &str, status: u16, mime: &str, headers: Value) {
        let ev = json!({ "requestId": id, "response": { "status": status, "mimeType": mime, "headers": headers } });
        assert_eq!(rec.event("Network.responseReceived", &ev.to_string()), None);
    }

    fn finished(rec: &mut Recorder, id: &str) -> Option<Ask> {
        rec.event(
            "Network.loadingFinished",
            &json!({ "requestId": id }).to_string(),
        )
    }

    /// A whole exchange with a text body, the way the app's glue feeds one.
    #[allow(clippy::too_many_arguments)]
    fn exchange(
        rec: &mut Recorder,
        id: &str,
        method: &str,
        url: &str,
        kind: &str,
        request: Value,
        mime: &str,
        body: &str,
    ) {
        sent(rec, id, method, url, kind, request, json!({}));
        // Dated, as a file the router keeps is: only that kind of script may be included.
        answered(
            rec,
            id,
            200,
            mime,
            json!({ "Content-Type": mime, "ETag": "1" }),
        );
        assert_eq!(finished(rec, id), Some(Ask::ResponseBody(id.to_string())));
        rec.response_body(
            id,
            &json!({ "body": body, "base64Encoded": false }).to_string(),
        );
    }

    fn opts() -> BuildOptions<'static> {
        BuildOptions {
            app_version: "1.2.3",
            generated_at_ms: 42,
            include_code: false,
            limit: None,
        }
    }

    fn built(rec: Recorder, opts: &BuildOptions<'_>) -> (String, Value) {
        let text = build(&rec.into_exchanges(), opts).expect("a clean file");
        let v = serde_json::from_str(&text).unwrap();
        (text, v)
    }

    #[test]
    fn records_what_the_page_asked_the_router_and_the_answer() {
        let mut rec = recorder();
        let ask = sent(
            &mut rec,
            "1",
            "POST",
            "http://192.168.8.1/api/net/net-mode?x=1",
            "XHR",
            json!({ "postData": "<request><LTEBand>800C5</LTEBand></request>", "headers": { "Content-Type": "text/xml; charset=UTF-8" } }),
            json!({}),
        );
        assert_eq!(ask, None);
        answered(
            &mut rec,
            "1",
            200,
            "text/xml",
            json!({ "Content-Type": "text/xml", "X-Frame-Options": "deny" }),
        );
        assert_eq!(finished(&mut rec, "1"), Some(Ask::ResponseBody("1".into())));
        rec.response_body(
            "1",
            r#"{"body":"<response>OK</response>","base64Encoded":false}"#,
        );

        let list = rec.into_exchanges();
        assert_eq!(list.len(), 1);
        let x = &list[0];
        assert_eq!(
            (x.method.as_str(), x.path.as_str(), x.kind),
            ("POST", "/api/net/net-mode", Kind::Api)
        );
        assert_eq!(x.query, vec![("x".to_string(), "1".to_string())]);
        assert_eq!(x.request_type.as_deref(), Some("text/xml"));
        assert_eq!(
            x.request_body.as_deref(),
            Some("<request><LTEBand>800C5</LTEBand></request>")
        );
        assert_eq!(
            (x.status, x.content_type.as_deref()),
            (Some(200), Some("text/xml"))
        );
        assert_eq!(x.response_headers, ["Content-Type", "X-Frame-Options"]);
        assert_eq!(x.body.as_deref(), Some("<response>OK</response>"));
        assert_eq!(x.note, None);
    }

    #[test]
    fn a_header_keeps_its_name_and_never_its_value() {
        let mut rec = recorder();
        let headers = json!({
            "Cookie": "SessionID=COOKIE-SECRET-VALUE",
            "Authorization": "Basic QVVUSC1TRUNSRVQ=",
            "__RequestVerificationToken": "TOKEN-SECRET-VALUE",
            "Content-Type": "application/json",
        });
        sent(
            &mut rec,
            "1",
            "GET",
            "http://192.168.8.1/api/x",
            "XHR",
            json!({ "headers": headers }),
            json!({}),
        );
        answered(
            &mut rec,
            "1",
            200,
            "application/json",
            json!({ "Set-Cookie": "SessionID=SET-COOKIE-SECRET" }),
        );
        let raw = format!("{:?}", rec.list);
        for secret in [
            "COOKIE-SECRET-VALUE",
            "QVVUSC1TRUNSRVQ",
            "TOKEN-SECRET-VALUE",
            "SET-COOKIE-SECRET",
        ] {
            assert!(!raw.contains(secret), "{secret} was stored");
        }
        let (text, v) = built(rec, &opts());
        assert_eq!(
            v["exchanges"][0]["requestHeaders"],
            json!([
                "__RequestVerificationToken",
                "Authorization",
                "Content-Type",
                "Cookie"
            ])
        );
        assert_eq!(v["exchanges"][0]["responseHeaders"], json!(["Set-Cookie"]));
        assert!(!text.contains("SECRET"));
    }

    #[test]
    fn another_host_is_never_stored() {
        let mut rec = recorder();
        for url in [
            "http://ipc.localhost/get_snapshot",
            "https://192.168.8.1/api/x",
            "http://192.168.8.1:8080/api/x",
            "http://example.com/track?id=1",
            "data:text/plain,hello",
        ] {
            assert_eq!(
                sent(&mut rec, "9", "GET", url, "Fetch", json!({}), json!({})),
                None
            );
            answered(&mut rec, "9", 200, "text/plain", json!({}));
            assert_eq!(finished(&mut rec, "9"), None, "{url}");
            rec.response_body("9", r#"{"body":"leak","base64Encoded":false}"#);
        }
        assert!(rec.is_empty());
    }

    #[test]
    fn a_redirect_ends_one_exchange_and_starts_the_next() {
        let mut rec = recorder();
        sent(
            &mut rec,
            "7",
            "POST",
            "http://192.168.8.1/go",
            "Document",
            json!({ "postData": "a=1" }),
            json!({}),
        );
        let hop =
            json!({ "redirectResponse": { "status": 302, "headers": { "Location": "/after" } } });
        sent(
            &mut rec,
            "7",
            "GET",
            "http://192.168.8.1/after",
            "Document",
            json!({}),
            hop,
        );
        answered(&mut rec, "7", 200, "text/html", json!({}));
        assert_eq!(finished(&mut rec, "7"), Some(Ask::ResponseBody("7".into())));
        rec.response_body("7", r#"{"body":"<html></html>","base64Encoded":false}"#);
        // Leaving for another host: the hop is noted, and the other host isn't recorded.
        sent(
            &mut rec,
            "8",
            "GET",
            "http://192.168.8.1/out",
            "Document",
            json!({}),
            json!({}),
        );
        let away = json!({ "redirectResponse": { "status": 301, "headers": {} } });
        sent(
            &mut rec,
            "8",
            "GET",
            "http://vendor.example/cloud",
            "Document",
            json!({}),
            away,
        );

        let list = rec.into_exchanges();
        assert_eq!(list.len(), 3);
        assert_eq!(
            (list[0].status, list[0].note),
            (Some(302), Some(NOTE_REDIRECT))
        );
        assert_eq!(list[0].redirect_to.as_deref(), Some("/after"));
        assert_eq!(list[0].response_headers, ["Location"]);
        assert_eq!(list[0].body, None);
        assert_eq!(
            (list[1].path.as_str(), list[1].body.as_deref()),
            ("/after", Some("<html></html>"))
        );
        assert_eq!(
            (list[2].status, list[2].redirect_to.as_deref()),
            (Some(301), Some(OTHER_HOST))
        );
    }

    #[test]
    fn a_static_file_keeps_no_body_and_a_cut_off_answer_says_so() {
        let mut rec = recorder();
        sent(
            &mut rec,
            "1",
            "GET",
            "http://192.168.8.1/img/logo.png",
            "Image",
            json!({}),
            json!({}),
        );
        answered(&mut rec, "1", 200, "image/png", json!({}));
        assert_eq!(finished(&mut rec, "1"), None);
        sent(
            &mut rec,
            "2",
            "GET",
            "http://192.168.8.1/api/slow",
            "Fetch",
            json!({}),
            json!({}),
        );
        rec.event(
            "Network.loadingFailed",
            r#"{"requestId":"2","errorText":"net::ERR_ABORTED","canceled":true}"#,
        );
        sent(
            &mut rec,
            "3",
            "GET",
            "http://192.168.8.1/api/gone",
            "Fetch",
            json!({}),
            json!({}),
        );
        assert_eq!(finished(&mut rec, "3"), Some(Ask::ResponseBody("3".into())));
        rec.response_body(
            "3",
            r#"{"spikeError":"No resource with given identifier found"}"#,
        );

        let list = rec.into_exchanges();
        assert_eq!(
            (list[0].kind, list[0].note),
            (Kind::Static, Some(NOTE_STATIC))
        );
        assert_eq!(list[1].note, Some(NOTE_UNFINISHED));
        assert_eq!(
            (list[2].body.as_deref(), list[2].note),
            (None, Some(NOTE_MISSING))
        );
    }

    #[test]
    fn gzip_bytes_sent_without_saying_so_are_unpacked() {
        // {"gz":"ok","model":"MOCK-5G-CPE"} gzipped, as the browser hands it over: base64.
        let mut rec = recorder();
        sent(
            &mut rec,
            "1",
            "GET",
            "http://192.168.8.1/api/gz",
            "Fetch",
            json!({}),
            json!({}),
        );
        finished(&mut rec, "1");
        rec.response_body(
            "1",
            r#"{"body":"H4sIAAAAAAAACqtWSq9SslLKz1bSUcrNT0nNUbJS8vV39tY1ddd1DnBVqgUAuBm2FSEAAAA=","base64Encoded":true}"#,
        );
        assert_eq!(
            rec.into_exchanges()[0].body.as_deref(),
            Some(r#"{"gz":"ok","model":"MOCK-5G-CPE"}"#)
        );
    }

    #[test]
    fn a_sent_body_the_event_left_out_is_asked_for() {
        let mut rec = recorder();
        let ask = sent(
            &mut rec,
            "1",
            "POST",
            "http://192.168.8.1/api/big",
            "XHR",
            json!({ "hasPostData": true }),
            json!({}),
        );
        assert_eq!(ask, Some(Ask::RequestBody("1".into())));
        rec.request_body("1", r#"{"postData":"{\"band\":\"3\"}"}"#);
        assert_eq!(
            rec.into_exchanges()[0].request_body.as_deref(),
            Some(r#"{"band":"3"}"#)
        );
    }

    #[test]
    fn the_limits_stop_the_recording_and_say_which() {
        let mut rec = recorder();
        for i in 0..MAX_EXCHANGES + 5 {
            sent(
                &mut rec,
                &format!("r{i}"),
                "GET",
                "http://192.168.8.1/api/x",
                "Fetch",
                json!({}),
                json!({}),
            );
        }
        assert_eq!(
            (rec.len(), rec.limit()),
            (MAX_EXCHANGES, Some(Limit::Requests))
        );

        let mut rec = recorder();
        sent(
            &mut rec,
            "big",
            "GET",
            "http://192.168.8.1/api/big",
            "Fetch",
            json!({}),
            json!({}),
        );
        finished(&mut rec, "big");
        let huge = "a".repeat(MAX_BODY_BYTES + 1);
        rec.response_body(
            "big",
            &json!({ "body": huge, "base64Encoded": false }).to_string(),
        );
        assert_eq!(rec.limit(), None);
        // A little under the largest body, because the addresses count as well.
        let chunk = "b".repeat(MAX_BODY_BYTES - 1024);
        for i in 0..11 {
            let id = format!("c{i}");
            sent(
                &mut rec,
                &id,
                "GET",
                "http://192.168.8.1/api/c",
                "Fetch",
                json!({}),
                json!({}),
            );
            finished(&mut rec, &id);
            rec.response_body(
                &id,
                &json!({ "body": chunk, "base64Encoded": false }).to_string(),
            );
        }
        assert_eq!(rec.limit(), Some(Limit::Size));
        let list = rec.into_exchanges();
        assert_eq!(list[0].note, Some(NOTE_TOO_LARGE));
        assert_eq!(list.iter().filter(|x| x.body.is_some()).count(), 10);

        let mut rec = recorder();
        rec.stop(Limit::Time);
        sent(
            &mut rec,
            "late",
            "GET",
            "http://192.168.8.1/api/x",
            "Fetch",
            json!({}),
            json!({}),
        );
        assert_eq!((rec.len(), rec.limit()), (0, Some(Limit::Time)));
    }

    #[test]
    fn a_login_keeps_its_names_and_hides_every_value() {
        let mut rec = recorder();
        // JSON-RPC, XML and a form: three ways routers send a login.
        exchange(
            &mut rec,
            "1",
            "POST",
            "http://192.168.8.1/ubus/",
            "XHR",
            json!({ "postData": r#"{"jsonrpc":"2.0","method":"call","params":["00000000","zwrt_web","web_login",{"password":"5F4DCC3B5AA765D61D8327DEB882CF99"}]}"# }),
            "application/json",
            r#"{"result":[0,{"ubus_rpc_session":"9f8e7d6c5b4a39281706f5e4d3c2b1a0"}]}"#,
        );
        exchange(
            &mut rec,
            "2",
            "POST",
            "http://192.168.8.1/api/user/authentication_login",
            "XHR",
            json!({ "postData": "<request><clientproof>abcdef0123456789abcdef</clientproof><finalnonce>NONCE-VALUE-123456</finalnonce></request>" }),
            "text/xml",
            "<response><serversignature>SIGNATURE-VALUE-99</serversignature><rsan>RSAN-VALUE-777777</rsan></response>",
        );
        exchange(
            &mut rec,
            "3",
            "POST",
            "http://192.168.8.1/goform/set",
            "Document",
            json!({ "postData": "user=admin&pw=hunter2-secret&goformId=LOGIN", "headers": { "Content-Type": "application/x-www-form-urlencoded" } }),
            "text/html",
            "<html><title>ok</title></html>",
        );
        let (text, v) = built(rec, &opts());
        for secret in [
            "5F4DCC3B5AA765D61D8327DEB882CF99",
            "9f8e7d6c5b4a39281706f5e4d3c2b1a0",
            "abcdef0123456789abcdef",
            "NONCE-VALUE-123456",
            "SIGNATURE-VALUE-99",
            "RSAN-VALUE-777777",
            "hunter2-secret",
        ] {
            assert!(!text.contains(secret), "{secret} is in the file");
        }
        let x = &v["exchanges"];
        assert_eq!(x[0]["login"], json!(true));
        assert_eq!(x[0]["request"]["params"][3]["password"], json!(HIDDEN));
        assert_eq!(x[0]["request"]["method"], json!(HIDDEN));
        assert_eq!(
            x[0]["response"]["result"][1]["ubus_rpc_session"],
            json!(HIDDEN)
        );
        assert_eq!(
            x[1]["request"],
            json!(format!("<request><clientproof>{HIDDEN}</clientproof><finalnonce>{HIDDEN}</finalnonce></request>"))
        );
        assert_eq!(
            x[1]["response"],
            json!(format!("<response><serversignature>{HIDDEN}</serversignature><rsan>{HIDDEN}</rsan></response>"))
        );
        assert_eq!(
            x[2]["request"],
            json!({ "user": HIDDEN, "pw": HIDDEN, "goformId": HIDDEN })
        );
    }

    #[test]
    fn readings_and_settings_stay_and_everything_personal_goes() {
        let mut rec = recorder();
        exchange(
            &mut rec,
            "1",
            "GET",
            "http://192.168.8.1/api/device/signal",
            "XHR",
            json!({}),
            "text/xml",
            "<response><rsrp>-95dBm</rsrp><sinr>12dB</sinr><band>3</band><mode>7</mode>\
             <cell_id>123456789</cell_id><pci>140</pci><WifiSsid>Hibi-Home-Net</WifiSsid>\
             <WifiWpapsk>my-wifi-password</WifiWpapsk><Imei>356789012345678</Imei>\
             <MacAddress1>AA:BB:CC:DD:EE:FF</MacAddress1><WanIPAddress>10.134.22.7</WanIPAddress></response>",
        );
        exchange(
            &mut rec,
            "2",
            "GET",
            "http://192.168.8.1/api/hosts",
            "Fetch",
            json!({}),
            "application/json",
            r#"{"hosts":[{"DeviceName":"Hibi-iPhone 12","ActualName":"Anad laptop","Active":"1","InterfaceType":"Wireless"}],
                "sms":[{"Content":"your code is 4471","Phone":"+966500000000","status":"read"}],
                "lte_rsrp":"-101","network_type":"5G NSA","lte_band_mask":"800C5","lte_ci":"87654321",
                "operator_name":"Zain KSA","note":"رسالة خاصة","wan_ip":"10.20.30.40"}"#,
        );
        let (text, v) = built(rec, &opts());
        for gone in [
            "123456789",
            "<pci>140</pci>",
            "Hibi-Home-Net",
            "my-wifi-password",
            "356789012345678",
            "AA:BB:CC:DD:EE:FF",
            "10.134.22.7",
            "Hibi-iPhone",
            "Anad laptop",
            "your code is 4471",
            "966500000000",
            "87654321",
            "رسالة",
            "10.20.30.40",
        ] {
            assert!(!text.contains(gone), "{gone} is in the file");
        }
        let signal = v["exchanges"][0]["response"].as_str().unwrap();
        for kept in [
            "<rsrp>-95dBm</rsrp>",
            "<sinr>12dB</sinr>",
            "<band>3</band>",
            "<mode>7</mode>",
        ] {
            assert!(signal.contains(kept), "{kept} is missing from {signal}");
        }
        let body = &v["exchanges"][1]["response"];
        assert_eq!(body["lte_rsrp"], json!("-101"));
        assert_eq!(body["network_type"], json!("5G NSA"));
        assert_eq!(body["lte_band_mask"], json!("800C5"));
        assert_eq!(body["operator_name"], json!("Zain KSA"));
        // Under a list of people's devices every value is hidden, and the names still show the
        // structure.
        assert_eq!(body["hosts"][0]["DeviceName"], json!(HIDDEN));
        assert_eq!(body["hosts"][0]["InterfaceType"], json!(HIDDEN));
        assert!(body["sms"][0].get("Content").is_some());
    }

    #[test]
    fn a_kept_name_still_cannot_carry_free_text_or_an_identifier() {
        let mut rec = recorder();
        exchange(
            &mut rec,
            "1",
            "GET",
            "http://192.168.8.1/api/x",
            "Fetch",
            json!({}),
            "application/json",
            &json!({
                "status": "مرحبا",
                "mode": "someone@example.com",
                "state": "y".repeat(201),
                "result": "seen at 10.9.8.7 by 356789012345678",
                "signal_note": "seen 10.9.8.7 356789012345678",
                "signal_level": 356789012345678u64,
                "band": 3,
            })
            .to_string(),
        );
        let (text, v) = built(rec, &opts());
        for gone in [
            "مرحبا",
            "someone@example.com",
            "10.9.8.7",
            "356789012345678",
        ] {
            assert!(!text.contains(gone), "{gone} is in the file");
        }
        let body = &v["exchanges"][0]["response"];
        // A state's name keeps one word of a state, never a sentence.
        assert_ne!(body["result"], json!("seen at 10.9.8.7 by 356789012345678"));
        assert_eq!(
            body["signal_note"],
            json!("seen [redacted-ipv4] [redacted-number]")
        );
        assert_eq!(body["band"], json!(3));
        assert!(!text.contains(&"y".repeat(201)));
    }

    #[test]
    fn a_command_and_its_names_stay_readable() {
        let mut rec = recorder();
        // What a driver's author needs most: which command was sent, and what it set.
        exchange(
            &mut rec,
            "1",
            "POST",
            "http://192.168.8.1/goform/goform_set_cmd_process",
            "XHR",
            json!({ "postData": "isTest=false&goformId=SET_BEARER_PREFERENCE&BearerPreference=NETWORK_auto&lte_band_lock=800C5&AD=0F1E2D3C4B5A69788796A5B4C3D2E1F0" }),
            "application/json",
            r#"{"result":"success"}"#,
        );
        exchange(
            &mut rec,
            "2",
            "POST",
            "http://192.168.8.1/ubus/",
            "XHR",
            json!({ "postData": r#"{"jsonrpc":"2.0","id":7,"method":"call","params":["0a1b2c3d4e5f60718293a4b5c6d7e8f9","zwrt_router.api","router_get_status",{}]}"# }),
            "application/json",
            r#"{"jsonrpc":"2.0","id":7,"result":[0,{"signalbar":"4"}]}"#,
        );
        exchange(
            &mut rec,
            "3",
            "GET",
            "http://192.168.8.1/goform/goform_get_cmd_process?isTest=false&cmd=network_type%2Clte_rsrp&multi_data=1&_=1794668197719",
            "XHR",
            json!({}),
            "application/json",
            r#"{"network_type":"LTE","lte_rsrp":"-97"}"#,
        );
        let (text, v) = built(rec, &opts());
        let x = &v["exchanges"];
        assert_eq!(x[0]["request"]["goformId"], json!("SET_BEARER_PREFERENCE"));
        assert_eq!(x[0]["request"]["lte_band_lock"], json!("800C5"));
        assert_eq!(x[0]["response"]["result"], json!("success"));
        assert!(!text.contains("0F1E2D3C4B5A69788796A5B4C3D2E1F0"));
        assert_eq!(x[1]["request"]["method"], json!("call"));
        assert_eq!(x[1]["request"]["params"][1], json!("zwrt_router.api"));
        assert_eq!(x[1]["request"]["params"][2], json!("router_get_status"));
        assert!(!text.contains("0a1b2c3d4e5f60718293a4b5c6d7e8f9"));
        assert_eq!(x[1]["response"]["result"][1]["signalbar"], json!("4"));
        assert_eq!(x[2]["query"]["cmd"], json!("network_type,lte_rsrp"));
        assert!(!text.contains("1794668197719"));
    }

    #[test]
    fn a_path_loses_what_names_a_session() {
        let mut rec = recorder();
        exchange(
            &mut rec,
            "1",
            "GET",
            "http://192.168.8.1/cgi-bin/luci/;stok=0123456789abcdef0123456789abcdef/admin/status",
            "Document",
            json!({}),
            "text/html",
            "<html><title>Status</title></html>",
        );
        let (text, v) = built(rec, &opts());
        assert_eq!(
            v["exchanges"][0]["path"],
            json!("/cgi-bin/luci/*/admin/status")
        );
        assert!(!text.contains("0123456789abcdef"));
    }

    #[test]
    fn code_is_left_out_unless_asked_for_and_is_cleaned_then() {
        let page = "<html><title>5G CPE</title><script>var mac = 'AA:BB:CC:DD:EE:FF'; $.get('/api/device/info')</script></html>";
        let script = "function load(){ return ajax('/goform/goform_get_cmd_process'); } // built for 356789012345678";
        let record = || {
            let mut rec = recorder();
            exchange(
                &mut rec,
                "1",
                "GET",
                "http://192.168.8.1/index.html",
                "Document",
                json!({}),
                "text/html",
                page,
            );
            exchange(
                &mut rec,
                "2",
                "GET",
                "http://192.168.8.1/js/app.js",
                "Script",
                json!({}),
                "application/javascript",
                script,
            );
            rec
        };
        let (text, v) = built(record(), &opts());
        let x = &v["exchanges"];
        assert_eq!(v["codeIncluded"], json!(false));
        assert_eq!(x[0]["apiPaths"], json!(["/api/device/info"]));
        assert_eq!(x[1]["apiPaths"], json!(["/goform/goform_get_cmd_process"]));
        assert_eq!(
            (&x[0]["note"], &x[1]["note"]),
            (&json!(NOTE_PAGE), &json!(NOTE_CODE))
        );
        assert!(x[0].get("text").is_none() && x[1].get("text").is_none());
        assert!(!text.contains("function load") && !text.contains("var mac"));
        // A page's title can be the router's host name, so it isn't kept either.
        assert!(!text.contains("5G CPE"));

        let with_code = BuildOptions {
            include_code: true,
            ..opts()
        };
        let (text, v) = built(record(), &with_code);
        let x = &v["exchanges"];
        assert_eq!(v["codeIncluded"], json!(true));
        // The script comes with its identifier replaced; the page's markup never comes.
        assert_eq!(
            x[1]["text"],
            json!("function load(){ return ajax('/goform/goform_get_cmd_process'); } // built for [redacted-number]")
        );
        assert!(x[1].get("note").is_none());
        assert_eq!(x[0]["note"], json!(NOTE_PAGE));
        assert!(x[0].get("text").is_none());
        assert!(!text.contains("AA:BB:CC:DD:EE:FF") && !text.contains("356789012345678"));
    }

    #[test]
    fn says_what_the_router_is_without_taking_a_device_on_the_network_for_it() {
        let mut rec = recorder();
        exchange(
            &mut rec,
            "1",
            "GET",
            "http://192.168.8.1/api/lan/HostInfo",
            "XHR",
            json!({}),
            "text/xml",
            "<response><Hosts><Host><DeviceName>Hibi-iPhone 12</DeviceName></Host></Hosts></response>",
        );
        exchange(
            &mut rec,
            "2",
            "GET",
            "http://192.168.8.1/api/device/information",
            "XHR",
            json!({}),
            "text/xml",
            "<response><DeviceName>H122-373</DeviceName><HardwareVersion>WL1H122M</HardwareVersion>\
             <SoftwareVersion>11.0.2.21</SoftwareVersion><Imei>356789012345678</Imei></response>",
        );
        let (text, v) = built(rec, &opts());
        assert_eq!(v["router"]["model"], json!("H122-373"));
        assert_eq!(v["router"]["hardware"], json!("WL1H122M"));
        assert_eq!(v["router"]["firmware"], json!("11.0.2.21"));
        assert!(!text.contains("Hibi-iPhone") && !text.contains("356789012345678"));
        // The version also stays where the router said it.
        assert!(v["exchanges"][1]["response"]
            .as_str()
            .unwrap()
            .contains("<SoftwareVersion>11.0.2.21</SoftwareVersion>"));
    }

    #[test]
    fn a_body_that_cannot_be_parsed_is_left_out() {
        let mut rec = recorder();
        exchange(
            &mut rec,
            "1",
            "POST",
            "http://192.168.8.1/api/blob",
            "XHR",
            json!({ "postData": "this is not a form, it has spaces and 356789012345678" }),
            "text/plain",
            "imei=356789012345678;name=Hibi",
        );
        let (text, v) = built(rec, &opts());
        let left_out = "(left out: not well-formed XML or JSON)";
        assert_eq!(v["exchanges"][0]["request"], json!(left_out));
        assert_eq!(v["exchanges"][0]["response"], json!(left_out));
        assert!(!text.contains("356789012345678") && !text.contains("Hibi"));
    }

    #[test]
    fn the_file_says_what_it_is_and_how_the_recording_ended() {
        let mut rec = recorder();
        exchange(
            &mut rec,
            "1",
            "GET",
            "http://192.168.8.1/api/x",
            "Fetch",
            json!({}),
            "application/json",
            r#"{"band":"3"}"#,
        );
        let stopped = BuildOptions {
            limit: Some(Limit::Time),
            ..opts()
        };
        let (_, v) = built(rec, &stopped);
        assert_eq!(v["format"], json!(FORMAT));
        assert_eq!(
            (&v["appVersion"], &v["generatedAtMs"]),
            (&json!("1.2.3"), &json!(42))
        );
        assert_eq!(v["limitHit"], json!("time"));
        assert_eq!(v["exchanges"][0]["n"], json!(1));
        assert_eq!(v["exchanges"][0]["kind"], json!("api"));
    }

    // ---- Found by the independent review of 2026-11-14 ----

    /// One answered API request whose sent body is declared a form.
    fn form_post(rec: &mut Recorder, id: &str, url: &str, body: &str) {
        let request = json!({ "postData": body, "headers": { "Content-Type": "application/x-www-form-urlencoded; charset=UTF-8" } });
        exchange(
            rec,
            id,
            "POST",
            url,
            "XHR",
            request,
            "application/json",
            r#"{"result":"0"}"#,
        );
    }

    #[test]
    fn a_body_declared_a_form_that_is_not_one_never_becomes_a_name() {
        let mut rec = recorder();
        // jQuery's default type on a JSON login, a base64 blob, and XML with an entity in it.
        form_post(
            &mut rec,
            "1",
            "http://192.168.8.1/api/login",
            r#"{"username":"admin","password":"5f4dcc3b5aa765d61d8327deb882cf99"}"#,
        );
        form_post(
            &mut rec,
            "2",
            "http://192.168.8.1/api/set",
            "dXNlcjpodW50ZXIyLXNlY3JldA==",
        );
        form_post(
            &mut rec,
            "3",
            "http://192.168.8.1/api/wlan",
            "<request><WifiSsid>Hibi &amp; Home</WifiSsid><WifiWpapsk>hunter22-wifi</WifiWpapsk></request>",
        );
        form_post(
            &mut rec,
            "4",
            "http://192.168.8.1/api/misc",
            "Hibi+Laptop=1&band=3",
        );
        let (text, v) = built(rec, &opts());
        for gone in [
            "5f4dcc3b5aa765d61d8327deb882cf99",
            "dXNlcjpodW50ZXIy",
            "hunter22-wifi",
            "Hibi",
        ] {
            assert!(!text.contains(gone), "{gone} is in the file");
        }
        assert_eq!(v["exchanges"][0]["request"]["password"], json!(HIDDEN));
    }

    #[test]
    fn a_name_that_is_really_data_is_not_copied() {
        let mut rec = recorder();
        exchange(
            &mut rec,
            "1",
            "GET",
            "http://192.168.8.1/api/all?Anad-PC=1&band=3",
            "XHR",
            json!({}),
            "application/json",
            r#"{"sms":{"966500000000":[{"status":"1"}]},
                "clients":{"Anad-PC":{"band":"5"},"AA:BB:CC:DD:EE:FF":{"band":"2"}},
                "0a1b2c3d4e5f":"x","lte_rsrp":"-90"}"#,
        );
        exchange(
            &mut rec,
            "2",
            "GET",
            "http://192.168.8.1/api/xml",
            "XHR",
            json!({}),
            "text/xml",
            r#"<response><Anad-PC Hibi-Phone="1"><band>3</band></Anad-PC><rsrp>-91</rsrp></response>"#,
        );
        let (text, v) = built(rec, &opts());
        for gone in [
            "966500000000",
            "Anad-PC",
            "AA:BB:CC:DD:EE:FF",
            "0a1b2c3d4e5f",
            "Hibi-Phone",
        ] {
            assert!(!text.contains(gone), "{gone} is in the file");
        }
        // The structure and the readings are still there.
        assert_eq!(v["exchanges"][0]["response"]["lte_rsrp"], json!("-90"));
        assert_eq!(v["exchanges"][0]["query"]["band"], json!("3"));
        let xml = v["exchanges"][1]["response"].as_str().unwrap();
        assert!(
            xml.contains("<band>3</band>") && xml.contains("<rsrp>-91</rsrp>"),
            "{xml}"
        );
    }

    #[test]
    fn a_session_folder_in_a_path_is_starred_in_every_place_a_path_is_shown() {
        let mut rec = recorder();
        let script = "load('/api/KDIRFAVOLMZYJHTGAB/info'); load('/cgi-bin/status.cgi');";
        exchange(
            &mut rec,
            "1",
            "GET",
            "http://192.168.8.1/KDIRFAVOLMZYJHTG/userRpm/Index.js",
            "Script",
            json!({}),
            "application/javascript",
            script,
        );
        let (text, v) = built(rec, &opts());
        assert_eq!(v["exchanges"][0]["path"], json!("/*/userRpm/Index.js"));
        assert_eq!(
            v["exchanges"][0]["apiPaths"],
            json!(["/api/*/info", "/cgi-bin/status.cgi"])
        );
        assert!(!text.contains("KDIRFAVOLMZYJHTG"));
    }

    #[test]
    fn a_kept_name_keeps_only_a_reading_or_a_state_word() {
        let mut rec = recorder();
        let body = json!({
            "result": "MyHomeWifi",
            "status": "{\"ssid\":\"MyHome\"}",
            "at_result": "+QENG: \"servingcell\",\"NOCONN\",\"LTE\",\"FDD\",420,04,1A2D003,140,1300,3,5,5,54A1,-95,-11",
            "wifi_mode": "WIFI:T:WPA;S:MyHome;P:hunter22;;",
            "modem_sn": "SN12345678XYZ",
            "product_sn": "PSN-000111",
            "tracking_area_code": "21665",
            "unlock_code": "8842",
            "broadband_id": "BB-77-AA",
            "state": "seen at the gate by someone today",
            "error_code": "0",
            "network_type": "5G NSA",
            "ppp_status": "ppp_connected",
            "lte_ca_pcell_band": "3",
        });
        exchange(
            &mut rec,
            "1",
            "GET",
            "http://192.168.8.1/api/x",
            "Fetch",
            json!({}),
            "application/json",
            &body.to_string(),
        );
        let (text, v) = built(rec, &opts());
        for gone in [
            "MyHomeWifi",
            "MyHome",
            "1A2D003",
            "54A1",
            "hunter22",
            "SN12345678XYZ",
            "PSN-000111",
            "21665",
            "8842",
            "BB-77-AA",
            "the gate",
        ] {
            assert!(!text.contains(gone), "{gone} is in the file");
        }
        let got = &v["exchanges"][0]["response"];
        assert_eq!(got["error_code"], json!("0"));
        assert_eq!(got["network_type"], json!("5G NSA"));
        assert_eq!(got["ppp_status"], json!("ppp_connected"));
        assert_eq!(got["lte_ca_pcell_band"], json!("3"));
    }

    #[test]
    fn a_sensitive_value_hides_its_length_too() {
        let mut rec = recorder();
        exchange(
            &mut rec,
            "1",
            "GET",
            "http://192.168.8.1/api/wlan",
            "XHR",
            json!({}),
            "text/xml",
            "<response><WifiWpapsk>my-wifi-password</WifiWpapsk><band>3</band></response>",
        );
        let (_, v) = built(rec, &opts());
        assert_eq!(
            v["exchanges"][0]["response"],
            json!(format!(
                "<response><WifiWpapsk>{HIDDEN}</WifiWpapsk><band>3</band></response>"
            ))
        );
    }

    #[test]
    fn a_page_is_never_included_and_a_script_only_when_nothing_hidden_is_in_it() {
        let mut rec = recorder();
        exchange(
            &mut rec,
            "1",
            "GET",
            "http://192.168.8.1/api/hosts",
            "Fetch",
            json!({}),
            "application/json",
            r#"{"list":[{"DeviceName":"Hibi-iPhone 12"}],"WifiWpapsk":"pa\"ss-word1"}"#,
        );
        let page = r#"<html><input name="admin_pw" value="hunter22-admin"><script>var sid = "c0ffee11c0ffee22";</script></html>"#;
        let js = "application/javascript";
        let get = |rec: &mut Recorder, id: &str, url: &str, kind: &str, mime: &str, body: &str| {
            exchange(rec, id, "GET", url, kind, json!({}), mime, body);
        };
        get(
            &mut rec,
            "2",
            "http://192.168.8.1/settings.html",
            "Document",
            "text/html",
            page,
        );
        get(
            &mut rec,
            "3",
            "http://192.168.8.1/js/login.js",
            "Script",
            js,
            "var salt = 'abc';",
        );
        get(
            &mut rec,
            "4",
            "http://192.168.8.1/js/data.js",
            "Script",
            js,
            "var name = 'Hibi-iPhone 12';",
        );
        get(
            &mut rec,
            "5",
            "http://192.168.8.1/js/wifi.js",
            "Script",
            js,
            r#"var k = "pa\"ss-word1";"#,
        );
        get(
            &mut rec,
            "6",
            "http://192.168.8.1/jsonp?cb=x",
            "Script",
            js,
            "x({a:1})",
        );
        get(
            &mut rec,
            "7",
            "http://192.168.8.1/js/app.js",
            "Script",
            js,
            "function bands(){ return 3; }",
        );
        let with_code = BuildOptions {
            include_code: true,
            ..opts()
        };
        let (text, v) = built(rec, &with_code);
        for gone in [
            "hunter22-admin",
            "c0ffee11c0ffee22",
            "var salt",
            "Hibi-iPhone",
            "ss-word1",
            "x({a:1})",
        ] {
            assert!(!text.contains(gone), "{gone} is in the file");
        }
        let x = &v["exchanges"];
        for left_out in [1, 2, 3, 4, 5] {
            assert!(
                x[left_out].get("text").is_none(),
                "exchange {} has its code",
                left_out + 1
            );
            assert!(x[left_out].get("note").is_some());
        }
        assert_eq!(x[6]["text"], json!("function bands(){ return 3; }"));
    }

    #[test]
    fn only_the_object_and_the_method_of_a_ubus_call_stay_readable() {
        let mut rec = recorder();
        for (id, body) in [
            (
                "1",
                r#"{"jsonrpc":"2.0","id":1,"method":"call","params":["0a1b2c3d4e5f60718293a4b5c6d7e8f9","uci","get",{"config":"anad.ddns.net","band":"3"}]}"#,
            ),
            (
                "2",
                r#"{"method":"set","params":["wireless","default_radio0","key","hunter_22"]}"#,
            ),
            (
                "3",
                r#"{"jsonrpc":"2.0","method":"call","params":["hunter_22","anad_router"]}"#,
            ),
        ] {
            let request = json!({ "postData": body });
            exchange(
                &mut rec,
                id,
                "POST",
                "http://192.168.8.1/ubus",
                "XHR",
                request,
                "application/json",
                "{}",
            );
        }
        let (text, v) = built(rec, &opts());
        for gone in [
            "0a1b2c3d4e5f60718293a4b5c6d7e8f9",
            "anad.ddns.net",
            "default_radio0",
            "hunter_22",
            "anad_router",
        ] {
            assert!(!text.contains(gone), "{gone} is in the file");
        }
        let call = &v["exchanges"][0]["request"]["params"];
        assert_eq!((&call[1], &call[2]), (&json!("uci"), &json!("get")));
        assert_eq!(call[3]["band"], json!("3"));
    }

    #[test]
    fn a_device_on_the_network_in_the_same_answer_is_not_the_model() {
        let build_one = |body: &str, mime: &str, kind: &str| {
            let mut rec = recorder();
            exchange(
                &mut rec,
                "1",
                "GET",
                "http://192.168.8.1/api/info",
                kind,
                json!({}),
                mime,
                body,
            );
            built(rec, &opts())
        };
        let (text, v) = build_one(
            r#"{"firmware_version":"1.2.3","clients":[{"device_name":"Hibi-PC 2"}]}"#,
            "application/json",
            "XHR",
        );
        assert_eq!(v["router"]["model"], json!(null));
        assert_eq!(v["router"]["firmware"], json!("1.2.3"));
        assert!(!text.contains("Hibi-PC"));
        // A MAC-shaped "model" is an identifier, not a model.
        let (text, v) = build_one(
            r#"{"model":"A4B1C2D3E4F5","hardware_version":"V1"}"#,
            "application/json",
            "XHR",
        );
        assert_eq!(v["router"]["model"], json!(null));
        assert!(!text.contains("A4B1C2D3E4F5"));
        let (_, v) = build_one(
            r#"{"model_name":"MC888 Ultra","hw_version":"MC888-1.0"}"#,
            "application/json",
            "XHR",
        );
        assert_eq!(v["router"]["model"], json!("MC888 Ultra"));
        // LuCI puts the router's host name in the title.
        let (text, _) = build_one(
            "<html><title>Anad-Router - Overview - LuCI</title></html>",
            "text/html",
            "Document",
        );
        assert!(!text.contains("Anad-Router"));
    }

    #[test]
    fn a_login_is_recognised_by_more_than_the_word_password() {
        let mut rec = recorder();
        let bodies = [
            r#"{"hash":"0f1e2d3c4b5a69788796a5b4c3d2e1f0"}"#,
            r#"{"digest":"0f1e2d3c4b5a69788796a5b4c3d2e1f1"}"#,
            r#"{"otp":"0f1e2d3c4b5a69788796a5b4c3d2e1f2"}"#,
            r#"{"auth":"0f1e2d3c4b5a69788796a5b4c3d2e1f3"}"#,
            r#"{"secret":"0f1e2d3c4b5a69788796a5b4c3d2e1f4"}"#,
        ];
        for (i, body) in bodies.iter().enumerate() {
            let request = json!({ "postData": body });
            let answer = r#"{"band":"3","stok":"aabbccdd00112233"}"#;
            exchange(
                &mut rec,
                &i.to_string(),
                "POST",
                "http://192.168.8.1/api/x",
                "XHR",
                request,
                "application/json",
                answer,
            );
        }
        let (text, v) = built(rec, &opts());
        for i in 0..bodies.len() {
            assert_eq!(
                v["exchanges"][i]["login"],
                json!(true),
                "exchange {}",
                i + 1
            );
        }
        assert!(!text.contains("0f1e2d3c4b5a6978") && !text.contains("aabbccdd00112233"));
    }

    #[test]
    fn a_sent_body_that_never_arrived_is_treated_as_a_login() {
        let mut rec = recorder();
        let request = json!({ "hasPostData": true });
        let ask = sent(
            &mut rec,
            "1",
            "POST",
            "http://192.168.8.1/api/x",
            "XHR",
            request,
            json!({}),
        );
        assert_eq!(ask, Some(Ask::RequestBody("1".into())));
        answered(&mut rec, "1", 200, "application/json", json!({}));
        finished(&mut rec, "1");
        rec.response_body("1", r#"{"body":"{\"band\":\"3\"}","base64Encoded":false}"#);
        let (_, v) = built(rec, &opts());
        assert_eq!(v["exchanges"][0]["login"], json!(true));
        assert_eq!(v["exchanges"][0]["response"]["band"], json!(HIDDEN));
    }

    #[test]
    fn a_late_sent_body_goes_to_the_request_that_sent_it() {
        let mut rec = recorder();
        let request = json!({ "hasPostData": true });
        sent(
            &mut rec,
            "1",
            "POST",
            "http://192.168.8.1/save",
            "Document",
            request,
            json!({}),
        );
        let hop = json!({ "redirectResponse": { "status": 302, "headers": {} } });
        sent(
            &mut rec,
            "1",
            "GET",
            "http://192.168.8.1/done",
            "Document",
            json!({}),
            hop,
        );
        rec.request_body("1", r#"{"postData":"a=1"}"#);
        let list = rec.into_exchanges();
        assert_eq!(list[0].request_body.as_deref(), Some("a=1"));
        assert_eq!(list[1].request_body, None);
    }

    #[test]
    fn an_address_cannot_hide_behind_a_name_or_a_character_reference() {
        let mut rec = recorder();
        exchange(
            &mut rec,
            "1",
            "GET",
            "http://192.168.8.1/api/x",
            "XHR",
            json!({}),
            "text/xml",
            "<?xml version=\"1.0\" encoding=\"10.9.8.7\"?><response><wanversion>10.134.22.7</wanversion>\
             <status>&#49;&#48;&#46;&#49;&#46;&#50;&#46;&#51;</status><signal>aabb-ccdd-eeff</signal>\
             <level>2a00:1:2:3::/64</level><SoftwareVersion>11.0.2.21</SoftwareVersion></response>",
        );
        let (text, v) = built(rec, &opts());
        for gone in [
            "10.9.8.7",
            "10.134.22.7",
            "&#49;",
            "aabb-ccdd-eeff",
            "2a00:1:2:3",
        ] {
            assert!(!text.contains(gone), "{gone} is in the file");
        }
        let xml = v["exchanges"][0]["response"].as_str().unwrap();
        assert!(
            xml.contains("<SoftwareVersion>11.0.2.21</SoftwareVersion>"),
            "{xml}"
        );
    }

    #[test]
    fn a_content_type_and_a_method_are_kept_only_in_their_usual_shape() {
        let mut rec = recorder();
        let headers =
            json!({ "Content-Type": "text/SECRET-IN-TYPE value; boundary=SECRET-BOUNDARY" });
        let request = json!({ "headers": headers });
        sent(
            &mut rec,
            "1",
            "HIBI-SECRET-METHOD",
            "http://192.168.8.1/api/x",
            "XHR",
            request,
            json!({}),
        );
        answered(&mut rec, "1", 200, "weird type SECRET-MIME", json!({}));
        let request = json!({ "headers": { "content-type": "Application/JSON; charset=utf-8" } });
        sent(
            &mut rec,
            "2",
            "POST",
            "http://192.168.8.1/api/y",
            "XHR",
            request,
            json!({}),
        );
        let (text, v) = built(rec, &opts());
        assert!(!text.contains("SECRET"), "{text}");
        assert_eq!(v["exchanges"][0]["method"], json!("OTHER"));
        assert_eq!(v["exchanges"][1]["requestType"], json!("application/json"));
    }

    #[test]
    fn a_very_long_address_is_not_kept() {
        let mut rec = recorder();
        let long = "a".repeat(100 * 1024);
        for i in 0..MAX_EXCHANGES {
            let url = format!("http://192.168.8.1/api/x?q={long}");
            sent(
                &mut rec,
                &i.to_string(),
                "GET",
                &url,
                "Fetch",
                json!({}),
                json!({}),
            );
        }
        let list = rec.into_exchanges();
        assert_eq!(list.len(), MAX_EXCHANGES);
        assert!(list
            .iter()
            .all(|x| x.path == TOO_LONG && x.query.is_empty()));
    }

    #[test]
    fn addresses_and_names_count_against_the_size_limit() {
        // Not reachable through the events (500 requests of 8 KiB), so the count is checked itself.
        let mut rec = recorder();
        assert!(rec.charge(MAX_TOTAL_BYTES - 10));
        let url = "http://192.168.8.1/api/a-path-longer-than-ten-bytes";
        assert_eq!(
            sent(&mut rec, "1", "GET", url, "Fetch", json!({}), json!({})),
            None
        );
        assert_eq!((rec.len(), rec.limit()), (0, Some(Limit::Size)));
    }

    #[test]
    fn only_a_web_address_can_be_recorded() {
        for not_web in ["file:///C:/router.html", "data:text/html,hi", "about:blank"] {
            assert!(
                Recorder::new(&Url::parse(not_web).unwrap()).is_none(),
                "{not_web}"
            );
        }
        assert!(Recorder::new(&Url::parse("https://192.168.8.1:8443/").unwrap()).is_some());
    }

    #[test]
    fn what_is_recorded_does_not_print_its_contents() {
        let mut rec = recorder();
        let request = json!({ "postData": "SENT-SECRET" });
        exchange(
            &mut rec,
            "1",
            "POST",
            "http://192.168.8.1/api/x?k=QUERY-SECRET",
            "XHR",
            request,
            "text/plain",
            "ANSWER-SECRET",
        );
        let printed = format!("{:?}", rec.into_exchanges());
        assert!(!printed.contains("SECRET"), "{printed}");
        assert!(printed.contains("/api/x"));
    }

    #[test]
    fn a_long_list_of_field_names_in_a_command_stays_whole() {
        let mut rec = recorder();
        let names: Vec<String> = (0..60).map(|i| format!("lte_field_name_{i}")).collect();
        let list = names.join(",");
        let url = format!(
            "http://192.168.8.1/goform/get?cmd={}&multi_data=1",
            list.replace(',', "%2C")
        );
        // The answer has each field that was asked for: that is what makes them field names.
        let answer: Map<String, Value> = names.iter().map(|n| (n.clone(), json!("1"))).collect();
        let answer = Value::Object(answer).to_string();
        exchange(
            &mut rec,
            "1",
            "GET",
            &url,
            "XHR",
            json!({}),
            "application/json",
            &answer,
        );
        let (_, v) = built(rec, &opts());
        assert_eq!(v["exchanges"][0]["query"]["cmd"], json!(list));
    }

    // ---- Usefulness: a real recording must still tell a driver's author what they need ----

    /// A GET the page made by itself, and the router's answer.
    fn poll(rec: &mut Recorder, id: &str, url: &str, mime: &str, body: &str) {
        exchange(rec, id, "GET", url, "XHR", json!({}), mime, body);
    }

    /// A POST the page made by itself, and the router's answer.
    fn post(rec: &mut Recorder, id: &str, url: &str, request: Value, mime: &str, body: &str) {
        exchange(rec, id, "POST", url, "XHR", request, mime, body);
    }

    #[test]
    fn a_huawei_style_recording_keeps_its_login_shape_its_readings_and_its_band_command() {
        let mut rec = recorder();
        let login = json!({
            "postData": "<request><username>admin</username><firstnonce>abcdef0123456789</firstnonce><mode>1</mode></request>",
            "headers": { "__RequestVerificationToken": "TOKENVALUE123456" },
        });
        post(
            &mut rec,
            "1",
            "http://192.168.8.1/api/user/challenge_login",
            login,
            "text/xml",
            "<response><salt>aa11bb22</salt><iterations>100</iterations><servernonce>bb22cc33dd44ee55</servernonce></response>",
        );
        poll(
            &mut rec,
            "2",
            "http://192.168.8.1/api/device/signal",
            "text/xml",
            "<response><pci>140</pci><cell_id>123456789</cell_id><rsrq>-11.0dB</rsrq><rsrp>-95dBm</rsrp>\
             <rssi>-63dBm</rssi><sinr>12dB</sinr><band>3</band><mode>7</mode><nrrsrp>-101dBm</nrrsrp></response>",
        );
        // jQuery calls the XML it posts a form.
        let set = json!({
            "postData": "<request><NetworkMode>03</NetworkMode><NetworkBand>3FFFFFFF</NetworkBand><LTEBand>800C5</LTEBand></request>",
            "headers": {
                "__RequestVerificationToken": "TOKENVALUE654321",
                "Content-Type": "application/x-www-form-urlencoded; charset=UTF-8",
            },
        });
        post(
            &mut rec,
            "3",
            "http://192.168.8.1/api/net/net-mode",
            set,
            "text/xml",
            "<response>OK</response>",
        );
        poll(
            &mut rec,
            "4",
            "http://192.168.8.1/api/net/net-mode",
            "text/xml",
            "<error><code>125002</code><message></message></error>",
        );
        let (text, v) = built(rec, &opts());
        let x = &v["exchanges"];
        assert_eq!(x[0]["login"], json!(true));
        assert!(x[0]["request"]
            .as_str()
            .unwrap()
            .contains(&format!("<firstnonce>{HIDDEN}</firstnonce>")));
        assert!(
            x[1].get("login").is_none()
                && x[2].get("login").is_none()
                && x[3].get("login").is_none()
        );
        assert_eq!(x[1]["path"], json!("/api/device/signal"));
        let signal = x[1]["response"].as_str().unwrap();
        for kept in [
            "<rsrq>-11.0dB</rsrq>",
            "<rsrp>-95dBm</rsrp>",
            "<rssi>-63dBm</rssi>",
            "<sinr>12dB</sinr>",
            "<band>3</band>",
            "<mode>7</mode>",
            "<nrrsrp>-101dBm</nrrsrp>",
        ] {
            assert!(signal.contains(kept), "{kept} is missing from {signal}");
        }
        assert_eq!(x[2]["path"], json!("/api/net/net-mode"));
        assert_eq!(
            x[2]["request"],
            json!("<request><NetworkMode>03</NetworkMode><NetworkBand>3FFFFFFF</NetworkBand><LTEBand>800C5</LTEBand></request>")
        );
        assert!(x[2]["requestHeaders"]
            .as_array()
            .unwrap()
            .contains(&json!("__RequestVerificationToken")));
        assert_eq!(x[2]["response"], json!("<response>OK</response>"));
        assert!(x[3]["response"]
            .as_str()
            .unwrap()
            .contains("<code>125002</code>"));
        for gone in [
            "TOKENVALUE",
            "abcdef0123456789",
            "bb22cc33dd44ee55",
            "aa11bb22",
            "123456789",
        ] {
            assert!(!text.contains(gone), "{gone} is in the file");
        }
    }

    #[test]
    fn a_zte_style_recording_keeps_its_polls_and_its_band_command() {
        let mut rec = recorder();
        // The status poll asks for `loginfo` among its fields. It is not a login.
        poll(
            &mut rec,
            "1",
            "http://192.168.8.1/goform/goform_get_cmd_process?isTest=false&cmd=modem_main_state%2Cloginfo%2Cnetwork_type%2Clte_rsrp%2CZ5g_rsrp%2Cwan_active_band&multi_data=1&_=1794668197719",
            "application/json",
            r#"{"modem_main_state":"modem_init_complete","loginfo":"ok","network_type":"ENDC","lte_rsrp":"-97","Z5g_rsrp":"-101","wan_active_band":"LTE BAND 3"}"#,
        );
        let form = |body: &str| json!({ "postData": body, "headers": { "Content-Type": "application/x-www-form-urlencoded" } });
        post(
            &mut rec,
            "2",
            "http://192.168.8.1/goform/goform_set_cmd_process",
            form("isTest=false&goformId=LOGIN&password=QWRtaW4xMjM0NTY3OA%3D%3D"),
            "application/json",
            r#"{"result":"0"}"#,
        );
        post(
            &mut rec,
            "3",
            "http://192.168.8.1/goform/goform_set_cmd_process",
            form("isTest=false&goformId=SET_LTE_BAND_LOCK&lte_band_lock=800C5&AD=0F1E2D3C4B5A69788796A5B4C3D2E1F0"),
            "application/json",
            r#"{"result":"success"}"#,
        );
        // A read that carries an anti-forgery token is still a read.
        poll(
            &mut rec,
            "4",
            "http://192.168.8.1/goform/goform_get_cmd_process?cmd=lte_rsrp&csrf_token=0f1e2d3c4b5a6978",
            "application/json",
            r#"{"lte_rsrp":"-98"}"#,
        );
        let (text, v) = built(rec, &opts());
        let x = &v["exchanges"];
        assert!(
            x[0].get("login").is_none(),
            "the status poll was taken for a login"
        );
        assert_eq!(
            x[0]["query"]["cmd"],
            json!("modem_main_state,loginfo,network_type,lte_rsrp,Z5g_rsrp,wan_active_band")
        );
        let status = &x[0]["response"];
        assert_eq!(status["modem_main_state"], json!("modem_init_complete"));
        assert_eq!(status["network_type"], json!("ENDC"));
        assert_eq!(
            (&status["lte_rsrp"], &status["Z5g_rsrp"]),
            (&json!("-97"), &json!("-101"))
        );
        assert_eq!(status["wan_active_band"], json!("LTE BAND 3"));
        assert_eq!(x[1]["login"], json!(true));
        assert_eq!(x[1]["request"]["password"], json!(HIDDEN));
        assert!(
            x[2].get("login").is_none(),
            "the band command was taken for a login"
        );
        assert_eq!(x[2]["path"], json!("/goform/goform_set_cmd_process"));
        assert_eq!(x[2]["request"]["goformId"], json!("SET_LTE_BAND_LOCK"));
        assert_eq!(x[2]["request"]["lte_band_lock"], json!("800C5"));
        assert_eq!(x[2]["response"]["result"], json!("success"));
        assert!(
            x[3].get("login").is_none(),
            "a read with a token was taken for a login"
        );
        assert_eq!(x[3]["response"]["lte_rsrp"], json!("-98"));
        for gone in [
            "QWRtaW4xMjM0NTY3OA",
            "0F1E2D3C4B5A69788796A5B4C3D2E1F0",
            "0f1e2d3c4b5a6978",
            "1794668197719",
        ] {
            assert!(!text.contains(gone), "{gone} is in the file");
        }
    }

    #[test]
    fn a_ubus_style_recording_keeps_its_calls_and_its_readings() {
        let mut rec = recorder();
        let call = |params: &str| json!({ "postData": format!(r#"{{"jsonrpc":"2.0","id":1,"method":"call","params":{params}}}"#) });
        post(
            &mut rec,
            "1",
            "http://192.168.8.1/ubus/",
            call(
                r#"["00000000000000000000000000000000","zwrt_web","web_login",{"password":"5F4DCC3B5AA765D61D8327DEB882CF99"}]"#,
            ),
            "application/json",
            r#"{"jsonrpc":"2.0","id":1,"result":[0,{"ubus_rpc_session":"9f8e7d6c5b4a39281706f5e4d3c2b1a0"}]}"#,
        );
        // A login whose arguments have names that say nothing: the method's name says it.
        post(
            &mut rec,
            "2",
            "http://192.168.8.1/ubus/",
            call(
                r#"["00000000000000000000000000000000","session","login",{"u":"root","p":"0a1b2c3d4e5f6071"}]"#,
            ),
            "application/json",
            r#"{"jsonrpc":"2.0","id":1,"result":[0,{"sid":"8e7d6c5b4a39281706f5e4d3c2b1a09f"}]}"#,
        );
        post(
            &mut rec,
            "3",
            "http://192.168.8.1/ubus/",
            call(
                r#"["9f8e7d6c5b4a39281706f5e4d3c2b1a0","zwrt_router.api","router_get_status",{}]"#,
            ),
            "application/json",
            r#"{"jsonrpc":"2.0","id":1,"result":[0,{"network_type":"5G NSA","lte_rsrp":"-97","nr5g_rsrp":"-101","lte_band":"3"}]}"#,
        );
        post(
            &mut rec,
            "4",
            "http://192.168.8.1/ubus/",
            call(
                r#"["9f8e7d6c5b4a39281706f5e4d3c2b1a0","zwrt_nwinfo.api","nwinfo_set_netselect",{"net_select_mode":"manual_select","lte_band_lock":"800C5"}]"#,
            ),
            "application/json",
            r#"{"jsonrpc":"2.0","id":1,"result":[0]}"#,
        );
        let (text, v) = built(rec, &opts());
        let x = &v["exchanges"];
        assert_eq!(
            (&x[0]["login"], &x[1]["login"]),
            (&json!(true), &json!(true))
        );
        assert!(x[2].get("login").is_none() && x[3].get("login").is_none());
        let read = &x[2]["request"]["params"];
        assert_eq!(
            (&read[1], &read[2]),
            (&json!("zwrt_router.api"), &json!("router_get_status"))
        );
        let status = &x[2]["response"]["result"][1];
        assert_eq!(status["network_type"], json!("5G NSA"));
        assert_eq!(
            (&status["lte_rsrp"], &status["nr5g_rsrp"]),
            (&json!("-97"), &json!("-101"))
        );
        assert_eq!(status["lte_band"], json!("3"));
        let set = &x[3]["request"]["params"];
        assert_eq!(
            (&set[1], &set[2]),
            (&json!("zwrt_nwinfo.api"), &json!("nwinfo_set_netselect"))
        );
        assert_eq!(set[3]["net_select_mode"], json!("manual_select"));
        assert_eq!(set[3]["lte_band_lock"], json!("800C5"));
        for gone in [
            "5F4DCC3B5AA765D61D8327DEB882CF99",
            "9f8e7d6c5b4a39281706f5e4d3c2b1a0",
            "0a1b2c3d4e5f6071",
            "8e7d6c5b4a39281706f5e4d3c2b1a09f",
        ] {
            assert!(!text.contains(gone), "{gone} is in the file");
        }
    }

    #[test]
    fn a_token_field_alone_is_hidden_without_hiding_the_rest() {
        let mut rec = recorder();
        post(
            &mut rec,
            "1",
            "http://192.168.8.1/api/x",
            json!({ "postData": r#"{"x_token":"0f1e2d3c4b5a69788796a5b4c3d2e1f5","lte_band":"3"}"# }),
            "application/json",
            r#"{"result":"aabbccdd00112233","band":"3"}"#,
        );
        let (text, v) = built(rec, &opts());
        let x = &v["exchanges"][0];
        assert!(x.get("login").is_none());
        assert_eq!(x["request"]["x_token"], json!(HIDDEN));
        assert_eq!(x["request"]["lte_band"], json!("3"));
        assert_eq!(x["response"]["band"], json!("3"));
        // A token handed back under a state's name isn't one word of a state.
        assert!(!text.contains("0f1e2d3c4b5a6978") && !text.contains("aabbccdd00112233"));
    }

    #[test]
    fn a_version_or_a_band_cannot_carry_a_long_number() {
        let mut rec = recorder();
        poll(
            &mut rec,
            "1",
            "http://192.168.8.1/api/device/information",
            "text/xml",
            "<response><SoftwareVersion>356789012345678</SoftwareVersion><LTEBand>356789012345679</LTEBand>\
             <NetworkBand>3FFFFFFF</NetworkBand><WebUIVersion>11.0.2.21(H057SP5C983)</WebUIVersion></response>",
        );
        let (text, v) = built(rec, &opts());
        assert!(!text.contains("356789012345678") && !text.contains("356789012345679"));
        let xml = v["exchanges"][0]["response"].as_str().unwrap();
        assert!(xml.contains("<NetworkBand>3FFFFFFF</NetworkBand>"), "{xml}");
        assert!(
            xml.contains("<WebUIVersion>11.0.2.21(H057SP5C983)</WebUIVersion>"),
            "{xml}"
        );
    }

    // ---- Found by the second pass of the independent review, 2026-11-14 ----

    #[test]
    fn a_path_segment_that_was_hidden_elsewhere_is_starred() {
        let mut rec = recorder();
        poll(
            &mut rec,
            "1",
            "http://192.168.8.1/api/hosts",
            "application/json",
            r#"{"hosts":[{"name":"Hibi-Laptop"}],"sms":[{"phone":"966500000000"}]}"#,
        );
        post(
            &mut rec,
            "2",
            "http://192.168.8.1/api/hosts/Hibi-Laptop/block",
            json!({}),
            "application/json",
            "{}",
        );
        poll(
            &mut rec,
            "3",
            "http://192.168.8.1/api/sms/966500000000",
            "application/json",
            "{}",
        );
        poll(
            &mut rec,
            "4",
            "http://192.168.8.1/api/sms/20261114",
            "application/json",
            "{}",
        );
        let (text, v) = built(rec, &opts());
        let x = &v["exchanges"];
        assert_eq!(x[1]["path"], json!("/api/hosts/*/block"));
        assert_eq!(x[2]["path"], json!("/api/sms/*"));
        // A long number in a path is somebody's, whether or not it was seen before.
        assert_eq!(x[3]["path"], json!("/api/sms/*"));
        assert!(!text.contains("Hibi-Laptop") && !text.contains("966500000000"));
    }

    #[test]
    fn a_declared_form_without_a_pair_is_not_a_form() {
        let mut rec = recorder();
        form_post(
            &mut rec,
            "1",
            "http://192.168.8.1/api/wifi/password",
            "hunter22",
        );
        form_post(
            &mut rec,
            "2",
            "http://192.168.8.1/api/wifi/password",
            "hunter23=",
        );
        let (text, _) = built(rec, &opts());
        assert!(
            !text.contains("hunter22") && !text.contains("hunter23"),
            "{text}"
        );
    }

    #[test]
    fn a_code_the_page_sends_is_hidden_and_a_code_the_router_answers_is_kept() {
        let mut rec = recorder();
        post(
            &mut rec,
            "1",
            "http://192.168.8.1/api/check",
            json!({ "postData": r#"{"name":"admin","code":"7421","mode":"3"}"# }),
            "application/json",
            r#"{"code":"0","result":"a1b2c3d4","status":"KDIRFAVOLMZYJHTG","nck_code":"12345678"}"#,
        );
        let (text, v) = built(rec, &opts());
        let x = &v["exchanges"][0];
        assert!(x.get("login").is_none());
        // What the page sends under "code" is something a person typed.
        assert_ne!(x["request"]["code"], json!("7421"));
        assert_eq!(x["request"]["mode"], json!("3"));
        assert_eq!(x["response"]["code"], json!("0"));
        for gone in ["7421", "a1b2c3d4", "KDIRFAVOLMZYJHTG", "12345678"] {
            assert!(!text.contains(gone), "{gone} is in the file");
        }
    }

    #[test]
    fn a_list_of_field_names_keeps_only_the_fields_the_answer_has() {
        let mut rec = recorder();
        poll(
            &mut rec,
            "1",
            "http://192.168.8.1/goform/get?cmd=lte_rsrp%2CHibiHome%2Chunter22%2C1234",
            "application/json",
            r#"{"lte_rsrp":"-97"}"#,
        );
        let (text, v) = built(rec, &opts());
        assert_eq!(v["exchanges"][0]["query"]["cmd"], json!("lte_rsrp,*,*,*"));
        assert!(!text.contains("HibiHome") && !text.contains("hunter22"));
    }

    #[test]
    fn a_value_hidden_in_one_place_is_not_kept_in_another() {
        // The Wi-Fi name is hidden where it is named. The same text under a state's name reads
        // like a state, so it is hidden there too, wherever in the recording it comes.
        let mut rec = recorder();
        poll(
            &mut rec,
            "1",
            "http://192.168.8.1/api/ppp",
            "application/json",
            r#"{"ppp_status":"hibi_home_net","band":"3"}"#,
        );
        poll(
            &mut rec,
            "2",
            "http://192.168.8.1/api/wlan",
            "application/json",
            r#"{"WifiSsid":"hibi_home_net"}"#,
        );
        let list = rec.into_exchanges();
        let text = build(&list, &opts()).expect("a clean file");
        let v: Value = serde_json::from_str(&text).unwrap();
        assert!(!text.contains("hibi_home_net"));
        assert_eq!(v["exchanges"][0]["response"]["band"], json!("3"));
        // Without the first pass the value is kept before its name is met, and only the scan of
        // the finished file stands between it and the disk.
        let blind = Redactor::with_rules(Rules::Capture);
        assert_eq!(
            build_knowing(&list, &opts(), &blind).unwrap_err(),
            ["an original identifier is still present"]
        );
    }

    #[test]
    fn an_ordinary_word_hidden_somewhere_never_stops_the_save() {
        let mut rec = recorder();
        // A stock OpenWrt names itself "OpenWrt", and settings say "static" in sensitive places.
        poll(
            &mut rec,
            "1",
            "http://192.168.8.1/cgi-bin/luci/admin/status",
            "application/json",
            r#"{"hostname":"OpenWrt","dns_mode":"static","net_mode":"static","wifi_enable":"1","lte_rsrp":"-97"}"#,
        );
        poll(
            &mut rec,
            "2",
            "http://192.168.8.1/js/luci.js",
            "application/javascript",
            "function luci(){ return 'openwrt'; }",
        );
        poll(
            &mut rec,
            "3",
            "http://192.168.8.1/api/board",
            "application/json",
            r#"{"hostname":"router"}"#,
        );
        poll(
            &mut rec,
            "4",
            "http://192.168.8.1/api/router/info",
            "application/json",
            r#"{"band":"3"}"#,
        );
        poll(
            &mut rec,
            "5",
            "http://192.168.8.1/api/fw",
            "text/xml",
            r#"<fw version="11.0.2.21"><band>3</band></fw>"#,
        );
        let (text, v) = built(rec, &opts());
        assert_eq!(v["router"]["vendor"], json!("OpenWrt"));
        assert_eq!(v["exchanges"][0]["response"]["lte_rsrp"], json!("-97"));
        assert_eq!(v["exchanges"][0]["response"]["net_mode"], json!("static"));
        assert_eq!(v["exchanges"][3]["response"]["band"], json!("3"));
        assert!(!text.contains("11.0.2.21"));
        assert!(v["exchanges"][4]["response"]
            .as_str()
            .unwrap()
            .contains("<band>3</band>"));
    }

    #[test]
    fn text_that_arrives_in_pieces_is_hidden() {
        let mut rec = recorder();
        poll(
            &mut rec,
            "1",
            "http://192.168.8.1/api/x",
            "text/xml",
            "<response><result>a1b2&amp;c3d4&amp;e5f6</result><status>ab<!-- x -->cd</status><band>3</band></response>",
        );
        let (text, v) = built(rec, &opts());
        for gone in ["a1b2", "c3d4", "e5f6", ">ab", "cd<"] {
            assert!(!text.contains(gone), "{gone} is in the file");
        }
        assert!(v["exchanges"][0]["response"]
            .as_str()
            .unwrap()
            .contains("<band>3</band>"));
    }

    #[test]
    fn a_script_is_included_only_when_it_is_a_file_the_router_keeps() {
        let mut rec = recorder();
        let mut script = |id: &str, url: &str, headers: Value, body: &str| {
            sent(&mut rec, id, "GET", url, "Script", json!({}), json!({}));
            answered(&mut rec, id, 200, "application/javascript", headers);
            finished(&mut rec, id);
            rec.response_body(
                id,
                &json!({ "body": body, "base64Encoded": false }).to_string(),
            );
        };
        script(
            "1",
            "http://192.168.8.1/js/app.js",
            json!({ "ETag": "1" }),
            "function app(){}",
        );
        script(
            "2",
            "http://192.168.8.1/js/lib.js",
            json!({ "Last-Modified": "x" }),
            "function lib(){}",
        );
        // Made on the spot: nothing says the router keeps it as a file.
        script(
            "3",
            "http://192.168.8.1/js/runtime.js",
            json!({}),
            "var ssid = 'made-now';",
        );
        script(
            "4",
            "http://192.168.8.1/js/status.js?cb=1",
            json!({ "ETag": "1" }),
            "cb({a:1})",
        );
        let with_code = BuildOptions {
            include_code: true,
            ..opts()
        };
        let (text, v) = built(rec, &with_code);
        let x = &v["exchanges"];
        assert_eq!(x[0]["text"], json!("function app(){}"));
        assert_eq!(x[1]["text"], json!("function lib(){}"));
        assert!(x[2].get("text").is_none() && x[3].get("text").is_none());
        assert!(!text.contains("made-now") && !text.contains("cb({a:1})"));
    }

    #[test]
    fn a_device_model_in_a_list_is_not_the_routers() {
        let model_of = |body: &str| {
            let mut rec = recorder();
            poll(
                &mut rec,
                "1",
                "http://192.168.8.1/api/info",
                "application/json",
                body,
            );
            built(rec, &opts()).1["router"]["model"].clone()
        };
        assert_eq!(
            model_of(r#"{"clients":[{"model":"iPhone 12"}]}"#),
            json!(null)
        );
        assert_eq!(
            model_of(r#"{"model":"MC888","firmware_version":"1.0.1"}"#),
            json!("MC888")
        );
        assert_eq!(model_of(r#"{"model_name":"MC8020"}"#), json!("MC8020"));
    }

    #[test]
    fn a_header_name_that_is_not_a_name_is_starred() {
        let mut rec = recorder();
        let headers = json!({ "X-Hibi Laptop": "1", "X-Requested-With": "XMLHttpRequest", "hibi_home_net": "1" });
        let request = json!({ "headers": headers });
        exchange(
            &mut rec,
            "1",
            "GET",
            "http://192.168.8.1/api/wlan",
            "XHR",
            request,
            "application/json",
            r#"{"WifiSsid":"hibi_home_net"}"#,
        );
        let (text, v) = built(rec, &opts());
        assert_eq!(
            v["exchanges"][0]["requestHeaders"],
            json!(["*", "*", "X-Requested-With"])
        );
        assert!(!text.contains("Hibi") && !text.contains("hibi_home_net"));
    }

    #[test]
    fn a_very_long_redirect_is_not_kept() {
        let mut rec = recorder();
        sent(
            &mut rec,
            "1",
            "GET",
            "http://192.168.8.1/go",
            "Document",
            json!({}),
            json!({}),
        );
        let long = format!("http://192.168.8.1/{}", "a".repeat(100 * 1024));
        let hop = json!({ "redirectResponse": { "status": 302, "headers": {} } });
        sent(&mut rec, "1", "GET", &long, "Document", json!({}), hop);
        assert_eq!(
            rec.into_exchanges()[0].redirect_to.as_deref(),
            Some(TOO_LONG)
        );
    }

    // ---- The whole build: numbers, size, and secrets planted at random ----

    #[test]
    fn a_number_hidden_in_one_place_is_not_kept_in_another() {
        let mut rec = recorder();
        poll(
            &mut rec,
            "1",
            "http://192.168.8.1/api/cell",
            "application/json",
            r#"{"tac":"1300","band":"3"}"#,
        );
        poll(
            &mut rec,
            "2",
            "http://192.168.8.1/api/radio",
            "application/json",
            r#"{"earfcn":1300,"band":3}"#,
        );
        let (_, v) = built(rec, &opts());
        assert_ne!(v["exchanges"][1]["response"]["earfcn"], json!(1300));
        assert_eq!(v["exchanges"][1]["response"]["band"], json!(3));
    }

    /// A small generator with a fixed seed, so a failure can be run again.
    struct Dice(u64);

    impl Dice {
        fn roll(&mut self, sides: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 33) as usize) % sides
        }

        fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
            from[self.roll(from.len())]
        }
    }

    /// A new marked secret, in one of the shapes a secret takes: a word with digits in mixed
    /// case, words with hyphens, capitals with digits, a hash.
    fn plant(markers: &mut Vec<String>, dice: &mut Dice, seed: u64) -> String {
        let n = markers.len();
        let m = match dice.roll(4) {
            0 => format!("Zq{n}Marker7x{seed}"),
            1 => format!("zq-marker-{n}-{seed}-home"),
            2 => format!("ZQMARK{n:04}X{seed:04}Q9"),
            _ => format!("zq{n:02}{seed:02}c0ffee11d00d22beef33"),
        };
        markers.push(m.clone());
        m
    }

    #[test]
    fn secrets_planted_at_random_never_reach_the_file_and_never_stop_the_save() {
        // Names whose values must be hidden: sensitive, or simply not on the keep list.
        const HIDING: &[&str] = &[
            "password",
            "WifiSsid",
            "wpa_psk",
            "imei",
            "session",
            "token",
            "hostname",
            "DeviceName",
            "note",
            "value",
            "data",
            "comment",
            "sn",
            "user",
            "mac_address",
            "msg",
        ];
        // Names that are kept, with values that are what they say.
        const KEPT: &[(&str, &str)] = &[
            ("lte_rsrp", "-97"),
            ("sinr", "12dB"),
            ("band", "3"),
            ("network_type", "5G NSA"),
            ("status", "connected"),
            ("mode", "auto"),
            ("error_code", "0"),
        ];
        for seed in 0..40u64 {
            let mut dice = Dice(seed * 7919 + 1);
            let mut markers: Vec<String> = Vec::new();
            let mut rec = recorder();
            for i in 0..12 {
                let id = i.to_string();
                // A body of random depth: kept readings beside hidden values.
                let mut body = Map::new();
                // The last value planted in this body, which nothing after it can overwrite.
                let mut in_body = String::new();
                for _ in 0..(2 + dice.roll(5)) {
                    let (k, v) = KEPT[dice.roll(KEPT.len())];
                    body.insert(k.to_string(), json!(v));
                    in_body = plant(&mut markers, &mut dice, seed);
                    let hidden = json!(in_body);
                    let name = dice.pick(HIDING).to_string();
                    match dice.roll(3) {
                        0 => {
                            body.insert(name, hidden);
                        }
                        1 => {
                            body.insert("list".into(), json!([{ name: hidden }, { "band": "3" }]));
                        }
                        _ => {
                            // The marker as a name: data where a name should be.
                            let key = plant(&mut markers, &mut dice, seed);
                            body.insert("map".into(), json!({ key: { name: hidden } }));
                        }
                    }
                }
                let body = Value::Object(body).to_string();
                match dice.roll(5) {
                    0 => {
                        // A login: everything it sends and gets is hidden.
                        let sent = json!({ "postData": json!({ "password": plant(&mut markers, &mut dice, seed), "band": plant(&mut markers, &mut dice, seed) }).to_string() });
                        post(
                            &mut rec,
                            &id,
                            "http://192.168.8.1/api/login",
                            sent,
                            "application/json",
                            &body,
                        );
                    }
                    1 => {
                        // A value in a header, and a path that names what the answer hides.
                        let headers = json!({ "X-Token": plant(&mut markers, &mut dice, seed), "Cookie": plant(&mut markers, &mut dice, seed) });
                        let url = format!("http://192.168.8.1/api/hosts/{in_body}/info");
                        exchange(
                            &mut rec,
                            &id,
                            "GET",
                            &url,
                            "XHR",
                            json!({ "headers": headers }),
                            "application/json",
                            &body,
                        );
                    }
                    2 => {
                        // Another host, a page and a script: none of their text is kept.
                        let m = plant(&mut markers, &mut dice, seed);
                        sent(
                            &mut rec,
                            &id,
                            "GET",
                            &format!("http://other.example/{m}"),
                            "Fetch",
                            json!({}),
                            json!({}),
                        );
                        let page = format!(
                            "<html><input value=\"{}\"></html>",
                            plant(&mut markers, &mut dice, seed)
                        );
                        exchange(
                            &mut rec,
                            &format!("p{id}"),
                            "GET",
                            "http://192.168.8.1/index.html",
                            "Document",
                            json!({}),
                            "text/html",
                            &page,
                        );
                        let script = format!("var k = '{}';", plant(&mut markers, &mut dice, seed));
                        exchange(
                            &mut rec,
                            &format!("s{id}"),
                            "GET",
                            "http://192.168.8.1/js/a.js?x=1",
                            "Script",
                            json!({}),
                            "application/javascript",
                            &script,
                        );
                    }
                    3 => {
                        let xml = format!(
                            "<response><WifiSsid>{}</WifiSsid><rsrp>-95dBm</rsrp><Hosts><Host><HostName>{}</HostName></Host></Hosts></response>",
                            plant(&mut markers, &mut dice, seed),
                            plant(&mut markers, &mut dice, seed)
                        );
                        poll(
                            &mut rec,
                            &id,
                            "http://192.168.8.1/api/xml",
                            "text/xml",
                            &xml,
                        );
                    }
                    _ => {
                        let url = format!(
                            "http://192.168.8.1/api/x?token={}&band=3",
                            plant(&mut markers, &mut dice, seed)
                        );
                        poll(&mut rec, &id, &url, "application/json", &body);
                    }
                }
            }
            let list = rec.into_exchanges();
            let text = match build(&list, &opts()) {
                Ok(text) => text,
                Err(found) => panic!("seed {seed}: the save was refused: {found:?}"),
            };
            for m in &markers {
                // Where it is, when it is there: the name it sits under says which rule let it by.
                let near = text.find(m.as_str()).map(|at| {
                    let from = text[..at]
                        .char_indices()
                        .rev()
                        .nth(160)
                        .map_or(0, |(i, _)| i);
                    &text[from..at]
                });
                assert!(
                    near.is_none(),
                    "seed {seed}: {m} is in the file, after: {near:?}"
                );
            }
            // And the readings beside them are still there.
            assert!(
                text.contains("\"band\": \"3\""),
                "seed {seed}: the readings are gone"
            );
        }
    }

    /// Not part of the usual run: `cargo test -p router-core capture -- --ignored --nocapture`.
    /// The largest recording there can be, to see that cleaning it stays a matter of seconds.
    #[test]
    #[ignore = "a timing run, for a person to read"]
    fn the_largest_recording_is_cleaned_in_seconds() {
        let mut rec = recorder();
        // About 300 hidden values, 500 exchanges and close to 20 MiB.
        let mut fields = Map::new();
        for i in 0..300 {
            fields.insert(
                format!("host_name_{i}"),
                json!(format!("Device-{i}-of-the-house")),
            );
            fields.insert(format!("lte_rsrp_{i}"), json!("-97"));
        }
        let small = Value::Object(fields).to_string();
        let filler = format!(
            "{{\"status\":\"connected\",\"pad\":\"{}\"}}",
            "x".repeat(1_900_000)
        );
        let script = format!("function app(){{ return 1; }} // {}", "y".repeat(1_900_000));
        for i in 0..MAX_EXCHANGES {
            let id = i.to_string();
            match i {
                0..=4 => poll(
                    &mut rec,
                    &id,
                    "http://192.168.8.1/api/big",
                    "application/json",
                    &filler,
                ),
                5..=8 => exchange(
                    &mut rec,
                    &id,
                    "GET",
                    "http://192.168.8.1/js/app.js",
                    "Script",
                    json!({}),
                    "application/javascript",
                    &script,
                ),
                _ if i % 50 == 0 => poll(
                    &mut rec,
                    &id,
                    "http://192.168.8.1/api/hosts",
                    "application/json",
                    &small,
                ),
                _ => poll(
                    &mut rec,
                    &id,
                    "http://192.168.8.1/api/status",
                    "application/json",
                    r#"{"lte_rsrp":"-97","band":"3"}"#,
                ),
            }
        }
        let list = rec.into_exchanges();
        let bytes: usize = list
            .iter()
            .map(|x| x.body.as_ref().map_or(0, String::len))
            .sum();
        for include_code in [false, true] {
            let started = std::time::Instant::now();
            let text = build(
                &list,
                &BuildOptions {
                    include_code,
                    ..opts()
                },
            )
            .expect("a clean file");
            println!(
                "{} exchanges, {:.1} MiB recorded, code {}: cleaned in {:.1?}, {:.1} MiB written",
                list.len(),
                bytes as f64 / 1048576.0,
                if include_code { "included" } else { "left out" },
                started.elapsed(),
                text.len() as f64 / 1048576.0
            );
            assert!(started.elapsed() < std::time::Duration::from_secs(60));
        }
    }
}
