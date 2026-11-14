//! Redacted diagnostics export for driver authors: raw responses of a router's read-only
//! endpoints plus the event-log tail, with every identifier replaced by a synthetic value of the
//! same shape. Ported from `tools/fixtures/redact-har.mjs` (rules and leak scan). Redaction
//! happens here, before anything is written, and a leak-scan hit blocks the export.
//!
//! A body is parsed, not pattern-matched, so attributes, CDATA and everything under a sensitive
//! element are covered. A body that's neither XML nor JSON is left out rather than guessed at.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashSet};

use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
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
pub(crate) const LEFT_OUT: &str = "(left out: not well-formed XML or JSON)";

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

/// Which values a redactor lets through as they are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rules {
    /// The read-only endpoints the app itself asks for: a value stays unless its name is
    /// sensitive.
    #[default]
    Diagnostics,
    /// Whatever a router's own page asked for (`capture.rs`), which can be any page at all: a
    /// value stays only under a name known to be technical, so a device's name, a message or a
    /// place can't come through. Everything else becomes a stand-in of the same shape.
    Capture,
    /// A login exchange: the names stay, and every value is hidden, its length too.
    ShapeOnly,
}

/// What a hidden value becomes when its length must not show either.
pub const HIDDEN: &str = "(hidden)";
/// A kept value longer than this is free text, not a reading or a setting.
const KEPT_MAX: usize = 64;
/// A command's list of the field names it asks for (`cmd=rsrp,sinr,...`) may be this long.
const NAME_LIST_MAX: usize = 4096;
/// A name longer than this is data, not a name.
const NAME_MAX: usize = 48;
/// A value hidden only for its name is remembered, for the check of included code, from this
/// length up.
const SOFT_MIN: usize = 8;
/// A longer value than this is a blob, not a name or a number someone would know again.
const SOFT_MAX: usize = 256;

/// Sensitive in a capture as well: a page can show messages, contacts and Wi-Fi secrets that the
/// app's own reads never touch.
const CAPTURE_SENSITIVE_PARTS: &[&str] = &[
    "sms",
    "content",
    "message",
    "subject",
    "contact",
    "psk",
    "wpa",
    "wep",
    "secret",
    "pass",
    "cred",
    "cookie",
    "csrf",
    "session",
    "location",
    "alias",
    "owner",
    "mail",
    "account",
    "ecgi",
    "scrambling",
    "unlock",
    "verif",
    "security",
    "activat",
    "postal",
];
/// The same, as whole words of a name (`cellId` is `cell` and `id`), where the word alone would
/// match too much as a part. The plurals name lists of people's devices.
const CAPTURE_SENSITIVE_WORDS: &[&str] = &[
    "ci",
    "cid",
    "eci",
    "nci",
    "gci",
    "sid",
    "id",
    "sn",
    "esn",
    "psc",
    "area",
    "qr",
    "access",
    "msg",
    "text",
    "body",
    "pin",
    "puk",
    "pw",
    "psw",
    "url",
    "uri",
    "number",
    "host",
    "hosts",
    "clients",
    "stations",
    "leases",
    "contacts",
    "users",
    "messages",
    "neighbors",
    "neighbours",
    "addr",
    "desc",
    "label",
    "login",
    "auth",
    "zip",
    "otp",
    "nck",
    "uuid",
    "guid",
];
/// Names of readings and of what the router is: their values are kept when they read like one.
const CAPTURE_READING_PARTS: &[&str] = &[
    "rsrp",
    "rsrq",
    "sinr",
    "rssi",
    "rscp",
    "ecio",
    "signal",
    "band",
    "arfcn",
    "freq",
    "version",
    "model",
    "hardware",
    "firmware",
    "software",
    "vendor",
    "manufactur",
    "product",
    "plmn",
    "operator",
    "numeric",
    "nettype",
    "networktype",
    "network_type",
    "net_type",
    "duplex",
    "aggregat",
];
const CAPTURE_READING_WORDS: &[&str] = &[
    "rat", "mcc", "mnc", "snr", "cqi", "csq", "bw", "bar", "bars", "level", "mimo", "rank", "dbm",
];
/// Names of states and of the command a request names. These names are generic, so their values
/// are kept only as one word of a state (`state_word`).
const CAPTURE_STATE_PARTS: &[&str] = &[
    "mode", "state", "status", "enable", "switch", "result", "support", "capab", "roam", "goform",
    "command", "method", "action", "jsonrpc",
];
const CAPTURE_STATE_WORDS: &[&str] = &[
    "type", "cmd", "code", "error", "errno", "errcode", "ret", "retcode", "response", "func",
    "module", "object", "lock",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Keep {
    Reading,
    State,
}

/// Why a value is hidden, which decides whether the leak scan remembers it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hide {
    /// Its own name is sensitive: always remembered.
    Named,
    /// It sits under a sensitive name or in a login: remembered unless it is an ordinary word.
    /// A list of devices says "Wireless", and a login says "session"; remembering those would
    /// stop every save in which the same word is a field's name.
    Inside,
    /// Its name just isn't on the capture's keep list: remembered for the check of included code
    /// only, and only when it isn't an ordinary word.
    Unlisted,
}

