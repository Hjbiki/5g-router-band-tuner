//! The built-in capture: what a router's own web page asked the router, cleaned for sharing.
//!
//! A driver for a new router needs to see how that router's page logs in, reads the signal and
//! changes bands. `Recorder` takes the browser's network events as they happen (the DevTools
//! protocol's JSON, nothing else) and keeps only what the page asked the router itself. `build`
//! turns that into the file a person can attach to an issue.
//!
//! The file is meant to be public, so this is the part that has to be right:
//!
//! - A header's value is never stored, only its name. So no cookie, authorization or token header
//!   can be in the file, whatever a router calls it.
//! - A request to any other host is never stored.
//! - A login exchange keeps its names and hides every value, the length too
//!   (`Rules::ShapeOnly`). A password's hash can be as sensitive as the password.
//! - Every other body is parsed and goes through `Rules::Capture`: a value stays only under a name
//!   known to be technical. A body that is neither XML, JSON nor a form is left out.
//! - Pages and scripts are the router maker's code and can hold anything: by default only their
//!   path, status and the API paths in them are kept.
//! - The finished file is scanned, and a hit means nothing is written.

use std::collections::{BTreeSet, HashMap};

use base64::Engine as _;
use reqwest::Url;
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::diagnostics::{redact_identifier_tokens, Redactor, Rules};
use crate::probe;

/// Format tag of the saved file.
pub const FORMAT: &str = "router-band-tuner-capture/1";
pub const MAX_EXCHANGES: usize = 500;
pub const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: usize = 20 * 1024 * 1024;

const NOTE_STATIC: &str = "a static file: no body kept";
const NOTE_REDIRECT: &str = "a redirect: its own body isn't available";
const NOTE_UNFINISHED: &str = "the page left before this answer finished";
const NOTE_MISSING: &str = "the body wasn't available";
const NOTE_TOO_LARGE: &str = "larger than 2 MiB: left out";
const NOTE_CODE: &str = "the router maker's code: left out";
const OTHER_HOST: &str = "(another address)";
const MAX_API_PATHS: usize = 60;
const MAX_TITLES: usize = 5;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
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
/// cleaned yet except that header values were never taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exchange {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub kind: Kind,
    /// Names only.
    pub request_headers: Vec<String>,
    pub request_type: Option<String>,
    pub request_body: Option<String>,
    pub status: Option<u16>,
    pub content_type: Option<String>,
    /// Names only.
    pub response_headers: Vec<String>,
    pub body: Option<String>,
    /// Why there is no body, when there is none.
    pub note: Option<&'static str>,
    pub redirect_to: Option<String>,
}

fn header_names(headers: &Value) -> Vec<String> {
    let mut names: Vec<String> = headers
        .as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    names.sort_by_key(|n| n.to_ascii_lowercase());
    names
}

/// The media type of a `Content-Type` header, without its parameters.
fn media_type(headers: &Value) -> Option<String> {
    let (_, v) = headers
        .as_object()?
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))?;
    let kind = v.as_str()?.split(';').next()?.trim().to_ascii_lowercase();
    (!kind.is_empty()).then_some(kind)
}

/// Keeps what the router's own page asked the router. Feed it every `Network.*` event.
pub struct Recorder {
    origin: String,
    list: Vec<Exchange>,
    /// A request id's newest exchange. A redirect keeps the id and starts a new exchange.
    open: HashMap<String, usize>,
    bytes: usize,
    limit: Option<Limit>,
}

impl Recorder {
    pub fn new(router: &Url) -> Self {
        Self {
            origin: router.origin().ascii_serialization(),
            list: Vec::new(),
            open: HashMap::new(),
            bytes: 0,
            limit: None,
        }
    }

