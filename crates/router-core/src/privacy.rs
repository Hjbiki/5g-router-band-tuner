//! The read-only privacy check: what the provider can reach on the router (TR-069 remote
//! management) and which devices the router remembers.
//!
//! It runs only when the user presses the button, sends GETs only, and changes nothing. What it
//! reads stays in memory for the open window: the event log gets how each read ended, the
//! router's own code and the element names of the answer, and never a value, an address, a name
//! or a count.

use serde::Serialize;
use ts_rs::TS;

use crate::{ApiErrorKind, RouterDriver, RouterError};

/// The refusal from a driver that can't read the remote-management settings.
pub const REMOTE_MANAGEMENT_NOT_SUPPORTED: &str = "reading remote management is not supported";

/// The refusal from a driver that can't read the device list.
pub const DEVICES_NOT_SUPPORTED: &str = "reading the device list is not supported";

/// Devices kept from one answer.
pub const MAX_DEVICES: usize = 256;
/// Characters kept of a device's name.
pub const MAX_NAME_CHARS: usize = 64;
/// Element names kept from one answer, for the event log.
pub const MAX_TAGS: usize = 40;

/// How one read of the check ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PrivacyOutcome {
    Ok,
    /// The router answered with a refusal (its code is in `PrivacyRead::code` when it gave one).
    Refused,
    /// The router says it has no such page (100002).
    NotOffered,
    /// The router answered, but not with something the app can read.
    Unreadable,
    /// No answer in time, or the router couldn't be reached.
    NoAnswer,
    /// The app's driver for this router doesn't have this read.
    NotSupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PrivacyRead {
    pub outcome: PrivacyOutcome,
    pub code: Option<u32>,
}

impl PrivacyRead {
    pub fn ok() -> Self {
        Self {
            outcome: PrivacyOutcome::Ok,
            code: None,
        }
    }

    /// A session-type refusal counts as refused here, not as a lost session: the router answers
    /// that way for a page it won't serve to this login, and the signal poll would notice a
    /// real loss within seconds.
    pub fn from_error(e: &RouterError) -> Self {
        let outcome = match e {
            RouterError::Api {
                kind: ApiErrorKind::NotSupported,
                ..
            } => PrivacyOutcome::NotOffered,
            RouterError::Validation { .. } | RouterError::Unsupported => {
                PrivacyOutcome::NotSupported
            }
            RouterError::Parse { .. } => PrivacyOutcome::Unreadable,
            RouterError::Timeout | RouterError::Unreachable => PrivacyOutcome::NoAnswer,
            _ => PrivacyOutcome::Refused,
        };
        Self {
            outcome,
            code: e.router_code(),
        }
    }
}

/// What the router says about the provider's remote management (TR-069). Only what the answer
/// shows for certain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteManagement {
    /// The management server's host name, without account, port or path.
    pub server_host: Option<String>,
    /// Whether a server address is set: `None` when the answer has no such field.
    pub server_set: Option<bool>,
    /// Whether remote management is switched on, from the field named exactly `enable` (seen on
    /// the H122 on 2026-11-13). `None` when the answer has no such field or an unknown value:
    /// never guessed from another name.
    pub enabled: Option<bool>,
}

/// How a device reaches the router.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DeviceLink {
    Wifi,
    Cable,
}

/// One device the router lists. For the open window only: never logged, stored or exported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LanDevice {
    /// The device's own name, cleaned (`clean_name`). Anyone on the network chooses theirs.
    pub name: Option<String>,
    /// `AA:BB:CC:DD:EE:FF`.
    pub mac: String,
    pub ip: Option<String>,
    /// Online now; `None` when the router doesn't say.
    pub active: Option<bool>,
    /// A private (randomised) Wi-Fi address, which doesn't identify the device elsewhere.
    pub private_mac: bool,
    /// Wi-Fi or cable; `None` when the router doesn't say.
    pub link: Option<DeviceLink>,
    /// The router marks this one as the device that asked: this PC.
    pub this_device: bool,
}

/// Which of the router's lists the devices came from, since they don't mean the same thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DeviceSource {
    /// Every device the router remembers, online or not.
    Lan,
    /// Only the devices on Wi-Fi right now.
    Wifi,
}

/// The whole check, for the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PrivacyReport {
    pub remote_management: PrivacyRead,
    pub management: Option<RemoteManagement>,
    pub devices_read: PrivacyRead,
    /// The list that answered; `None` when neither did.
    pub devices_source: Option<DeviceSource>,
    pub devices: Vec<LanDevice>,
}

/// A parsed answer with the element names it held (`MAX_TAGS` at most, each once, in order).
/// The names are the firmware's vocabulary, not the user's data: they go to the event log so an
/// answer the app couldn't use can be understood later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen<T> {
    pub value: T,
    pub tags: Vec<String>,
}