/// A word anyone might write: letters, with `_`, `.`, `-` or a space between them. A password,
/// a token, an address or a device with a number in its name is not one.
fn ordinary_word(s: &str) -> bool {
    s.len() <= 24
        && s.chars()
            .all(|c| c.is_ascii_alphabetic() || "_.- ".contains(c))
}

/// One short word in one case, like `static`, `auto` or `enable`: what a setting says. Settings
/// with sensitive names say these all the time, and the same word elsewhere is no leak.
fn enum_word(s: &str) -> bool {
    let mixed =
        s.chars().any(|c| c.is_ascii_uppercase()) && s.chars().any(|c| c.is_ascii_lowercase());
    s.len() <= 10 && s.chars().all(|c| c.is_ascii_alphabetic()) && !mixed
}

/// A name a router uses in its answers (`code`, `result`, `status`). When the page sends
/// something under such a name, a person typed it.
fn answer_word(name: &str) -> bool {
    const ANSWERS: &[&str] = &[
        "code", "result", "status", "state", "error", "errno", "errcode", "ret", "retcode",
        "response",
    ];
    keep_class(name) == Some(Keep::State)
        && words(name).iter().any(|w| ANSWERS.contains(&w.as_str()))
}

/// Where a kept value sits, which decides how it is read.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Spot {
    /// A JSON string, which may hold XML.
    Json,
    /// An element's text: the only place the leak scan lets a version or a band mask through.
    XmlText,
    XmlAttr,
}

