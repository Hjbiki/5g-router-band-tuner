//! Redacted diagnostics export for driver authors: raw responses of a router's read-only
//! endpoints plus the event-log tail, with every identifier replaced by a synthetic value of the
//! same shape. Ported from `tools/fixtures/redact-har.mjs` (rules and leak scan). Redaction
//! happens here, before anything is written, and a leak-scan hit blocks the export.
//!
//! A body is parsed, not pattern-matched, so attributes, CDATA and everything under a sensitive
//! element are covered. A body that's neither XML nor JSON is left out rather than guessed at.

use std::borrow::Cow;
use std::collections::BTreeSet;

use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesStart, BytesText, Event};
use quick_xml::name::QName;
use quick_xml::{Reader, Writer};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::{EventRecord, RouterError};

/// Format tag of the exported file.
pub const FORMAT: &str = "router-band-tuner-diagnostics/1";
/// How many event-log lines the export keeps.
pub const EVENT_TAIL: usize = 200;
/// Stands in for a body that couldn't be parsed, so nothing in it can leak.
const LEFT_OUT: &str = "(left out: not well-formed XML or JSON)";

/// Tags known to be secret, identifying or address-like (redact-har.mjs `SENSITIVE_TAGS`).
const SENSITIVE_TAGS: &[&str] = &[
    "token",
    "salt",
    "servernonce",
    "firstnonce",
    "finalnonce",
    "clientproof",
    "serversignature",
    "rsapubkeysignature",
    "rsan",
    "serialnumber",
    "imei",
    "imsi",
    "iccid",
    "msisdn",
    "macaddress1",
    "macaddress2",
    "wifimacaddrwl0",
    "wifimacaddrwl1",
    "wanipaddress",
    "wanipv6address",
    "wan_dns_address",
    "wan_ipv6_dns_address",
    "submask",
    "primarydns",
    "secondarydns",
    "primaryipv6dns",
    "secondaryipv6dns",
    "ip",
    "cell_id",
    "enodeb_id",
    "pci",
    "nrpci",
    "scc_pci",
    "tac",
    "lac",
    "rac",
    "nci",
    "eci",
    "ecgi",
    "sc",
    "bsic",
    "esn",
    "sn",
    "lat",
    "lon",
    "lng",
];

/// An unknown router names things differently, so any tag or key containing one of these is
/// treated as sensitive too. Over-redaction only loses detail; under-redaction leaks.
const SENSITIVE_PARTS: &[&str] = &[
    "imei",
    "imsi",
    "iccid",
    "serial",
    "mac",
    "msisdn",
    "ipaddr",
    "ipv4",
    "ipv6",
    "dns",
    "mask",
    "gateway",
    "token",
    "nonce",
    "salt",
    "proof",
    "signature",
    "rsa",
    "ssid",
    "password",
    "passwd",
    "pwd",
    "key",
    "cell",
    "enodeb",
    "gnodeb",
    "enb",
    "gnb",
    "meid",
    "user",
    "hostname",
    "latitude",
    "longitude",
    "tac",
    "lac",
    "pci",
    "phone",
    "nei",
    "sector",
];

pub fn is_sensitive(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    SENSITIVE_TAGS.contains(&n.as_str()) || SENSITIVE_PARTS.iter().any(|p| n.contains(p))
}

/// Replaces values and remembers what it replaced, for the leak scan.
#[derive(Default)]
pub struct Redactor {
    secrets: BTreeSet<String>,
    generated: BTreeSet<String>,
    counter: u32,
}

fn synth(kind: &str, len: usize, label: &str) -> String {
    let mut out = String::new();
    let mut i = 0u32;
    while out.len() < len {
        out.push_str(&hex::encode(Sha256::digest(format!("fixture:{label}:{i}"))));
        i += 1;
    }
    out.truncate(len);
    match kind {
        "digits" => out
            .chars()
            .map(|c| match c {
                'a'..='f' => char::from(b'0' + (c as u8) % 10),
                _ => c,
            })
            .collect(),
        "alnum" => out
            .chars()
            .enumerate()
            .map(|(k, c)| {
                if k % 3 == 0 {
                    c.to_ascii_uppercase()
                } else {
                    c
                }
            })
            .collect(),
        _ => out,
    }
}

fn is_ipv4(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|p| (1..=3).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit()))
}