/// The host of an `http://` or `https://` address, lower-cased: no account, port, path or query.
/// Anything else, an IPv6 literal included, is `None`. So is an address with an `@` after the
/// host part: an account written with a `/` in it would otherwise be cut short and read as the
/// host.
pub fn server_host(url: &str) -> Option<String> {
    let lower = url.trim().to_ascii_lowercase();
    let rest = lower
        .strip_prefix("http://")
        .or_else(|| lower.strip_prefix("https://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if rest[authority.len()..].contains('@') {
        return None;
    }
    let host = authority.rsplit('@').next()?.split(':').next()?;
    let plain = host
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    (!host.is_empty() && host.len() <= 253 && plain).then(|| host.to_string())
}

/// `AA:BB:CC:DD:EE:FF` from six hex pairs written with `:`, `-` or nothing between them.
pub fn normalize_mac(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if !raw.is_ascii() {
        return None;
    }
    let b = raw.as_bytes();
    let step = match b.len() {
        12 => 2,
        17 if matches!(b[2], b':' | b'-') && (2..17).step_by(3).all(|i| b[i] == b[2]) => 3,
        _ => return None,
    };
    let pairs: Vec<&str> = (0..6).map(|i| &raw[i * step..i * step + 2]).collect();
    pairs
        .iter()
        .all(|p| p.bytes().all(|c| c.is_ascii_hexdigit()))
        .then(|| pairs.join(":").to_ascii_uppercase())
}

/// Whether a normalised MAC address is locally administered: a private (randomised) address.
pub fn is_private_mac(mac: &str) -> bool {
    mac.get(0..2)
        .and_then(|first| u8::from_str_radix(first, 16).ok())
        .is_some_and(|first| first & 0b10 != 0 && first & 0b01 == 0)
}

/// Marks that change the direction text is drawn in. A name carrying one could scramble the
/// line it is shown on.
fn is_direction_mark(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
    )
}

/// Characters that draw nothing by themselves: spaces, joiners, the soft hyphen, the word
/// joiner, variation selectors and tag characters.
fn is_invisible(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '\u{00AD}'
                | '\u{034F}'
                | '\u{180E}'
                | '\u{200B}'..='\u{200D}'
                | '\u{2060}'..='\u{2064}'
                | '\u{FE00}'..='\u{FE0F}'
                | '\u{E0000}'..='\u{E007F}'
                | '\u{E0100}'..='\u{E01EF}'
        )
}

/// A device name as it may be shown: control and text-direction characters removed, trimmed and
/// cut to `MAX_NAME_CHARS`. Nothing left, or nothing left that can be seen, is `None`. Joiners
/// inside a name stay, because Arabic-script names and emoji are written with them.
pub fn clean_name(raw: &str) -> Option<String> {
    let kept: String = raw
        .chars()
        .filter(|c| !c.is_control() && !is_direction_mark(*c))
        .collect();
    let cut: String = kept.trim().chars().take(MAX_NAME_CHARS).collect();
    let cut = cut.trim_end();
    cut.chars()
        .any(|c| !is_invisible(c))
        .then(|| cut.to_string())
}

/// One read's line in the event log: how it ended, the router's code and the element names.
fn read_detail(read: &PrivacyRead, tags: &[String]) -> serde_json::Value {
    let mut line = serde_json::Map::new();
    line.insert(
        "result".to_string(),
        serde_json::to_value(read.outcome).unwrap_or(serde_json::Value::Null),
    );
    if let Some(code) = read.code {
        line.insert("code".to_string(), code.into());
    }
    if !tags.is_empty() {
        line.insert("tags".to_string(), tags.into());
    }
    serde_json::Value::Object(line)
}

/// Runs the check on `driver`: remote management, then the device list, each ending in its own
/// outcome. The Wi-Fi list is asked for only when the router answers the LAN list with a refusal
/// or something unreadable; a router that doesn't answer isn't asked twice. Three reads at most.
///
/// Returns the report for the UI and the event-log detail: per read its outcome, code and
/// element names, which list answered (`source`), and what the router did with the other one
/// (`lan` when the Wi-Fi list was used instead, `wifi` when both failed).
pub async fn check(driver: &dyn RouterDriver) -> (PrivacyReport, serde_json::Value) {
    let (remote_management, management, management_tags) = match driver.remote_management().await {
        Ok(seen) => (PrivacyRead::ok(), Some(seen.value), seen.tags),
        Err(e) => (PrivacyRead::from_error(&e), None, Vec::new()),
    };

    let mut devices: Vec<LanDevice> = Vec::new();
    let mut devices_source = None;
    let mut device_tags: Vec<String> = Vec::new();
    // How the list that wasn't used ended, with its name in the log.
    let mut other: Option<(&str, PrivacyRead)> = None;
    let devices_read = match driver.lan_devices().await {
        Ok(seen) => {
            (devices, device_tags, devices_source) =
                (seen.value, seen.tags, Some(DeviceSource::Lan));
            PrivacyRead::ok()
        }
        Err(e) => {
            let lan = PrivacyRead::from_error(&e);
            if lan.outcome == PrivacyOutcome::NoAnswer {
                lan
            } else {
                match driver.wifi_devices().await {
                    Ok(seen) => {
                        (devices, device_tags, devices_source) =
                            (seen.value, seen.tags, Some(DeviceSource::Wifi));
                        other = Some(("lan", lan));
                        PrivacyRead::ok()
                    }
                    Err(e) => {
                        other = Some(("wifi", PrivacyRead::from_error(&e)));
                        lan
                    }
                }
            }
        }
    };

    let mut devices_line = read_detail(&devices_read, &device_tags);
    if let Some(line) = devices_line.as_object_mut() {
        if let Some(source) = devices_source {
            line.insert(
                "source".to_string(),
                serde_json::to_value(source).unwrap_or(serde_json::Value::Null),
            );
        }
        if let Some((name, read)) = &other {
            line.insert(name.to_string(), read_detail(read, &[]));
        }
    }
    let detail = serde_json::json!({
        "remoteManagement": read_detail(&remote_management, &management_tags),
        "devices": devices_line,
    });
    let report = PrivacyReport {
        remote_management,
        management,
        devices_read,
        devices_source,
        devices,
    };
    (report, detail)
}