/// A name's words, lowercased: split at anything that isn't a letter or digit, where a small
/// letter meets a capital, where an acronym ends (`LTEBand`), and between letters and digits.
fn words(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_ascii_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if let Some(&prev) = i.checked_sub(1).and_then(|p| chars.get(p)) {
            let next_lower = chars.get(i + 1).is_some_and(char::is_ascii_lowercase);
            let boundary = (prev.is_ascii_lowercase() && c.is_ascii_uppercase())
                || (prev.is_ascii_uppercase() && c.is_ascii_uppercase() && next_lower)
                || (prev.is_ascii_digit() != c.is_ascii_digit() && prev.is_ascii_alphanumeric());
            if boundary && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        }
        cur.push(c.to_ascii_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Sensitive under the capture's rules.
pub fn capture_sensitive(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let w = words(name);
    // `lte_band_mask` is a setting, not a subnet mask, and `lte_ca_pcell_band` is a band, not a
    // cell's identity.
    let radio =
        ["band", "freq", "arfcn"].iter().any(|r| n.contains(r)) || w.iter().any(|x| x == "bw");
    let excused = |p: &str| (p == "mask" && n.contains("band")) || (p == "cell" && radio);
    if SENSITIVE_TAGS.contains(&n.as_str())
        || SENSITIVE_PARTS.iter().any(|p| n.contains(p) && !excused(p))
        || CAPTURE_SENSITIVE_PARTS.iter().any(|p| n.contains(p))
    {
        return true;
    }
    // `goformId` names a command. Every other id is an identifier.
    w.iter().any(|x| {
        CAPTURE_SENSITIVE_WORDS.contains(&x.as_str()) && !(x == "id" && n.contains("goform"))
    })
}

fn keep_class(name: &str) -> Option<Keep> {
    if capture_sensitive(name) {
        return None;
    }
    // A modem is a part, not a mode.
    let n = name.to_ascii_lowercase().replace("modem", "");
    let w = words(name);
    let word_in = |list: &[&str]| w.iter().any(|x| list.contains(&x.as_str()));
    if CAPTURE_READING_PARTS.iter().any(|p| n.contains(p)) || word_in(CAPTURE_READING_WORDS) {
        Some(Keep::Reading)
    } else if CAPTURE_STATE_PARTS.iter().any(|p| n.contains(p)) || word_in(CAPTURE_STATE_WORDS) {
        Some(Keep::State)
    } else {
        None
    }
}

/// Whether a capture may keep the value under this name exactly.
pub fn capture_keeps(name: &str) -> bool {
    keep_class(name).is_some()
}

/// A name a program gives a field: letters, digits and `_`, not long, and neither an identifier
/// nor a random-looking token. A short number is a position in a list.
fn plain_name(s: &str) -> bool {
    let digits = s.bytes().filter(u8::is_ascii_digit).count();
    if !s.is_empty() && s.len() <= 4 && digits == s.len() {
        return true;
    }
    s.len() <= NAME_MAX
        && s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !(s.len() >= 16 && digits >= 4)
        // With nothing joining its parts, three digits among letters is a token, not a name.
        && (digits < 3 || s.contains('_'))
        && identifier_hits(s).is_empty()
}

/// A reading or a setting as routers write one: short, plain characters, a few words at most.
fn plain_value(s: &str) -> bool {
    s.len() <= KEPT_MAX
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || " _.,:+/()%-".contains(c))
        && s.split(' ').filter(|w| !w.is_empty()).count() <= 4
}

/// One word of a state: a number, letters in one case, or words joined by `_` or `-`.
fn state_word(s: &str) -> bool {
    let digits = s.bytes().filter(u8::is_ascii_digit).count();
    // A code is short. A long number under a state's name is more likely a session.
    let numeric = s.len() <= 8 && s.chars().all(|c| c.is_ascii_digit() || "+-.".contains(c));
    let wordy = s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    let mixed =
        s.chars().any(|c| c.is_ascii_uppercase()) && s.chars().any(|c| c.is_ascii_lowercase());
    // A token handed back as a "result" has digits all through it.
    let random = s.len() >= 12 && digits >= 4;
    // With nothing joining its parts, three digits among letters is a token (`a1b2c3d4`), and so
    // is a long run of letters with too few vowels to be words.
    let joined = s.contains('_') || s.contains('-');
    let vowels = s.chars().filter(|c| "aeiouAEIOU".contains(*c)).count();
    let letters = s.chars().all(|c| c.is_ascii_alphabetic());
    let token = !joined
        && ((digits >= 3 && digits < s.len())
            || (letters && s.len() >= 12 && vowels * 4 < s.len()));
    !s.is_empty()
        && s.len() <= NAME_MAX
        && (numeric || (wordy && !random && !token && (joined || !mixed)))
}

/// The object or the method of a ubus call, like `zwrt_router.api` or `get`.
fn ubus_name(s: &str) -> bool {
    s.len() <= NAME_MAX
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
        && identifier_hits(s).is_empty()
}

/// OpenWrt's JSON-RPC: `{"jsonrpc":…,"method":"call","params":[session, object, method, {…}]}`.
/// Only this exact shape lets the object and the method through by their position.
fn ubus_call(m: &Map<String, Value>) -> bool {
    m.get("jsonrpc").is_some_and(Value::is_string)
        && m.get("method").and_then(Value::as_str) == Some("call")
        && m.get("params").and_then(Value::as_array).is_some_and(|p| {
            p.len() == 4 && p[..3].iter().all(Value::is_string) && p[3].is_object()
        })
}

/// What a reference in XML text stands for: a character by its number (`&#40;`, `&#x28;`) or
/// one of XML's own five names. Anything else can't be read here.
fn reference_text(r: &quick_xml::events::BytesRef<'_>) -> Option<String> {
    if r.is_char_ref() {
        return r.resolve_char_ref().ok().flatten().map(String::from);
    }
    let name = r.decode().ok()?;
    quick_xml::escape::resolve_xml_entity(&name).map(str::to_string)
}

/// A version's own name, not one that could hold an address (`wanversion`).
fn version_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with("version")
        && !["wan", "lan", "dns", "gateway", "addr", "host", "mac", "ip"]
            .iter()
            .any(|p| n.contains(p))
}