    fn ours(&self, url: &str) -> Option<Url> {
        let url = Url::parse(url).ok()?;
        (url.origin().ascii_serialization() == self.origin).then_some(url)
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
                x.content_type = response["mimeType"]
                    .as_str()
                    .filter(|m| !m.is_empty())
                    .map(str::to_string);
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
        let mut x = Exchange {
            method: request["method"].as_str().unwrap_or("GET").to_string(),
            path: url.path().to_string(),
            query: url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect(),
            kind: Kind::from_cdp(v["type"].as_str().unwrap_or("")),
            request_headers: header_names(&request["headers"]),
            request_type: media_type(&request["headers"]),
            request_body: None,
            status: None,
            content_type: None,
            response_headers: Vec::new(),
            body: None,
            note: None,
            redirect_to: None,
        };
        let mut ask = None;
        if let Some(body) = request["postData"].as_str() {
            x.request_body = self.take(body.to_string());
        } else if request["hasPostData"].as_bool() == Some(true) {
            ask = Some(Ask::RequestBody(id.clone()));
        }
        self.open.insert(id, self.list.len());
        self.list.push(x);
        ask
    }

    /// Counts a body against the limits. `None` when it is too large or the recording is full.
    fn take(&mut self, text: String) -> Option<String> {
        if text.len() > MAX_BODY_BYTES || self.limit.is_some() {
            return None;
        }
        if self.bytes + text.len() > MAX_TOTAL_BYTES {
            self.limit = Some(Limit::Size);
            return None;
        }
        self.bytes += text.len();
        Some(text)
    }