pub(crate) fn is_mac(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 17
        && b.iter().enumerate().all(|(i, c)| {
            if i % 3 == 2 {
                *c == b':' || *c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

impl Redactor {
    pub fn new() -> Self {
        Self::default()
    }

    /// A synthetic value shaped like `value` (redact-har.mjs `synthLike`). Empty stays empty.
    pub fn value(&mut self, value: &str, label: &str) -> String {
        if value.is_empty() {
            return String::new();
        }
        self.secrets.insert(value.to_string());
        self.counter += 1;
        let n = self.counter;
        let v = value;
        let fake = if is_ipv4(v) {
            format!("192.0.2.{}", n % 250 + 1)
        } else if is_mac(v) {
            format!("02:00:00:00:00:{:02X}", n % 256)
        } else if v.contains(':')
            && v.chars()
                .all(|c| c.is_ascii_hexdigit() || ":.,; ".contains(c))
        {
            "2001:db8::1".to_string()
        } else if v.contains('.') && v.chars().all(|c| c.is_ascii_digit() || ".,; ".contains(c)) {
            "192.0.2.53,192.0.2.54".to_string()
        } else if v.bytes().all(|b| b.is_ascii_digit()) {
            synth("digits", v.len(), label)
        } else if v.bytes().all(|b| b.is_ascii_hexdigit()) {
            synth("hex", v.len(), label)
        } else {
            synth("alnum", v.len(), label)
        };
        self.generated.insert(fake.clone());
        fake
    }

    /// Redacts all text under a sensitive element and every sensitive attribute. Plain text comes
    /// back as it is, and text with a `<` that doesn't parse as XML is left out.
    pub fn xml(&mut self, xml: &str, label: &str) -> String {
        if !xml.contains('<') {
            return xml.to_string();
        }
        self.rewrite_xml(xml, label)
            .unwrap_or_else(|| LEFT_OUT.to_string())
    }

    fn rewrite_xml(&mut self, xml: &str, label: &str) -> Option<String> {
        let mut reader = Reader::from_str(xml);
        let mut writer = Writer::new(Vec::new());
        // The open elements: name, and whether they or a parent are sensitive.
        let mut open: Vec<(String, bool)> = Vec::new();
        loop {
            let secret = open.last().is_some_and(|o| o.1);
            let at = format!(
                "{label}:{}",
                open.last().map(|o| o.0.as_str()).unwrap_or("")
            );
            let event = match reader.read_event().ok()? {
                Event::Eof => break,
                Event::Start(e) => {
                    let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                    let hide = secret || is_sensitive(&name);
                    let tag = self.xml_tag(&e, &name, hide, label)?;
                    open.push((name, hide));
                    Event::Start(tag)
                }
                Event::Empty(e) => {
                    let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                    let hide = secret || is_sensitive(&name);
                    Event::Empty(self.xml_tag(&e, &name, hide, label)?)
                }
                Event::End(e) => {
                    open.pop()?;
                    Event::End(e)
                }
                Event::Text(t) if secret && !t.iter().all(u8::is_ascii_whitespace) => {
                    let fake = self.value(&String::from_utf8_lossy(&t), &at);
                    Event::Text(BytesText::from_escaped(fake))
                }
                Event::CData(c) if secret => {
                    let fake = self.value(&String::from_utf8_lossy(&c), &at);
                    Event::Text(BytesText::from_escaped(fake))
                }
                Event::Comment(_) | Event::DocType(_) | Event::PI(_) => continue,
                Event::GeneralRef(_) if secret => continue,
                other => other,
            };
            writer.write_event(event).ok()?;
        }
        if !open.is_empty() {
            return None;
        }
        String::from_utf8(writer.into_inner()).ok()
    }

    fn xml_tag(
        &mut self,
        tag: &BytesStart<'_>,
        name: &str,
        hide: bool,
        label: &str,
    ) -> Option<BytesStart<'static>> {
        let mut out = BytesStart::new(name.to_string());
        for attr in tag.attributes() {
            let attr = attr.ok()?;
            let key = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
            let value = if hide || is_sensitive(&key) {
                let raw = String::from_utf8_lossy(&attr.value).into_owned();
                let at = format!("{label}:{name}@{key}");
                Cow::Owned(self.value(&raw, &at).into_bytes())
            } else {
                attr.value
            };
            out.push_attribute(Attribute {
                key: QName(key.as_bytes()),
                value,
            });
        }
        Some(out)
    }

    /// One response body, by what it is. JSON stays JSON, so its keys can be read.
    pub fn body(&mut self, text: &str, label: &str) -> Value {
        let start = text.trim_start();
        if start.is_empty() || start.starts_with('<') {
            return Value::String(self.xml(text, label));
        }
        match serde_json::from_str::<Value>(text) {
            Ok(v) => self.json(&v, label),
            Err(_) => Value::String(LEFT_OUT.to_string()),
        }
    }

    /// Redacts a JSON value: every field under a sensitive key gets a synthetic value
    /// (numbers stay numbers), recursively.
    pub fn json(&mut self, v: &Value, label: &str) -> Value {
        match v {
            Value::Object(m) => {
                let mut out = Map::new();
                for (k, x) in m {
                    let l = format!("{label}.{k}");
                    let red = if is_sensitive(k) {
                        self.leaf_json(x, &l)
                    } else {
                        self.json(x, &l)
                    };
                    out.insert(k.clone(), red);
                }
                Value::Object(out)
            }
            Value::Array(a) => Value::Array(
                a.iter()
                    .enumerate()
                    .map(|(i, x)| self.json(x, &format!("{label}[{i}]")))
                    .collect(),
            ),
            Value::String(s) => Value::String(self.xml(s, label)),
            other => other.clone(),
        }
    }

    fn leaf_json(&mut self, v: &Value, label: &str) -> Value {
        match v {
            Value::Number(n) => {
                let fake = self.value(&n.to_string(), label);
                fake.parse::<u64>()
                    .map(Value::from)
                    .unwrap_or(Value::String(fake))
            }
            Value::String(s) => Value::String(self.value(s, label)),
            Value::Null | Value::Bool(_) => v.clone(),
            // A nested object under a sensitive key: redact every scalar inside it.
            Value::Object(m) => Value::Object(
                m.iter()
                    .map(|(k, x)| (k.clone(), self.leaf_json(x, &format!("{label}.{k}"))))
                    .collect(),
            ),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.leaf_json(x, label)).collect()),
        }
    }

    /// The leak scan of redact-har.mjs over the finished text. Any hit blocks the export.
    pub fn leaks(&self, text: &str) -> Vec<String> {
        let mut found = Vec::new();
        // An original that already is a documentation value ("2001:db8::1") maps to itself.
        if self
            .secrets
            .iter()
            .any(|s| s.len() >= 6 && !self.generated.contains(s) && text.contains(s.as_str()))
        {
            found.push("an original identifier is still present".to_string());
        }
        // In the finished JSON a line break is `\n`, which would hide what follows it.
        let plain = text
            .replace("\\n", " ")
            .replace("\\t", " ")
            .replace("\\r", " ");
        found.extend(pattern_hits(
            &strip_harmless_fields(&plain),
            &self.generated,
        ));
        found
    }
}

/// Parses `tag>value</tag` at the start of `s` (the `<` already consumed); `value` holds no `<`.
fn leaf(s: &str) -> Option<(&str, &str, usize)> {
    let name_len = s
        .char_indices()
        .take_while(|(i, c)| {
            if *i == 0 {
                c.is_ascii_alphabetic() || *c == '_'
            } else {
                c.is_ascii_alphanumeric() || "_.-".contains(*c)
            }
        })
        .count();
    if name_len == 0 || s.as_bytes().get(name_len) != Some(&b'>') {
        return None;
    }
    let tag = &s[..name_len];
    let body = &s[name_len + 1..];
    let end = body.find('<')?;
    let close = format!("</{tag}>");
    body[end..].starts_with(&close).then(|| {
        let consumed = name_len + 1 + end + close.len();
        (tag, &body[..end], consumed)
    })
}

/// Firmware and app versions look like IPv4 addresses ("11.0.2.21"), and band masks like long
/// numbers. They are not identifiers.
fn strip_harmless_fields(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let after = &rest[lt + 1..];
        match leaf(after) {
            Some((tag, _, consumed))
                if ["version", "band"]
                    .iter()
                    .any(|end| tag.to_ascii_lowercase().ends_with(end)) =>
            {
                rest = &after[consumed..];
            }
            _ => {
                out.push('<');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Tokens bounded like the script's `\b`: maximal runs of `[0-9A-Za-z_.:-]`, then checked whole.
/// The redactor's own values are skipped only as whole tokens: cutting them out of the text would
/// let a short one, like a fake PCI, shorten a real identifier until it passed.
fn pattern_hits(text: &str, generated: &BTreeSet<String>) -> Vec<String> {
    let mut hits = BTreeSet::new();
    for tok in text.split(|c: char| !(word(c) || ".:-".contains(c))) {
        let tok = tok.trim_matches(|c: char| ".:-".contains(c));
        if generated.contains(tok) {
            continue;
        }
        for run in tok.split(|c: char| !c.is_ascii_digit()) {
            // A digit run only counts on its own word, not inside "a123...".
            if run.len() >= 14 && digit_run_is_word(tok, run) {
                hits.insert(match run.len() {
                    15 => "a 15-digit number (IMEI or IMSI)",
                    19 | 20 => "a 19 or 20-digit number (ICCID)",
                    _ => "a number of 14 or more digits",
                });
            }
        }
        let digits = tok.bytes().filter(u8::is_ascii_digit).count();
        if digits >= 14 && tok.contains('-') && tok.bytes().all(|b| b.is_ascii_digit() || b == b'-')
        {
            hits.insert("a number of 14 or more digits");
        }
        if is_ipv6(tok) && !tok.to_ascii_lowercase().starts_with("2001:db8:") {
            hits.insert("an IPv6 address");
        }
        if is_bare_mac(tok) {
            hits.insert("a MAC address");
        }
        for part in tok.split(|c: char| !(c.is_ascii_digit() || c == '.')) {
            let p = part.trim_matches('.');
            if is_ipv4(p) && !p.starts_with("192.0.2.") && p != "0.0.0.0" {
                hits.insert("an IPv4 address");
            }
        }
        if tok.len() >= 17 {
            for i in 0..=tok.len() - 17 {
                if let Some(m) = tok.get(i..i + 17) {
                    if is_mac(m) && !m.to_ascii_uppercase().starts_with("02:00:00:00:00:") {
                        hits.insert("a MAC address");
                    }
                }
            }
        }
    }
    hits.into_iter().map(String::from).collect()
}

/// What the leak scan finds in `text` when nothing was redacted: IMEI- and ICCID-length numbers,
/// IP and MAC addresses. The CSV export's last check before anything is written.
pub fn identifier_hits(text: &str) -> Vec<String> {
    pattern_hits(text, &BTreeSet::new())
}

/// The short name in a `[redacted-<kind>]` placeholder for a `pattern_hits` finding.
fn hit_kind(hit: &str) -> &'static str {
    if hit.contains("IPv4") {
        "ipv4"
    } else if hit.contains("IPv6") {
        "ipv6"
    } else if hit.contains("MAC") {
        "mac"
    } else {
        "number"
    }
}

/// Replaces each token of `text` that the leak scan flags (an IMEI- or ICCID-length number, an
/// IP or MAC address) with `[redacted-<kind>]`, leaving everything else as it was. Tokens are cut
/// exactly as `identifier_hits` cuts them, so what it would find is what goes. Returns the text
/// and one finding per replaced token. For text that isn't a router response (the page-script
/// saver), where nothing can be parsed by name.
pub fn redact_identifier_tokens(text: &str) -> (String, Vec<String>) {
    let none = BTreeSet::new();
    let is_tok = |c: char| word(c) || ".:-".contains(c);
    let mut out = String::with_capacity(text.len());
    let mut found = Vec::new();
    let mut copied = 0;
    let mut start: Option<usize> = None;
    let ends = text.char_indices().map(|(i, c)| (i, is_tok(c)));
    for (i, tok_char) in ends.chain(std::iter::once((text.len(), false))) {
        if tok_char {
            start.get_or_insert(i);
            continue;
        }
        let Some(s) = start.take() else { continue };
        let tok = &text[s..i];
        let core = tok.trim_matches(|c: char| ".:-".contains(c));
        if core.is_empty() {
            continue;
        }
        let hits = pattern_hits(core, &none);
        let Some(first) = hits.first() else { continue };
        let core_start =
            s + (tok.len() - tok.trim_start_matches(|c: char| ".:-".contains(c)).len());
        out.push_str(&text[copied..core_start]);
        out.push_str("[redacted-");
        out.push_str(hit_kind(first));
        out.push(']');
        copied = core_start + core.len();
        found.push(first.clone());
    }
    out.push_str(&text[copied..]);
    (out, found)
}

/// Compressed (`fe80::1a2b`) or full. Times like `12:34:56` have too few groups to count.
fn is_ipv6(tok: &str) -> bool {
    let hex = tok.bytes().filter(u8::is_ascii_hexdigit).count();
    tok.bytes().all(|b| b.is_ascii_hexdigit() || b == b':')
        && ((tok.contains("::") && hex >= 4) || tok.matches(':').count() == 7)
}

/// `AABBCCDDEEFF` or `AABB.CCDD.EEFF`. All digits is more likely a plain number.
fn is_bare_mac(tok: &str) -> bool {
    let hex = |s: &str| s.bytes().all(|b| b.is_ascii_hexdigit());
    let shaped = (tok.len() == 12 && hex(tok))
        || (tok.len() == 14 && tok.split('.').all(|g| g.len() == 4 && hex(g)));
    shaped && tok.bytes().any(|b| b.is_ascii_alphabetic())
}

fn digit_run_is_word(tok: &str, run: &str) -> bool {
    let Some(start) = tok.find(run) else {
        return false;
    };
    let before = tok[..start].chars().last();
    let after = tok[start + run.len()..].chars().next();
    // `_` joins words but not numbers: IMEI_356789012345678 is still an IMEI.
    let joined = |c: char| c.is_ascii_alphanumeric();
    !before.is_some_and(joined) && !after.is_some_and(joined)
}

/// What it takes to confirm which brand a SIM is: its home network, its name, and the network it
/// is registered on. Network codes, not identifiers: the full IMSI stays redacted.
fn sim_identity(reads: &[EndpointRead]) -> Value {
    let body = |path: &str| {
        reads
            .iter()
            .find(|r| r.path.ends_with(path))
            .and_then(|r| r.result.as_ref().ok())
            .map(String::as_str)
            .unwrap_or("")
    };
    let tag = |xml: &str, name: &str| -> String {
        let open = format!("<{name}>");
        xml.split_once(&open)
            .and_then(|(_, rest)| rest.split_once('<'))
            .map(|(v, _)| v.to_string())
            .unwrap_or_default()
    };
    let plmn = body("/net/current-plmn");
    json!({
        "homePlmn": crate::drivers::huawei::xml::home_plmn(&tag(body("/device/information"), "Imsi")),
        "spn": crate::drivers::huawei::xml::clean_spn(&tag(plmn, "Spn")),
        "registeredPlmn": tag(plmn, "Numeric"),
    })
}

/// One read-only endpoint's outcome.
pub struct EndpointRead {
    pub path: String,
    pub result: Result<String, RouterError>,
}

/// Builds the redacted report and runs the leak scan on it. `Err` lists what the scan found;
/// nothing must be written then.
pub fn build_report(
    app_version: &str,
    driver_id: &str,
    driver_name: &str,
    generated_at_ms: u64,
    reads: &[EndpointRead],
    events: &[EventRecord],
) -> Result<String, Vec<String>> {
    let mut r = Redactor::new();
    let endpoints: Vec<Value> = reads
        .iter()
        .map(|e| match &e.result {
            Ok(body) => json!({ "path": e.path, "ok": true, "body": r.body(body, &e.path) }),
            Err(err) => {
                json!({ "path": e.path, "ok": false, "error": r.xml(&err.to_string(), &e.path) })
            }
        })
        .collect();
    let start = events.len().saturating_sub(EVENT_TAIL);
    let events: Vec<Value> = events[start..]
        .iter()
        .enumerate()
        .map(|(i, e)| {
            json!({
                "atMs": e.at_ms,
                "kind": e.kind,
                "detail": r.json(&e.detail, &format!("event{i}")),
            })
        })
        .collect();
    let doc = json!({
        "format": FORMAT,
        "appVersion": app_version,
        "driver": { "id": driver_id, "name": driver_name },
        "generatedAtMs": generated_at_ms,
        "sim": sim_identity(reads),
        "redaction": "Identifiers (IMEI, IMSI, ICCID, serial, MAC and IP addresses, cell, eNB, PCI, TAC, tokens) are replaced by synthetic values of the same shape.",
        "endpoints": endpoints,
        "events": events,
    });
    let text = serde_json::to_string_pretty(&doc).map_err(|e| vec![e.to_string()])?;
    let leaks = r.leaks(&text);
    if leaks.is_empty() {
        Ok(text)
    } else {
        Err(leaks)
    }
}

/// Parses `events.jsonl` content, skipping lines that are not event records.
pub fn parse_events(jsonl: &str) -> Vec<EventRecord> {
    jsonl
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICE: &str = "<response><DeviceName>H122-373</DeviceName><SerialNumber>ABC123DEF4567890</SerialNumber><Imei>356789012345678</Imei><Imsi>420031234567890</Imsi><Iccid>89966031234567890123</Iccid><MacAddress1>A4:B1:C2:D3:E4:F5</MacAddress1><WanIPAddress>10.45.67.89</WanIPAddress><SoftwareVersion>11.0.2.21(H500SP1C00)</SoftwareVersion></response>";
    const SIGNAL: &str = "<response><pci>120</pci><cell_id>123456789</cell_id><enodeb_id>482253</enodeb_id><band>3</band><rsrp>-92dBm</rsrp><nrpci>140</nrpci><nrearfcn>528980</nrearfcn></response>";

    fn read(path: &str, body: &str) -> EndpointRead {
        EndpointRead {
            path: path.into(),
            result: Ok(body.into()),
        }
    }

    #[test]
    fn identifiers_are_replaced_with_same_shape_values() {
        let mut r = Redactor::new();
        let out = r.xml(DEVICE, "info");
        for secret in [
            "ABC123DEF4567890",
            "356789012345678",
            "420031234567890",
            "89966031234567890123",
            "A4:B1:C2:D3:E4:F5",
            "10.45.67.89",
        ] {
            assert!(!out.contains(secret), "{secret} leaked");
        }
        assert!(out.contains("<DeviceName>H122-373</DeviceName>"));
        assert!(out.contains("11.0.2.21"));
        assert!(out.contains("<MacAddress1>02:00:00:00:00:"));
        assert!(out.contains("<WanIPAddress>192.0.2."));
        let imei = out
            .split("<Imei>")
            .nth(1)
            .unwrap()
            .split('<')
            .next()
            .unwrap();
        assert_eq!(imei.len(), 15);
        assert!(imei.bytes().all(|b| b.is_ascii_digit()));
        assert!(r.leaks(&out).is_empty(), "{:?}", r.leaks(&out));
    }

    #[test]
    fn cell_identifiers_are_redacted_but_radio_values_kept() {
        let mut r = Redactor::new();
        let out = r.xml(SIGNAL, "signal");
        assert!(!out.contains("123456789") && !out.contains("482253"));
        assert!(!out.contains("<pci>120</pci>") && !out.contains("<nrpci>140</nrpci>"));
        assert!(out.contains("<band>3</band>") && out.contains("<rsrp>-92dBm</rsrp>"));
        assert!(out.contains("<nrearfcn>528980</nrearfcn>"));
    }

    #[test]
    fn unknown_router_tags_are_caught_by_name_parts() {
        let mut r = Redactor::new();
        let out = r.xml("<x><DeviceImeiSv>3567890123456789</DeviceImeiSv><LanGateway>192.168.1.1</LanGateway><wifi_key>hunter22</wifi_key></x>", "x");
        assert!(
            !out.contains("3567890123456789")
                && !out.contains("192.168.1.1")
                && !out.contains("hunter22")
        );
    }

    #[test]
    fn attributes_cdata_and_children_of_sensitive_elements_are_redacted() {
        let mut r = Redactor::new();
        let out = r.xml(
            concat!(
                r#"<?xml version="1.0"?><x><SerialNumber type="a">SECRET0001</SerialNumber>"#,
                r#"<wifi_key><![CDATA[SECRET0002]]></wifi_key>"#,
                r#"<cell><id>SECRET0003</id><note hint="SECRET0004"/></cell>"#,
                r#"<band serial="SECRET0005">3</band><Username>SECRET0006</Username>"#,
                r#"<!-- SECRET0007 --></x>"#
            ),
            "x",
        );
        assert!(!out.contains("SECRET"), "{out}");
        assert!(out.contains(">3</band>"), "{out}");
        assert!(out.starts_with("<?xml"), "{out}");
    }

    #[test]
    fn json_bodies_are_redacted_and_anything_else_is_left_out() {
        let mut r = Redactor::new();
        let out = r.body(
            r#"{"imei":"SECRET0001","band":3,"list":[{"mac":"x1"}]}"#,
            "x",
        );
        assert_eq!(out["band"], 3);
        assert!(!out.to_string().contains("SECRET0001"));
        assert_ne!(out["list"][0]["mac"], "x1");

        assert_eq!(r.body("plain SECRET0002", "x"), LEFT_OUT);
        assert_eq!(r.body("<a><b>SECRET0003</a>", "x"), LEFT_OUT);
        assert_eq!(r.body("<a>SECRET0004", "x"), LEFT_OUT);
    }

    #[test]
    fn the_scan_catches_the_other_shapes_of_an_identifier() {
        let r = Redactor::new();
        for leak in [
            "35678901234567",
            "356789012345678901",
            "IMEI_356789012345678",
            "35-678901-234567-8",
            "fe80::1a2b:3c4d",
            "2a00:1:2:3:4:5:6:7",
            "AABBCCDDEEFF",
            "AABB.CCDD.EEFF",
            "first line\\n356789012345678",
        ] {
            assert!(!r.leaks(leak).is_empty(), "{leak}");
        }
        // A fake value elsewhere in the text can't shorten a real number into passing.
        let mut r = Redactor::new();
        let fake = r.value("7", "x");
        let real = format!("{fake}{}", "1".repeat(14));
        assert!(!r.leaks(&format!("<a>{fake}</a> {real}")).is_empty());
        assert!(r.leaks(&format!("<a>{fake}</a>")).is_empty());

        for fine in [
            "1793458120235",
            "12:34:56",
            "2026-10-31",
            "<LTEBand>20000000000000</LTEBand>",
            "<SoftwareVersion>11.0.2.21</SoftwareVersion>",
        ] {
            assert!(r.leaks(fine).is_empty(), "{fine}: {:?}", r.leaks(fine));
        }
    }

    /// The router's own responses must still export, with its radio values readable.
    #[test]
    fn the_h122_fixtures_still_export() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/huawei_h122");
        let reads: Vec<EndpointRead> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "xml"))
            .map(|e| {
                read(
                    &e.file_name().to_string_lossy(),
                    &std::fs::read_to_string(e.path()).unwrap(),
                )
            })
            .collect();
        assert!(reads.len() > 3);
        let text = build_report("1.0.0", "huawei_h122", "H122", 7, &reads, &[]).unwrap();
        assert!(!text.contains(LEFT_OUT), "a fixture didn't parse");
    }

    #[test]
    fn event_details_are_redacted_by_key() {
        let mut r = Redactor::new();
        let detail = json!({ "from": { "rat": "lte", "pci": 103, "cellId": 987654321, "band": "B3" }, "to": null });
        let out = r.json(&detail, "e");
        assert_eq!(out["from"]["band"], "B3");
        assert_ne!(out["from"]["cellId"], 987654321);
        assert!(out["from"]["cellId"].is_u64());
        assert!(!out.to_string().contains("987654321"));
    }

    #[test]
    fn scan_blocks_an_identifier_under_an_innocent_name() {
        let reads = [read(
            "/api/x",
            "<response><note>356789012345678</note></response>",
        )];
        let err = build_report("0.1.0", "huawei_h122", "Huawei", 0, &reads, &[]).unwrap_err();
        assert!(err.iter().any(|e| e.contains("15-digit")), "{err:?}");
        let reads = [read(
            "/api/x",
            "<response><router>192.168.8.1</router></response>",
        )];
        assert!(build_report("0.1.0", "h", "H", 0, &reads, &[]).is_err());
        let reads = [read(
            "/api/x",
            "<response><hw>A4:B1:C2:D3:E4:F5</hw></response>",
        )];
        assert!(build_report("0.1.0", "h", "H", 0, &reads, &[]).is_err());
    }

    /// What confirms an MVNO brand: network codes and the SIM's name, never the IMSI itself.
    #[test]
    fn report_names_the_sims_network_and_keeps_the_imsi_out() {
        let imsi = format!("42009{}", "7".repeat(10));
        let info =
            format!("<response><DeviceName>H122-373</DeviceName><Imsi>{imsi}</Imsi></response>");
        let plmn = "<response><FullName>Zain KSA</FullName><Numeric>42004</Numeric><Spn>Salam</Spn></response>";
        let reads = [
            read("/api/device/information", &info),
            read("/api/net/current-plmn", plmn),
        ];
        let text = build_report("0.2.0", "huawei_h122", "Huawei H122-373", 7, &reads, &[]).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["sim"]["homePlmn"], "42009");
        assert_eq!(v["sim"]["spn"], "Salam");
        assert_eq!(v["sim"]["registeredPlmn"], "42004");
        assert!(!text.contains(&imsi));

        // A SIM from elsewhere gives no home network, and nothing breaks without the reads.
        let text = build_report("0.2.0", "mock", "Mock", 7, &[], &[]).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert!(v["sim"]["homePlmn"].is_null());
    }

    #[test]
    fn report_has_endpoints_errors_and_the_event_tail() {
        let events: Vec<EventRecord> = (0..250)
            .map(|i| EventRecord {
                at_ms: i,
                kind: "poll".into(),
                detail: json!({}),
            })
            .collect();
        let reads = [
            read("/api/device/information", DEVICE),
            read("/api/device/signal", SIGNAL),
            EndpointRead {
                path: "/api/net/current-plmn".into(),
                result: Err(RouterError::SessionLost { code: None }),
            },
        ];
        let text = build_report(
            "0.2.0",
            "huawei_h122",
            "Huawei H122-373",
            7,
            &reads,
            &events,
        )
        .unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["format"], FORMAT);
        assert_eq!(v["endpoints"].as_array().unwrap().len(), 3);
        assert_eq!(v["endpoints"][2]["ok"], false);
        assert_eq!(v["events"].as_array().unwrap().len(), EVENT_TAIL);
        assert_eq!(v["events"][0]["atMs"], 50);
    }

    #[test]
    fn the_data_switch_value_is_kept() {
        let mut r = Redactor::new();
        let body = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><dataswitch>1</dataswitch></response>";
        let out = r.body(body, "/api/dialup/mobile-dataswitch");
        assert!(
            out.as_str().unwrap().contains("<dataswitch>1</dataswitch>"),
            "{out}"
        );
        let text = build_report(
            "0.0.0",
            "huawei_h122",
            "Huawei",
            0,
            &[read("/api/dialup/mobile-dataswitch", body)],
            &[],
        )
        .unwrap();
        assert!(text.contains("<dataswitch>1</dataswitch>"), "{text}");
    }

    #[test]
    fn redaction_is_deterministic() {
        let a = Redactor::new().xml(DEVICE, "info");
        let b = Redactor::new().xml(DEVICE, "info");
        assert_eq!(a, b);
    }

    #[test]
    fn identifier_hits_ignore_times_and_measurements() {
        let ok = "2026-11-01T10:00:00Z,2026-11-01 13:00,-95.5,-11.0,116.4,1793491200000,B1+B3,n41";
        assert!(identifier_hits(ok).is_empty(), "{:?}", identifier_hits(ok));
        assert_eq!(identifier_hits("server 10.1.2.3"), vec!["an IPv4 address"]);
        assert_eq!(
            identifier_hits("356789012345678"),
            vec!["a 15-digit number (IMEI or IMSI)"]
        );
        assert_eq!(identifier_hits("aa:bb:cc:dd:ee:ff"), vec!["a MAC address"]);
    }

    #[test]
    fn identifier_tokens_are_replaced_in_place() {
        let text = "var a = \"356789012345678\"; ip='10.1.2.3:80'; m=aa:bb:cc:dd:ee:ff; v=\"11.0\"; t=1793491200000;";
        let (out, found) = redact_identifier_tokens(text);
        assert_eq!(
            out,
            "var a = \"[redacted-number]\"; ip='[redacted-ipv4]'; m=[redacted-mac]; v=\"11.0\"; t=1793491200000;"
        );
        assert_eq!(found.len(), 3);
        assert!(
            identifier_hits(&out).is_empty(),
            "{:?}",
            identifier_hits(&out)
        );
        // The documentation values the scan allows stay.
        let (same, none) = redact_identifier_tokens("x = '192.0.2.1' || '0.0.0.0'");
        assert_eq!(same, "x = '192.0.2.1' || '0.0.0.0'");
        assert!(none.is_empty());
    }
}