/// Replaces values and remembers what it replaced, for the leak scan.
#[derive(Default)]
pub struct Redactor {
    secrets: BTreeSet<String>,
    /// Values hidden only for their name, under the capture's rules.
    soft: BTreeSet<String>,
    /// Every value that was hidden for any reason, ordinary words too (but not a setting's one
    /// word): a path segment that is one of them is somebody's.
    inside: BTreeSet<String>,
    /// What an earlier pass over the same recording hid (`knowing`).
    known: BTreeSet<String>,
    /// The page is sending what is being cleaned; the router isn't answering it.
    sent: bool,
    /// The names in the answer to the request being cleaned.
    answer_names: BTreeSet<String>,
    generated: BTreeSet<String>,
    counter: u32,
    names: u32,
    rules: Rules,
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

    pub fn with_rules(rules: Rules) -> Self {
        Self {
            rules,
            ..Self::default()
        }
    }

    /// Changes the rules for what comes next. What was replaced so far stays remembered.
    pub fn set_rules(&mut self, rules: Rules) {
        self.rules = rules;
    }

    /// A second pass over a recording, knowing what the first one hid. A value hidden in one
    /// place is then never kept in another, in whatever order the exchanges came.
    pub fn knowing(rules: Rules, earlier: &Redactor) -> Self {
        let known = earlier
            .secrets
            .iter()
            .filter(|s| !earlier.generated.contains(*s))
            .cloned()
            .collect();
        Self {
            rules,
            known,
            inside: earlier.inside.clone(),
            ..Self::default()
        }
    }

    /// Whether what comes next is what the page sends (true) or what the router answers.
    pub fn set_sent(&mut self, sent: bool) {
        self.sent = sent;
    }

    /// The names in the answer to the request that comes next.
    pub fn set_answer_names(&mut self, names: BTreeSet<String>) {
        self.answer_names = names;
    }

    /// Whether everything under this name is hidden, whatever its children are called.
    fn sensitive(&self, name: &str) -> bool {
        match self.rules {
            Rules::Diagnostics => is_sensitive(name),
            Rules::Capture => capture_sensitive(name),
            Rules::ShapeOnly => true,
        }
    }

    /// Whether a value directly under this name is hidden.
    fn hides(&self, name: &str) -> bool {
        match self.rules {
            Rules::Diagnostics => is_sensitive(name),
            Rules::Capture => !capture_keeps(name) || (self.sent && answer_word(name)),
            Rules::ShapeOnly => true,
        }
    }

    /// A synthetic value shaped like `value` (redact-har.mjs `synthLike`). Empty stays empty.
    pub fn value(&mut self, value: &str, label: &str) -> String {
        self.stand_in(value, label, Hide::Named)
    }

    /// Whether a value under this name is remembered for the leak scan whatever it says. The
    /// app's own reads always were; under the stricter rules it takes a sensitive name.
    fn named(&self, name: &str) -> Hide {
        if self.rules == Rules::Diagnostics || capture_sensitive(name) {
            Hide::Named
        } else {
            Hide::Inside
        }
    }