    /// The browser's answer to `Network.getResponseBody` for this request.
    pub fn response_body(&mut self, id: &str, json: &str) {
        let Some(&i) = self.open.get(id) else { return };
        let text = serde_json::from_str::<Value>(json).ok().and_then(|v| {
            let body = v["body"].as_str()?;
            if v["base64Encoded"].as_bool() == Some(true) {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(body)
                    .ok()?;
                // Routers send gzip without saying so (the H122 does), and the browser hands the
                // raw bytes over.
                Some(probe::decode(&bytes))
            } else {
                Some(body.to_string())
            }
        });
        let (body, note) = match text {
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
        let Some(&i) = self.open.get(id) else { return };
        let Some(text) = serde_json::from_str::<Value>(json)
            .ok()
            .and_then(|v| v["postData"].as_str().map(str::to_string))
        else {
            return;
        };
        let kept = self.take(text);
        if let Some(x) = self.list.get_mut(i) {
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
    /// The user's choice: pages and scripts are the router maker's code.
    pub include_code: bool,
    pub limit: Option<Limit>,
}

/// A login exchange, by its path, its query or what it sends. Judged widely on purpose: a wrong
/// yes only hides values, a wrong no could publish a password's hash.
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
    const IN_BODY: &[&str] = &[
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
    ];
    // A field named just `pw`, in a form, JSON or XML.
    const PW_FIELD: &[&str] = &["pw=", "\"pw\"", "<pw>"];
    let path = probe::fully_decoded(&x.path).to_ascii_lowercase();
    if IN_PATH.iter().any(|w| path.contains(w)) {
        return true;
    }
    let mut sent = x.request_body.clone().unwrap_or_default();
    for (k, v) in &x.query {
        sent.push_str(k);
        sent.push(' ');
        sent.push_str(v);
        sent.push(' ');
    }
    let sent = sent.to_ascii_lowercase();
    IN_BODY.iter().chain(PW_FIELD).any(|w| sent.contains(w))
}

/// `a=1&b=2`, by its declared type or by its look.
fn form_fields(body: &str, content_type: Option<&str>) -> Option<Map<String, Value>> {
    let text = body.trim();
    let declared = content_type.is_some_and(|c| c.contains("x-www-form-urlencoded"));
    let looks = !text.starts_with(['{', '[', '<'])
        && !text.contains(char::is_whitespace)
        && text.split('&').all(|pair| pair.contains('='));
    if text.is_empty() || !(declared || looks) {
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

/// A name the file shows as it is (a header's, a path's): still with no identifier inside it.
fn name_text(s: &str) -> String {
    redact_identifier_tokens(s).0
}

/// What the recording says the router is. Joined to the file after the scan, by the label rule,
/// because versions look like addresses.
#[derive(Default)]
struct Facts {
    vendor: Option<&'static str>,
    model: Option<String>,
    hardware: Option<String>,
    firmware: Option<String>,
    titles: Vec<String>,
}

impl Facts {
    fn read(&mut self, x: &Exchange) {
        let Some(text) = x.body.as_deref() else {
            return;
        };
        if self.vendor.is_none() {
            self.vendor = probe::vendor_in(text);
        }
        if matches!(x.kind, Kind::Page) {
            if let Some(t) = probe::title_of(text) {
                let plain = t.is_ascii() && !t.contains('@');
                if plain && self.titles.len() < MAX_TITLES && !self.titles.contains(&t) {
                    self.titles.push(t);
                }
            }
        }
        if x.kind == Kind::Script {
            return;
        }
        let hardware = probe::hardware_field(text);
        let firmware = probe::firmware_field(text);
        // A list of the devices on the network has "device name" fields too. A model is believed
        // only beside a version, or when it is shaped like a router's model name.
        if let Some(model) = probe::model_field(text) {
            let described = hardware.is_some() || firmware.is_some();
            if self.model.is_none() && (described || probe::model_in_text(&model).is_some()) {
                self.model = Some(model);
            }
        }
        if self.hardware.is_none() {
            self.hardware = hardware;
        }
        if self.firmware.is_none() {
            self.firmware = firmware;
        }
    }
}

fn entry(red: &mut Redactor, n: usize, x: &Exchange, include_code: bool) -> Value {
    let label = format!("x{n}");
    let login = login_shaped(x);
    red.set_rules(if login {
        Rules::ShapeOnly
    } else {
        Rules::Capture
    });

    let mut out = Map::new();
    out.insert("n".into(), json!(n));
    out.insert("method".into(), json!(name_text(&x.method)));
    out.insert("path".into(), json!(name_text(&probe::clean_path(&x.path))));
    out.insert("kind".into(), json!(x.kind));
    if login {
        out.insert("login".into(), json!(true));
    }
    if !x.query.is_empty() {
        let fields: Map<String, Value> = x
            .query
            .iter()
            .map(|(k, v)| (name_text(k), Value::String(v.clone())))
            .collect();
        out.insert(
            "query".into(),
            red.json(&Value::Object(fields), &format!("{label}.query")),
        );
    }
    let names = |list: &[String]| -> Vec<String> { list.iter().map(|h| name_text(h)).collect() };
    out.insert("requestHeaders".into(), json!(names(&x.request_headers)));
    if let Some(kind) = &x.request_type {
        out.insert("requestType".into(), json!(name_text(kind)));
    }
    if let Some(body) = &x.request_body {
        let at = format!("{label}.request");
        let sent = match form_fields(body, x.request_type.as_deref()) {
            Some(fields) => red.json(&Value::Object(fields), &at),
            None => red.body(body, &at),
        };
        out.insert("request".into(), sent);
    }
    out.insert("status".into(), json!(x.status));
    if let Some(kind) = &x.content_type {
        out.insert("type".into(), json!(name_text(kind)));
    }
    out.insert("responseHeaders".into(), json!(names(&x.response_headers)));
    if let Some(to) = &x.redirect_to {
        out.insert(
            "redirectTo".into(),
            json!(name_text(&probe::clean_path(to))),
        );
    }

    let mut note = x.note;
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
                .map(|p| name_text(p))
                .collect();
            out.insert("apiPaths".into(), json!(paths));
            if include_code {
                out.insert("text".into(), json!(redact_identifier_tokens(text).0));
            } else {
                note = Some(NOTE_CODE);
            }
        }
        _ => {}
    }
    if let Some(note) = note {
        out.insert("note".into(), json!(note));
    }
    red.set_rules(Rules::Capture);
    Value::Object(out)
}

/// The cleaned file. `Err` lists what the last scan found in it: nothing may be written then.
pub fn build(exchanges: &[Exchange], opts: &BuildOptions<'_>) -> Result<String, Vec<String>> {
    let mut red = Redactor::with_rules(Rules::Capture);
    let mut facts = Facts::default();
    let mut list = Vec::with_capacity(exchanges.len());
    for (i, x) in exchanges.iter().enumerate() {
        facts.read(x);
        list.push(entry(&mut red, i + 1, x, opts.include_code));
    }
    let titles: Vec<String> = facts.titles.iter().map(|t| name_text(t)).collect();
    let mut file = json!({
        "format": FORMAT,
        "appVersion": opts.app_version,
        "generatedAtMs": opts.generated_at_ms,
        "codeIncluded": opts.include_code,
        "limitHit": opts.limit,
        "router": { "vendor": facts.vendor, "titles": titles },
        "exchanges": list,
    });
    let text = serde_json::to_string_pretty(&file).map_err(|e| vec![e.to_string()])?;
    let leaks = red.leaks(&text);
    if !leaks.is_empty() {
        return Err(leaks);
    }
    // Versions look like IP addresses, so these join after the scan, by their own rule.
    let label = |s: &Option<String>| {
        s.as_deref()
            .filter(|v| probe::is_plain_label(v))
            .map(str::to_string)
    };
    file["router"]["model"] = json!(label(&facts.model));
    file["router"]["hardware"] = json!(label(&facts.hardware));
    file["router"]["firmware"] = json!(label(&facts.firmware));
    serde_json::to_string_pretty(&file).map_err(|e| vec![e.to_string()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::HIDDEN;

    const ROUTER: &str = "http://192.168.8.1";

    fn recorder() -> Recorder {
        Recorder::new(&Url::parse(ROUTER).unwrap())
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
        answered(rec, id, 200, mime, json!({ "Content-Type": mime }));
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
        let chunk = "b".repeat(MAX_BODY_BYTES);
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
        assert_eq!(body["hosts"][0]["InterfaceType"], json!("Wireless"));
        // A hidden value keeps its name and its shape, so the structure can still be read.
        assert_eq!(
            body["hosts"][0]["DeviceName"].as_str().unwrap().len(),
            "Hibi-iPhone 12".len()
        );
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
        assert_eq!(
            body["result"],
            json!("seen at [redacted-ipv4] by [redacted-number]")
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
            (&json!(NOTE_CODE), &json!(NOTE_CODE))
        );
        assert!(x[0].get("text").is_none() && x[1].get("text").is_none());
        assert!(!text.contains("function load") && !text.contains("var mac"));
        assert_eq!(v["router"]["titles"], json!(["5G CPE"]));

        let with_code = BuildOptions {
            include_code: true,
            ..opts()
        };
        let (text, v) = built(record(), &with_code);
        let x = &v["exchanges"];
        assert_eq!(v["codeIncluded"], json!(true));
        assert!(x[1]["text"]
            .as_str()
            .unwrap()
            .starts_with("function load(){"));
        assert!(x[0].get("note").is_none());
        assert!(!text.contains("AA:BB:CC:DD:EE:FF") && !text.contains("356789012345678"));
    }

    #[test]
    fn the_last_scan_refuses_a_file_that_still_holds_a_secret() {
        // The Wi-Fi name is hidden where it is named, and the same text sits in the page's code.
        let mut rec = recorder();
        exchange(
            &mut rec,
            "1",
            "GET",
            "http://192.168.8.1/api/wlan",
            "Fetch",
            json!({}),
            "application/json",
            r#"{"WifiSsid":"Hibi-Home-Net"}"#,
        );
        exchange(
            &mut rec,
            "2",
            "GET",
            "http://192.168.8.1/js/boot.js",
            "Script",
            json!({}),
            "application/javascript",
            "var current = 'Hibi-Home-Net';",
        );
        let list = rec.into_exchanges();
        assert!(build(&list, &opts()).is_ok());
        let with_code = BuildOptions {
            include_code: true,
            ..opts()
        };
        let found = build(&list, &with_code).unwrap_err();
        assert!(!found.is_empty());
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
}