    /// Replaces a value. `hide` says why, which decides what the leak scan remembers of it.
    fn stand_in(&mut self, value: &str, label: &str, hide: Hide) -> String {
        if value.is_empty() {
            return String::new();
        }
        let ordinary = ordinary_word(value);
        match hide {
            // The app's own reads remember everything, as they always did.
            Hide::Named if self.rules == Rules::Diagnostics || !enum_word(value) => {
                self.secrets.insert(value.to_string());
            }
            Hide::Inside if !ordinary => {
                self.secrets.insert(value.to_string());
            }
            Hide::Unlisted if !ordinary && (SOFT_MIN..=SOFT_MAX).contains(&value.len()) => {
                self.soft.insert(value.to_string());
            }
            _ => {}
        }
        let secret = hide != Hide::Unlisted;
        if value.len() <= 64 && !enum_word(value) {
            self.inside.insert(value.to_string());
        }
        if self.rules == Rules::ShapeOnly || (secret && self.rules == Rules::Capture) {
            return HIDDEN.to_string();
        }
        if self.rules != Rules::Diagnostics && value.len() > KEPT_MAX {
            // A megabyte of stand-in tells nobody anything: say how long the value was.
            let start = synth("alnum", 16, label);
            let fake = format!("{start}[{} more]", value.len() - 16);
            self.generated.insert(start);
            self.generated.insert(fake.clone());
            return fake;
        }
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
        // The open elements: name, whether they or a parent are sensitive, and the name written.
        let mut open: Vec<(String, bool, String)> = Vec::new();
        // Stricter rules than the app's own reads: an element's text is judged whole, so here is
        // the text so far, and whether something broke it up. A character reference is read as
        // the character it stands for (a router writes a bracket as `&#40;`), so the whole can
        // still be judged. A comment, CDATA, an instruction or a reference nobody can read breaks
        // it: each piece alone could pass for something it isn't.
        let strict = self.rules != Rules::Diagnostics;
        let mut text: Option<(String, bool)> = None;
        loop {
            let event = reader.read_event().ok()?;
            if strict {
                let part = match &event {
                    Event::Text(t) => Some((String::from_utf8_lossy(t).into_owned(), false)),
                    Event::GeneralRef(r) => Some(match reference_text(r) {
                        Some(c) => (c, false),
                        None => (String::new(), true),
                    }),
                    Event::CData(c) => Some((String::from_utf8_lossy(c).into_owned(), true)),
                    Event::Comment(_) | Event::PI(_) => Some((String::new(), true)),
                    // The declaration and the doctype are free text of their own.
                    Event::Decl(_) | Event::DocType(_) => continue,
                    _ => None,
                };
                if let Some((piece, breaks)) = part {
                    let (so_far, broken) = text.get_or_insert((String::new(), false));
                    so_far.push_str(&piece);
                    *broken |= breaks;
                    continue;
                }
                if let Some((raw, broken)) = text.take() {
                    let written = self.xml_text(&raw, broken, &open, label);
                    // What is written is a stand-in or a plain value. Escaped all the same.
                    let escaped = quick_xml::escape::partial_escape(&written).into_owned();
                    writer
                        .write_event(Event::Text(BytesText::from_escaped(escaped)))
                        .ok()?;
                }
            }
            let secret = open.last().is_some_and(|o| o.1);
            let inside = open.last().map(|o| o.0.clone()).unwrap_or_default();
            let at = format!("{label}:{inside}");
            let event = match event {
                Event::Eof => break,
                Event::Start(e) => {
                    let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                    let hide = secret || self.sensitive(&name);
                    let shown = self.xml_name(&name);
                    let tag = self.xml_tag(&e, &name, &shown, hide, label)?;
                    open.push((name, hide, shown));
                    Event::Start(tag)
                }
                Event::Empty(e) => {
                    let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                    let hide = secret || self.sensitive(&name);
                    let shown = self.xml_name(&name);
                    Event::Empty(self.xml_tag(&e, &name, &shown, hide, label)?)
                }
                Event::End(_) => {
                    let (_, _, shown) = open.pop()?;
                    Event::End(BytesEnd::new(shown))
                }
                // From here on: the app's own reads, as they always were.
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

    /// An element's whole text under the stricter rules, with its character references read.
    /// `broken` means something else broke it up, and then it is hidden whatever its name.
    fn xml_text(
        &mut self,
        raw: &str,
        broken: bool,
        open: &[(String, bool, String)],
        label: &str,
    ) -> String {
        if raw.chars().all(|c| c.is_ascii_whitespace()) {
            return raw.to_string();
        }
        let (inside, secret) = open.last().map(|o| (o.0.clone(), o.1)).unwrap_or_default();
        let at = format!("{label}:{inside}");
        if secret {
            let why = self.named(&inside);
            self.stand_in(raw, &at, why)
        } else if broken || self.hides(&inside) {
            self.stand_in(raw, &at, Hide::Unlisted)
        } else {
            self.kept(raw, &inside, &at, Spot::XmlText)
        }
    }

    /// `name` is the element's own name, which decides what is hidden; `shown` is what is written.
    fn xml_tag(
        &mut self,
        tag: &BytesStart<'_>,
        name: &str,
        shown: &str,
        hide: bool,
        label: &str,
    ) -> Option<BytesStart<'static>> {
        let mut out = BytesStart::new(shown.to_string());
        for attr in tag.attributes() {
            let attr = attr.ok()?;
            let key = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
            let at = format!("{label}:{name}@{key}");
            let value = if hide || self.hides(&key) {
                let raw = String::from_utf8_lossy(&attr.value).into_owned();
                let why = if hide || self.sensitive(&key) {
                    self.named(&key)
                } else {
                    Hide::Unlisted
                };
                Cow::Owned(self.stand_in(&raw, &at, why).into_bytes())
            } else if self.rules != Rules::Diagnostics {
                let raw = String::from_utf8_lossy(&attr.value).into_owned();
                Cow::Owned(self.kept(&raw, &key, &at, Spot::XmlAttr).into_bytes())
            } else {
                attr.value
            };
            let key = self.xml_name(&key);
            out.push_attribute(Attribute {
                key: QName(key.as_bytes()),
                value,
            });
        }
        Some(out)
    }

    /// A name as the file shows it. Under the stricter rules a router's page chooses the names, so
    /// one that doesn't read like a field's name is data: it gets a numbered stand-in.
    fn name(&mut self, name: &str) -> String {
        let hidden = self.secrets.contains(name) || self.known.contains(name);
        if self.rules == Rules::Diagnostics || (plain_name(name) && !hidden) {
            return name.to_string();
        }
        self.names += 1;
        format!("hidden_name_{}", self.names)
    }

    /// The same for an element or an attribute, which may carry one namespace prefix.
    fn xml_name(&mut self, name: &str) -> String {
        let plain = match name.split_once(':') {
            Some((prefix, local)) => plain_name(prefix) && plain_name(local),
            None => plain_name(name),
        };
        let hidden = self.secrets.contains(name) || self.known.contains(name);
        if self.rules == Rules::Diagnostics || (plain && !hidden) {
            return name.to_string();
        }
        self.names += 1;
        format!("hidden_name_{}", self.names)
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
        self.json_at(v, label, "")
    }

    /// `key` is the name the value sits under; a list's items sit under the list's name.
    fn json_at(&mut self, v: &Value, label: &str, key: &str) -> Value {
        match v {
            Value::Object(m) => {
                let ubus = self.rules == Rules::Capture && ubus_call(m);
                let mut out = Map::new();
                for (k, x) in m {
                    let l = format!("{label}.{k}");
                    let red = if ubus && k == "params" {
                        self.ubus_params(x, &l)
                    } else if self.sensitive(k) {
                        self.leaf_json(x, &l, k)
                    } else {
                        self.json_at(x, &l, k)
                    };
                    out.insert(self.name(k), red);
                }
                Value::Object(out)
            }
            Value::Array(a) => Value::Array(
                a.iter()
                    .enumerate()
                    .map(|(i, x)| self.json_at(x, &format!("{label}[{i}]"), key))
                    .collect(),
            ),
            Value::String(s) => Value::String(match self.rules {
                Rules::Diagnostics => self.xml(s, label),
                _ if !self.hides(key) => self.kept(s, key, label, Spot::Json),
                _ => self.stand_in(s, label, Hide::Unlisted),
            }),
            Value::Number(n) if self.rules != Rules::Diagnostics => {
                let text = n.to_string();
                // A state's code is short, as a number too.
                let long_state = keep_class(key) == Some(Keep::State) && text.len() > 8;
                // A number that was hidden somewhere else isn't kept here either.
                let hidden = self.holds_secret(&text);
                if self.hides(key) || long_state || hidden || !identifier_hits(&text).is_empty() {
                    let fake = self.stand_in(&text, label, Hide::Unlisted);
                    fake.parse::<u64>()
                        .map(Value::from)
                        .unwrap_or(Value::String(fake))
                } else {
                    v.clone()
                }
            }
            other => other.clone(),
        }
    }

    /// `[session, object, method, {…}]` of a ubus call (`ubus_call` checked the shape): the
    /// session is a secret, and the object and the method are what a driver needs to read.
    fn ubus_params(&mut self, params: &Value, label: &str) -> Value {
        let Some([session, object, method, args]) = params.as_array().map(Vec::as_slice) else {
            return self.leaf_json(params, label, "params");
        };
        let name = |red: &mut Self, v: &Value, at: &str| match v.as_str() {
            Some(s) if ubus_name(s) => Value::String(s.to_string()),
            Some(s) => Value::String(red.stand_in(s, at, Hide::Unlisted)),
            None => Value::Null,
        };
        Value::Array(vec![
            self.leaf_json(session, &format!("{label}[0]"), "session"),
            name(self, object, &format!("{label}[1]")),
            name(self, method, &format!("{label}[2]")),
            self.json_at(args, &format!("{label}[3]"), ""),
        ])
    }

    /// A value the stricter rules let through by its name. It must still read like a reading or a
    /// state, hold no identifier, and not be something that was hidden elsewhere.
    fn kept(&mut self, text: &str, name: &str, label: &str, spot: Spot) -> String {
        if spot == Spot::Json && text.contains('<') {
            return self.xml(text, label);
        }
        // Before anything that reads the whole of it: a megabyte isn't a reading.
        if text.len() > NAME_LIST_MAX || self.holds_secret(text) {
            return self.stand_in(text, label, Hide::Unlisted);
        }
        // `cmd=rsrp,sinr,lte_band`: the field names a command asks for. Each is kept only when
        // the answer really has a field of that name, so nothing else can ride in the list.
        if text.contains(',') && words(name).iter().any(|w| w == "cmd") {
            let items: Vec<&str> = text
                .split(',')
                .map(|item| {
                    if plain_name(item) && self.answer_names.contains(item) {
                        item
                    } else {
                        "*"
                    }
                })
                .collect();
            return items.join(",");
        }
        let state = keep_class(name) == Some(Keep::State);
        if !text.is_ascii() || !plain_value(text) || (state && !state_word(text)) {
            return self.stand_in(text, label, Hide::Unlisted);
        }
        // The leak scan lets a version and a band mask through by their XML name, unseen. So
        // here a version must be under a version's own name and read like one (it may look like
        // an IPv4 address, nothing else), and a mask must be a short run of hex digits (twelve of
        // them look like a MAC, nothing else).
        let hits = identifier_hits(text);
        let only = |kind: &str| hits.iter().all(|hit| hit.contains(kind));
        let version = version_name(name) && crate::probe::is_plain_label(text) && only("IPv4");
        let mask = text.len() <= 32
            && text
                .chars()
                .all(|c| c.is_ascii_hexdigit() || "x, ".contains(c));
        let band = name.to_ascii_lowercase().ends_with("band") && mask && only("MAC");
        if spot == Spot::XmlText && (version || band) {
            return text.to_string();
        }
        redact_identifier_tokens(text).0
    }

    /// Everything under a sensitive name. `key` is the name a value sits under itself.
    fn leaf_json(&mut self, v: &Value, label: &str, key: &str) -> Value {
        match v {
            Value::Number(n) => {
                let why = self.named(key);
                let fake = self.stand_in(&n.to_string(), label, why);
                fake.parse::<u64>()
                    .map(Value::from)
                    .unwrap_or(Value::String(fake))
            }
            Value::String(s) => {
                let why = self.named(key);
                Value::String(self.stand_in(s, label, why))
            }
            Value::Null | Value::Bool(_) => v.clone(),
            // A nested object under a sensitive key: redact every scalar inside it.
            Value::Object(m) => {
                let mut out = Map::new();
                for (k, x) in m {
                    let red = self.leaf_json(x, &format!("{label}.{k}"), k);
                    out.insert(self.name(k), red);
                }
                Value::Object(out)
            }
            Value::Array(a) => {
                Value::Array(a.iter().map(|x| self.leaf_json(x, label, key)).collect())
            }
        }
    }

    /// Whether `text` holds a value this redactor hid for being sensitive, now or in the pass
    /// before. A value that is one plain word must be there as a whole word, so `enable` isn't
    /// found in `wifi_enable`; anything else is looked for as it is, with and without the
    /// backslashes a file or a script puts before a quote.
    pub fn holds_secret(&self, text: &str) -> bool {
        let all = || {
            self.secrets
                .iter()
                .chain(&self.known)
                .filter(|s| !self.generated.contains(*s))
        };
        let tokens: HashSet<&str> = text.split(|c: char| !word(c)).collect();
        if all().any(|s| s.len() >= 4 && s.chars().all(word) && tokens.contains(s.as_str())) {
            return true;
        }
        let phrases: Vec<&str> = all()
            .filter(|s| s.len() >= 6 && !s.chars().all(word))
            .map(String::as_str)
            .collect();
        held(text, &phrases)
    }

    /// Whether `value` is one of this redactor's own stand-ins, written in place of something.
    pub(crate) fn made(&self, value: &str) -> bool {
        self.generated.contains(value)
    }

    /// Whether this exact word was hidden as a value anywhere, for any reason. For a path's
    /// segments: `/api/hosts/Hibi-Laptop` names the device the list of hosts hid.
    pub fn hidden_word(&self, word: &str) -> bool {
        self.inside.contains(word) || self.secrets.contains(word) || self.known.contains(word)
    }

    /// Whether `text`, as it is and not yet escaped for a file, holds a value this redactor hid,
    /// for whatever reason. For text that can't be parsed by name (a router's script), before it
    /// is let into a file.
    pub fn holds_hidden(&self, text: &str) -> bool {
        let soft: Vec<&str> = self.soft.iter().map(String::as_str).collect();
        self.holds_secret(text) || held(text, &soft)
    }

    /// The leak scan of redact-har.mjs over the finished text. Any hit blocks the export.
    pub fn leaks(&self, text: &str) -> Vec<String> {
        let mut found = Vec::new();
        let loose = text.replace('\\', "");
        // An original that already is a documentation value ("2001:db8::1") maps to itself.
        if self
            .secrets
            .iter()
            .any(|s| s.len() >= 6 && !self.generated.contains(s) && written_in(text, &loose, s))
        {
            found.push("an original identifier is still present".to_string());
        }
        found.extend(self.pattern_leaks(text));
        found
    }

    /// The part of the leak scan that looks for identifiers by their shape.
    pub fn pattern_leaks(&self, text: &str) -> Vec<String> {
        // In the finished JSON a line break is `\n`, which would hide what follows it.
        let plain = text
            .replace("\\n", " ")
            .replace("\\t", " ")
            .replace("\\r", " ");
        pattern_hits(&strip_harmless_fields(&plain), &self.generated)
    }
}

/// Whether any of `needles` is in `text`, in one pass for all of them: a recording can hide
/// hundreds of values, and the text is megabytes.
fn any_in(text: &str, needles: &[&str]) -> bool {
    const AT_ONCE: usize = 400;
    let (short, long): (Vec<&str>, Vec<&str>) = needles
        .iter()
        .copied()
        .filter(|n| !n.is_empty())
        .partition(|n| n.len() <= SOFT_MAX);
    if long.iter().any(|n| text.contains(n)) {
        return true;
    }
    if text.len() < 4096 {
        return short.iter().any(|n| text.contains(n));
    }
    short.chunks(AT_ONCE).any(|chunk| {
        let any = chunk
            .iter()
            .map(|n| regex::escape(n))
            .collect::<Vec<_>>()
            .join("|");
        match regex::RegexBuilder::new(&any).size_limit(64 << 20).build() {
            Ok(re) => re.is_match(text),
            // Never answer "not there" without looking.
            Err(_) => chunk.iter().any(|n| text.contains(n)),
        }
    })
}

/// Whether `text` holds any of `values`, as they are or written with escapes: a finished JSON
/// file and a script both put a backslash before a quote.
fn held(text: &str, values: &[&str]) -> bool {
    if values.is_empty() {
        return false;
    }
    if any_in(text, values) {
        return true;
    }
    if !text.contains('\\') {
        return false;
    }
    let loose = text.replace('\\', "");
    let plain: Vec<String> = values.iter().map(|v| v.replace('\\', "")).collect();
    any_in(
        &loose,
        &plain.iter().map(String::as_str).collect::<Vec<_>>(),
    )
}

/// Whether `text` holds `value`, as it is or written with escapes: a finished JSON file and a
/// script both put a backslash before a quote. `loose` is `text` without its backslashes.
fn written_in(text: &str, loose: &str, value: &str) -> bool {
    text.contains(value) || loose.contains(&value.replace('\\', ""))
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
        // A prefix like `2a00:1:2::` loses its ending below, and with it what makes it an address.
        if ipv6_prefix(tok) {
            hits.insert("an IPv6 address");
        }
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
        let mut hits = pattern_hits(core, &none);
        if hits.is_empty() && ipv6_prefix(tok) {
            hits.push("an IPv6 address".to_string());
        }
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

/// A token as the text has it, ending in `::`: an IPv6 prefix such as `2a00:1:2:3::`.
fn ipv6_prefix(tok: &str) -> bool {
    let t = tok.trim_matches(|c: char| ".-".contains(c));
    t.ends_with("::") && is_ipv6(t) && !t.to_ascii_lowercase().starts_with("2001:db8:")
}

/// `AABBCCDDEEFF`, `AABB.CCDD.EEFF` or `AABB-CCDD-EEFF`. All digits is more likely a plain number.
fn is_bare_mac(tok: &str) -> bool {
    let hex = |s: &str| s.bytes().all(|b| b.is_ascii_hexdigit());
    let shaped = (tok.len() == 12 && hex(tok))
        || (tok.len() == 14 && tok.split(['.', '-']).all(|g| g.len() == 4 && hex(g)));
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
