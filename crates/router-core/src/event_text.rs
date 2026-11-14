//! Plain-words view of `events.jsonl` records for the History page and the CSV export.
//!
//! [`describe`] turns one [`EventRecord`] into an [`EventItem`]: an i18n key
//! (`events.kind.<camelCase kind>`), typed parameters, a category, a severity and whether the
//! event is technical (hidden unless the user asks for technical events). [`group_changes`]
//! folds the records of one band change, from its snapshot read to its end, into a
//! [`ChangeGroup`].
//!
//! Nothing here names a cell: a cell change reads "moved to another B3 cell", never a PCI.
//! A detail with an unexpected shape gives `events.kind.unknown`; nothing panics.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::bands::lte::LteBandSet;
use crate::event_log::EventRecord;

/// Every event kind the app writes. Write sites use these, so the describer can't miss one.
pub mod kinds {
    pub const SAFE_APPLY_STARTED: &str = "safe_apply.started";
    pub const SAFE_APPLY_APPLIED: &str = "safe_apply.applied";
    pub const SAFE_APPLY_VERIFIED: &str = "safe_apply.verified";
    pub const SAFE_APPLY_REATTACHED: &str = "safe_apply.reattached";
    pub const SAFE_APPLY_COMMITTED: &str = "safe_apply.committed";
    pub const SAFE_APPLY_REVERTED: &str = "safe_apply.reverted";
    pub const SAFE_APPLY_FELL_BACK_TO_AUTO: &str = "safe_apply.fell_back_to_auto";
    pub const SAFE_APPLY_REVERT_FAILED: &str = "safe_apply.revert_failed";
    pub const SAFE_APPLY_REJECTED: &str = "safe_apply.rejected";
    pub const SAFE_APPLY_RESUMED: &str = "safe_apply.resumed";
    pub const SAFE_APPLY_LOSS_IGNORED: &str = "safe_apply.loss_ignored";
    pub const SAFE_APPLY_READ: &str = "safe_apply.read";
    pub const SAFE_APPLY_POST: &str = "safe_apply.post";

    pub const JOURNAL_FOREIGN: &str = "journal.foreign";
    pub const JOURNAL_UNREADABLE: &str = "journal.unreadable";
    pub const JOURNAL_WRITE_FAILED: &str = "journal.write_failed";
    pub const JOURNAL_ARCHIVE_FAILED: &str = "journal.archive_failed";

    pub const SEQUENCE_STARTED: &str = "sequence.started";
    pub const SEQUENCE_STEP2: &str = "sequence.step2";
    pub const SEQUENCE_ENDED: &str = "sequence.ended";
    pub const SEQUENCE_RESUMED: &str = "sequence.resumed";
    pub const SEQUENCE_DISCARDED: &str = "sequence.discarded";
    pub const SEQUENCE_CANCELLED: &str = "sequence.cancelled";
    pub const SEQUENCE_WRITE_FAILED: &str = "sequence.write_failed";
    pub const SEQUENCE_CLEAR_FAILED: &str = "sequence.clear_failed";

    pub const RELOCK_STARTED: &str = "relock.started";
    pub const RELOCK_OUTCOME: &str = "relock.outcome";
    pub const RELOCK_GAVE_UP: &str = "relock.gave_up";
    pub const RELOCK_RESET: &str = "relock.reset";

    pub const AUTOMATION_SETTINGS: &str = "automation.settings";
    pub const SPEEDTEST_ACTIVE: &str = "speedtest.active";
    pub const SPEEDTEST_FAILED: &str = "speedtest.failed";
    pub const SPEEDTEST_AUTO_SKIPPED: &str = "speedtest.auto_skipped";
    pub const SCAN_ACTIVE: &str = "scan.active";

    pub const DEVICE_REBOOT: &str = "device.reboot";
    pub const DEVICE_FACTORY_RESET: &str = "device.factory_reset";

    pub const SESSION: &str = "session";
    pub const SESSION_LOST: &str = "session.lost";
    pub const SESSION_REFRESHED: &str = "session.refreshed";
    pub const SESSION_IDENTITY_NOT_SAVED: &str = "session.identity_not_saved";
    pub const SESSION_MARKER_NOT_SAVED: &str = "session.marker_not_saved";
    pub const SESSION_LOGOUT: &str = "session.logout";
    pub const SESSION_LOGOUT_MARKER_NOT_SAVED: &str = "session.logout_marker_not_saved";
    pub const SESSION_AUTO_CONNECT_SKIPPED: &str = "session.auto_connect_skipped";
    pub const SESSION_LOGOUT_OVERRIDDEN: &str = "session.logout_overridden";
    pub const HEARTBEAT_FAILED: &str = "heartbeat.failed";
    pub const CELL_CHANGED: &str = "cell.changed";
    pub const WAN_RANGE_CHANGED: &str = "wan.range_changed";
    /// The router refused an optional read (APN list, data switch, internet address); that
    /// feature is off until the next Connect.
    pub const FEATURE_REFUSED: &str = "feature.refused";

    pub const IP_REFRESH_STARTED: &str = "ip_refresh.started";
    pub const IP_REFRESH_TRY: &str = "ip_refresh.try";
    pub const IP_REFRESH_ENDED: &str = "ip_refresh.ended";
    pub const IP_REFRESH_DATA_ON_FAILED: &str = "ip_refresh.data_on_failed";
    pub const IP_REFRESH_RESUMED: &str = "ip_refresh.resumed";
    pub const IP_REFRESH_MANUAL_ON: &str = "ip_refresh.manual_on";
    /// A network search ended: how long it took and how many networks it listed (a count only).
    pub const IP_REFRESH_SEARCH: &str = "ip_refresh.search";
    /// The router refused "off", so the try went on as a network search.
    pub const IP_REFRESH_METHOD_FALLBACK: &str = "ip_refresh.method_fallback";
    /// The band config after a run that searched differs from the one before it.
    pub const IP_REFRESH_BANDS_CHANGED: &str = "ip_refresh.bands_changed";
    /// A band switch for a new IP: started (ranges and configs), step 1 kept early, ended.
    pub const IP_REATTACH_STARTED: &str = "ip_reattach.started";
    pub const IP_REATTACH_STEP1_KEPT: &str = "ip_reattach.step1_kept";
    pub const IP_REATTACH_ENDED: &str = "ip_reattach.ended";

    /// APN switches: index, phase and reason only, never a name or an APN.
    pub const APN_STARTED: &str = "apn.started";
    pub const APN_APPLIED: &str = "apn.applied";
    pub const APN_VERIFIED: &str = "apn.verified";
    pub const APN_REATTACHED: &str = "apn.reattached";
    pub const APN_COMMITTED: &str = "apn.committed";
    pub const APN_REVERTED: &str = "apn.reverted";
    pub const APN_REVERT_FAILED: &str = "apn.revert_failed";
    pub const APN_REJECTED: &str = "apn.rejected";
    pub const APN_RESUMED: &str = "apn.resumed";
    /// APN profile writes: the index only.
    pub const APN_PROFILE_ADDED: &str = "apn.profile_added";
    pub const APN_PROFILE_EDITED: &str = "apn.profile_edited";
    pub const APN_PROFILE_DELETED: &str = "apn.profile_deleted";
    /// After a write the router dialled with another profile (`index` is the one from before,
    /// `now` the new one, `reverting` whether a switch back started).
    pub const APN_UNEXPECTED_SWITCH: &str = "apn.unexpected_switch";

    pub const NOTIFY: &str = "notify";
    pub const ALERTS_LAGGED: &str = "alerts.lagged";
    pub const APP_STARTED: &str = "app.started";
    pub const APP_STOPPED: &str = "app.stopped";

    pub const HISTORY_PRUNED: &str = "history.pruned";
    pub const HISTORY_CLEARED: &str = "history.cleared";
    pub const HISTORY_RESET: &str = "history.reset";
    pub const HISTORY_IMPORTED: &str = "history.imported";
    pub const HISTORY_DROPPED: &str = "history.dropped";
    pub const HISTORY_EXPORTED: &str = "history.exported";
    /// "Save router page scripts": counts only (`files`, `bytes`, `skipped`), no paths.
    pub const DIAGNOSTICS_PAGE_SCRIPTS_SAVED: &str = "diagnostics.page_scripts_saved";
    /// The privacy check ran: per read, how it ended (`result`), the router's `code` and the
    /// answer's element names (`tags`). Never a value, an address, a name or a count.
    pub const PRIVACY_CHECKED: &str = "privacy.checked";

    pub const ALL: &[&str] = &[
        SAFE_APPLY_STARTED,
        SAFE_APPLY_APPLIED,
        SAFE_APPLY_VERIFIED,
        SAFE_APPLY_REATTACHED,
        SAFE_APPLY_COMMITTED,
        SAFE_APPLY_REVERTED,
        SAFE_APPLY_FELL_BACK_TO_AUTO,
        SAFE_APPLY_REVERT_FAILED,
        SAFE_APPLY_REJECTED,
        SAFE_APPLY_RESUMED,
        SAFE_APPLY_LOSS_IGNORED,
        SAFE_APPLY_READ,
        SAFE_APPLY_POST,
        JOURNAL_FOREIGN,
        JOURNAL_UNREADABLE,
        JOURNAL_WRITE_FAILED,
        JOURNAL_ARCHIVE_FAILED,
        SEQUENCE_STARTED,
        SEQUENCE_STEP2,
        SEQUENCE_ENDED,
        SEQUENCE_RESUMED,
        SEQUENCE_DISCARDED,
        SEQUENCE_CANCELLED,
        SEQUENCE_WRITE_FAILED,
        SEQUENCE_CLEAR_FAILED,
        RELOCK_STARTED,
        RELOCK_OUTCOME,
        RELOCK_GAVE_UP,
        RELOCK_RESET,
        AUTOMATION_SETTINGS,
        SPEEDTEST_ACTIVE,
        SPEEDTEST_FAILED,
        SPEEDTEST_AUTO_SKIPPED,
        SCAN_ACTIVE,
        DEVICE_REBOOT,
        DEVICE_FACTORY_RESET,
        SESSION,
        SESSION_LOST,
        SESSION_REFRESHED,
        SESSION_IDENTITY_NOT_SAVED,
        SESSION_MARKER_NOT_SAVED,
        SESSION_LOGOUT,
        SESSION_LOGOUT_MARKER_NOT_SAVED,
        SESSION_AUTO_CONNECT_SKIPPED,
        SESSION_LOGOUT_OVERRIDDEN,
        HEARTBEAT_FAILED,
        CELL_CHANGED,
        WAN_RANGE_CHANGED,
        FEATURE_REFUSED,
        IP_REFRESH_STARTED,
        IP_REFRESH_TRY,
        IP_REFRESH_ENDED,
        IP_REFRESH_DATA_ON_FAILED,
        IP_REFRESH_RESUMED,
        IP_REFRESH_MANUAL_ON,
        IP_REFRESH_SEARCH,
        IP_REFRESH_METHOD_FALLBACK,
        IP_REFRESH_BANDS_CHANGED,
        IP_REATTACH_STARTED,
        IP_REATTACH_STEP1_KEPT,
        IP_REATTACH_ENDED,
        APN_STARTED,
        APN_APPLIED,
        APN_VERIFIED,
        APN_REATTACHED,
        APN_COMMITTED,
        APN_REVERTED,
        APN_REVERT_FAILED,
        APN_REJECTED,
        APN_RESUMED,
        APN_PROFILE_ADDED,
        APN_PROFILE_EDITED,
        APN_PROFILE_DELETED,
        APN_UNEXPECTED_SWITCH,
        NOTIFY,
        ALERTS_LAGGED,
        APP_STARTED,
        APP_STOPPED,
        HISTORY_PRUNED,
        HISTORY_CLEARED,
        HISTORY_RESET,
        HISTORY_IMPORTED,
        HISTORY_DROPPED,
        HISTORY_EXPORTED,
        DIAGNOSTICS_PAGE_SCRIPTS_SAVED,
        PRIVACY_CHECKED,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum EventCategory {
    BandChange,
    Connection,
    SpeedTest,
    Device,
    App,
}

/// Shown as a text pill, never by colour alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Severity {
    Info,
    Good,
    Warning,
    Problem,
}

/// One parameter of an event's text. `Key` is an i18n key to translate and insert.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "t", content = "v", rename_all = "camelCase")]
#[ts(export)]
pub enum EventParam {
    Text(String),
    Number(f64),
    Key(String),
    /// LTE band numbers, rendered "B1+B3".
    Bands(Vec<u16>),
    /// NR band numbers, rendered "n41+n78".
    NrBands(Vec<u16>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EventItem {
    #[ts(type = "number")]
    pub at_ms: u64,
    /// The raw kind, such as `safe_apply.reverted`.
    pub kind: String,
    pub category: EventCategory,
    pub severity: Severity,
    /// Internal steps and bookkeeping, hidden by default.
    pub technical: bool,
    /// `events.kind.<camelCase kind>`, or a more specific key for some kinds.
    pub key: String,
    pub params: BTreeMap<String, EventParam>,
    /// A second line that explains the event (`events.why.*`), when there is one.
    pub detail_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum ChangeOutcome {
    /// Still running, or the log ends before it did.
    Open,
    Kept,
    /// `reason` is the i18n key `safeApply.reason.<x>`. `fallback`: it ended on Auto.
    Reverted {
        reason: String,
        fallback: bool,
    },
    RevertFailed,
    Rejected,
}

/// One band change, from its snapshot read to its end.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChangeGroup {
    #[ts(type = "number")]
    pub started_ms: u64,
    #[ts(type = "number | null")]
    pub ended_ms: Option<u64>,
    /// The bands before the change; empty when the log doesn't say.
    pub from: Vec<u16>,
    /// The bands asked for; empty when the log doesn't say.
    pub to: Vec<u16>,
    /// The primary band of an enclosing two-step sequence.
    pub primary: Option<u16>,
    pub outcome: ChangeOutcome,
    pub steps: Vec<EventItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum EventEntry {
    Single { item: EventItem },
    Change { group: ChangeGroup },
}

/// The i18n key for a revert reason, shared with the Bands-page banner.
pub fn revert_reason_key(reason: &str) -> Option<&'static str> {
    Some(match reason {
        "applyRejected" => "safeApply.reason.rejected",
        "verifyMismatch" => "safeApply.reason.verifyMismatch",
        "noReattach" => "safeApply.reason.noReattach",
        "lossOfService" => "safeApply.reason.connectionLost",
        "userRequested" => "safeApply.reason.user",
        "resumedWithoutService" => "safeApply.reason.resumedWithoutService",
        _ => return None,
    })
}

/// "safe_apply.fell_back_to_auto" -> "safeApplyFellBackToAuto".
fn camel(kind: &str) -> String {
    let mut out = String::with_capacity(kind.len());
    let mut upper = false;
    for c in kind.chars() {
        if c == '.' || c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

const MAX_TEXT: usize = 200;

fn text(s: &str) -> EventParam {
    EventParam::Text(s.chars().take(MAX_TEXT).collect())
}

fn bands_of(config: &Value) -> Option<Vec<u16>> {
    let set: LteBandSet = serde_json::from_value(config.get("lteBands")?.clone()).ok()?;
    Some(set.bands().into_iter().map(u16::from).collect())
}

fn phase_kind(d: &Value) -> Option<&str> {
    d.get("phase")?.get("kind")?.as_str()
}

/// Builder for one item; `None` from a mapping means the detail had an unexpected shape.
struct Item {
    category: EventCategory,
    severity: Severity,
    technical: bool,
    key: Option<String>,
    params: BTreeMap<String, EventParam>,
    detail_key: Option<String>,
}

impl Item {
    fn new(category: EventCategory, severity: Severity) -> Self {
        Self {
            category,
            severity,
            technical: false,
            key: None,
            params: BTreeMap::new(),
            detail_key: None,
        }
    }
    fn technical(mut self) -> Self {
        self.technical = true;
        self
    }
    fn key(mut self, k: String) -> Self {
        self.key = Some(k);
        self
    }
    fn why(mut self, k: impl Into<String>) -> Self {
        self.detail_key = Some(k.into());
        self
    }
    fn param(mut self, name: &str, p: EventParam) -> Self {
        self.params.insert(name.to_string(), p);
        self
    }
    fn num(self, name: &str, v: Option<f64>) -> Self {
        match v {
            Some(v) => self.param(name, EventParam::Number(v)),
            None => self,
        }
    }
    /// With the router's own error code in the detail, the text that names it (`key`, with
    /// `{code}`). Without one, the item stays as it is.
    fn router_code(self, key: &str, d: &Value) -> Self {
        match router_code(d) {
            Some(code) => self.key(key.into()).num("code", Some(code)),
            None => self,
        }
    }
}

/// The router's code in the detail's error (`error`, or `refusal` for a Refresh IP run), as a
/// serialized `RouterError` carries it: `api`, `loginRejected` and `sessionLost` have one.
fn router_code(d: &Value) -> Option<f64> {
    ["error", "refusal"]
        .iter()
        .find_map(|k| d.get(*k)?.get("code")?.as_f64())
}

fn on_off(v: Option<bool>) -> EventParam {
    EventParam::Key(
        if v == Some(true) {
            "events.value.on"
        } else {
            "events.value.off"
        }
        .into(),
    )
}

/// The plain-words view of one record.
pub fn describe(e: &EventRecord) -> EventItem {
    let built = map(e.kind.as_str(), &e.detail);
    match built {
        Some(item) => EventItem {
            at_ms: e.at_ms,
            kind: e.kind.clone(),
            category: item.category,
            severity: item.severity,
            technical: item.technical,
            key: item
                .key
                .unwrap_or_else(|| format!("events.kind.{}", camel(&e.kind))),
            params: item.params,
            detail_key: item.detail_key,
        },
        None => unknown(e),
    }
}

fn unknown(e: &EventRecord) -> EventItem {
    let mut params = BTreeMap::new();
    params.insert("code".to_string(), text(&e.kind));
    EventItem {
        at_ms: e.at_ms,
        kind: e.kind.clone(),
        category: EventCategory::App,
        severity: Severity::Info,
        technical: true,
        key: "events.kind.unknown".into(),
        params,
        detail_key: None,
    }
}

fn map(kind: &str, d: &Value) -> Option<Item> {
    use kinds::*;
    use EventCategory::*;
    use Severity::*;
    let s = |k: &str| d.get(k).and_then(Value::as_str);
    let n = |k: &str| d.get(k).and_then(Value::as_f64);
    let b = |k: &str| d.get(k).and_then(Value::as_bool);
    let item = match kind {
        SAFE_APPLY_STARTED => Item::new(BandChange, Info),
        SAFE_APPLY_APPLIED | SAFE_APPLY_VERIFIED => Item::new(BandChange, Info).technical(),
        SAFE_APPLY_REATTACHED => Item::new(BandChange, Info),
        SAFE_APPLY_COMMITTED => Item::new(BandChange, Good),
        SAFE_APPLY_REVERTED => {
            let reason = revert_reason_key(s("reason")?)?;
            let item =
                Item::new(BandChange, Warning).param("reason", EventParam::Key(reason.into()));
            if b("fallback")? {
                item.why("events.why.fellBackToAuto")
            } else {
                item
            }
        }
        SAFE_APPLY_FELL_BACK_TO_AUTO => Item::new(BandChange, Warning),
        SAFE_APPLY_REVERT_FAILED => Item::new(BandChange, Problem).why("events.why.revertFailed"),
        SAFE_APPLY_REJECTED => Item::new(BandChange, Warning).param("message", text(s("message")?)),
        SAFE_APPLY_RESUMED => {
            Item::new(BandChange, Info).param("phase", text(phase_kind(d).unwrap_or("")))
        }
        SAFE_APPLY_LOSS_IGNORED => {
            let reason = revert_reason_key(s("reason")?)?;
            Item::new(BandChange, Warning)
                .param("reason", EventParam::Key(reason.into()))
                .why("events.why.lossIgnored")
        }
        SAFE_APPLY_READ | SAFE_APPLY_POST => {
            let phase = phase_kind(d)?;
            let failed = d.get("error").is_some();
            let mut item = Item::new(BandChange, if failed { Warning } else { Info })
                .technical()
                .param("phase", text(phase));
            if let Some(bands) = d.get("config").and_then(bands_of) {
                item = item.param("bands", EventParam::Bands(bands));
            }
            if failed {
                item = item.key(format!("events.kind.{}Failed", camel(kind)));
            }
            item
        }
        JOURNAL_FOREIGN if s("file") == Some("data_switch") => Item::new(Connection, Warning)
            .key("events.kind.dataJournalForeign".into())
            .why("events.why.dataJournalBlocked"),
        JOURNAL_UNREADABLE if s("file") == Some("data_switch") => Item::new(Connection, Problem)
            .key("events.kind.dataJournalUnreadable".into())
            .why("events.why.dataJournalBlocked"),
        JOURNAL_FOREIGN => Item::new(BandChange, Warning).why("events.why.foreignJournal"),
        JOURNAL_UNREADABLE => Item::new(BandChange, Problem).why("events.why.journalUnreadable"),
        JOURNAL_WRITE_FAILED => Item::new(BandChange, Problem).technical(),
        JOURNAL_ARCHIVE_FAILED => Item::new(BandChange, Warning).technical(),
        SEQUENCE_STARTED => {
            let mut item = Item::new(BandChange, Info).num("primary", n("primary"));
            if let Some(bands) = d.get("final").and_then(bands_of) {
                item = item.param("bands", EventParam::Bands(bands));
            }
            // A band switch for a new IP has no primary band: `ip_reattach.started` says it.
            if s("origin") == Some("ipReattach") {
                item = item.technical();
            }
            item
        }
        SEQUENCE_STEP2 => {
            let mut item = Item::new(BandChange, Info);
            if let Some(bands) = d.get("final").and_then(bands_of) {
                item = item.param("bands", EventParam::Bands(bands));
            }
            item
        }
        SEQUENCE_ENDED => {
            let ok = b("ok").unwrap_or(true);
            Item::new(BandChange, if ok { Info } else { Warning })
                .technical()
                .num("primary", n("primary"))
                .param("outcome", text(s("outcome").unwrap_or("")))
        }
        SEQUENCE_RESUMED => Item::new(BandChange, Info)
            .num("primary", n("primary"))
            .num("step", n("step")),
        SEQUENCE_DISCARDED => Item::new(BandChange, Warning)
            .param("why", text(s("why").unwrap_or("")))
            .why("events.why.sequenceDiscarded"),
        SEQUENCE_CANCELLED => Item::new(BandChange, Info)
            .technical()
            .param("why", text(s("why").unwrap_or(""))),
        SEQUENCE_WRITE_FAILED | SEQUENCE_CLEAR_FAILED => Item::new(BandChange, Problem).technical(),
        RELOCK_STARTED => Item::new(BandChange, Info)
            .num("primary", n("primary"))
            .num("servingBand", n("serving_band")),
        RELOCK_OUTCOME => {
            let ok = b("ok")?;
            Item::new(BandChange, if ok { Good } else { Warning })
                .key(
                    if ok {
                        "events.kind.relockSucceeded"
                    } else {
                        "events.kind.relockFailed"
                    }
                    .into(),
                )
                .num("primary", n("primary"))
                .num("failures", n("failures"))
        }
        RELOCK_GAVE_UP => Item::new(BandChange, Warning)
            .num("primary", n("primary"))
            .num("attempts", n("attempts"))
            .why("events.why.relockGaveUp"),
        RELOCK_RESET => Item::new(BandChange, Info).num("primary", n("primary")),
        AUTOMATION_SETTINGS => Item::new(App, Info)
            .num("primary", n("primary_band"))
            .param("relock", on_off(b("relock_primary")))
            .param("revertOnLoss", on_off(b("revert_on_loss"))),
        SPEEDTEST_ACTIVE => Item::new(SpeedTest, Info).technical().key(
            if b("active")? {
                "events.kind.speedtestStarted"
            } else {
                "events.kind.speedtestStopped"
            }
            .into(),
        ),
        SCAN_ACTIVE => Item::new(BandChange, Info).key(
            if b("active")? {
                "events.kind.scanStarted"
            } else {
                "events.kind.scanStopped"
            }
            .into(),
        ),
        SPEEDTEST_FAILED => Item::new(SpeedTest, Warning)
            .param(
                "reason",
                EventParam::Key(format!("speedtest.fail.{}", s("reason")?)),
            )
            .param("provider", text(s("provider").unwrap_or(""))),
        SPEEDTEST_AUTO_SKIPPED => Item::new(SpeedTest, Info).param(
            "reason",
            EventParam::Key(format!("events.why.{}", s("reason")?)),
        ),
        DEVICE_REBOOT => Item::new(Device, Info),
        DEVICE_FACTORY_RESET => Item::new(Device, Warning),
        SESSION => {
            let k = s("kind")?;
            let severity = match k {
                "loggedIn" => Good,
                "connecting" | "notConfigured" | "confirmRouter" | "loggedOut" => Info,
                "reconnecting" | "takenOver" => Warning,
                "badCredentials" | "lockedOut" | "algorithmMismatch" | "loginRejected" => Problem,
                _ => return None,
            };
            let mut item = Item::new(Connection, severity)
                .key(format!("events.kind.session{}", camel(&format!("_{k}"))))
                .num("retryInS", n("retryInS"))
                .num("waitS", n("waitS"))
                .num("code", n("code"));
            if k == "connecting" || k == "reconnecting" {
                item = item.technical();
            }
            item
        }
        SESSION_LOST => Item::new(Connection, Warning)
            .why("events.why.sessionLost")
            .router_code("events.kind.sessionLostCode", d),
        SESSION_REFRESHED => {
            let ok = b("ok").unwrap_or(false);
            Item::new(Connection, if ok { Info } else { Warning }).technical()
        }
        SESSION_IDENTITY_NOT_SAVED | SESSION_MARKER_NOT_SAVED | SESSION_LOGOUT_MARKER_NOT_SAVED => {
            Item::new(Connection, Problem).technical()
        }
        SESSION_LOGOUT => {
            if b("ok").unwrap_or(false) {
                Item::new(Connection, Info).technical()
            } else {
                Item::new(Connection, Warning)
                    .technical()
                    .key("events.kind.sessionLogoutUnconfirmed".into())
            }
        }
        SESSION_AUTO_CONNECT_SKIPPED | SESSION_LOGOUT_OVERRIDDEN => {
            Item::new(Connection, Info).technical()
        }
        HEARTBEAT_FAILED => Item::new(Connection, Warning).technical(),
        CELL_CHANGED => {
            let band = |side: &str| -> Option<String> {
                Some(d.get(side)?.get("band")?.as_str()?.to_string())
            };
            let (from, to) = (band("from")?, band("to")?);
            let item = Item::new(Connection, Info);
            if from == to {
                item.key("events.kind.cellChangedSameBand".into())
                    .param("band", text(&to))
            } else {
                item.param("from", text(&from)).param("to", text(&to))
            }
        }
        FEATURE_REFUSED => Item::new(Connection, Info)
            .technical()
            .param("what", text(s("what")?))
            .router_code("events.kind.featureRefusedCode", d),
        WAN_RANGE_CHANGED => Item::new(Connection, Info)
            .param("from", text(s("from")?))
            .param("to", text(s("to")?)),
        IP_REFRESH_STARTED => {
            let item = Item::new(Connection, Info).num("maxTries", n("max_tries"));
            match s("target") {
                Some(t) => item
                    .key("events.kind.ipRefreshStartedTarget".into())
                    .param("target", text(t)),
                None => item,
            }
        }
        IP_REFRESH_TRY => {
            let item = Item::new(Connection, Info)
                .technical()
                .num("n", n("n"))
                .num("offS", n("off_s"));
            let search = s("method") == Some("networkSearch");
            match (s("from"), s("to"), search) {
                (Some(from), Some(to), false) => {
                    item.param("from", text(from)).param("to", text(to))
                }
                (Some(from), Some(to), true) => item
                    .key("events.kind.ipRefreshTrySearch".into())
                    .param("from", text(from))
                    .param("to", text(to)),
                (_, _, false) => item.key("events.kind.ipRefreshTryNoRange".into()),
                (_, _, true) => item.key("events.kind.ipRefreshTrySearchNoRange".into()),
            }
        }
        IP_REFRESH_SEARCH => {
            let item = Item::new(Connection, Info)
                .technical()
                .num("n", n("n"))
                .num("seconds", n("seconds"));
            if b("timed_out").unwrap_or(false) {
                item.key("events.kind.ipRefreshSearchTimedOut".into())
            } else if d.get("error").is_some_and(|e| !e.is_null()) {
                item.key("events.kind.ipRefreshSearchFailed".into())
                    .router_code("events.kind.ipRefreshSearchFailedCode", d)
            } else {
                match n("networks") {
                    Some(count) => item.num("networks", Some(count)),
                    None => item.key("events.kind.ipRefreshSearchNoCount".into()),
                }
            }
        }
        IP_REFRESH_METHOD_FALLBACK => {
            Item::new(Connection, Info).router_code("events.kind.ipRefreshMethodFallbackCode", d)
        }
        IP_REFRESH_BANDS_CHANGED => Item::new(BandChange, Warning)
            .param(
                "from",
                EventParam::Bands(d.get("before").and_then(bands_of)?),
            )
            .param("to", EventParam::Bands(d.get("after").and_then(bands_of)?))
            .why("events.why.ipRefreshBandsChanged"),
        IP_REFRESH_ENDED | IP_REFRESH_RESUMED => {
            let outcome = s("outcome")?;
            let severity = match outcome {
                "matched" | "changed" => Good,
                "unchanged" | "notMatched" | "stopped" | "rateLimited" | "resumed" => Info,
                "onlineTimeout" | "hardCap" | "sessionLost" | "offRefused" | "journalFailed"
                | "dataOnFailed" | "searchRefused" => Warning,
                _ => return None,
            };
            let item = Item::new(Connection, severity)
                .param(
                    "outcome",
                    EventParam::Key(format!("ipRefresh.outcome.{outcome}")),
                )
                .num("tries", n("tries"));
            let item = if kind == IP_REFRESH_RESUMED {
                item.why("events.why.ipRefreshResumed")
            } else {
                item.router_code("events.kind.ipRefreshEndedCode", d)
            };
            match s("to") {
                Some(to) => item.param("to", text(to)),
                None => item,
            }
        }
        IP_REATTACH_STARTED => Item::new(BandChange, Info)
            .param(
                "bands",
                EventParam::Bands(d.get("step1").and_then(bands_of)?),
            )
            .param(
                "finalBands",
                EventParam::Bands(d.get("final").and_then(bands_of)?),
            ),
        IP_REATTACH_STEP1_KEPT => Item::new(BandChange, Info).technical(),
        IP_REATTACH_ENDED => {
            let outcome = s("outcome")?;
            let severity = match outcome {
                "changed" => Good,
                "unchanged" | "unknown" | "reverted" | "cancelled" => Info,
                "step1Failed" | "step2Failed" => Warning,
                _ => return None,
            };
            let item = Item::new(Connection, severity).param(
                "outcome",
                EventParam::Key(format!("ipReattach.outcome.{outcome}")),
            );
            match s("to") {
                Some(to) => item
                    .key("events.kind.ipReattachEndedTo".into())
                    .param("to", text(to)),
                None => item,
            }
        }
        IP_REFRESH_DATA_ON_FAILED => Item::new(Connection, Problem)
            .num("attempt", n("attempt"))
            .why("events.why.dataOnFailed"),
        IP_REFRESH_MANUAL_ON => {
            if b("ok").unwrap_or(false) {
                Item::new(Connection, Info)
            } else {
                Item::new(Connection, Warning).key("events.kind.ipRefreshManualOnFailed".into())
            }
        }
        APN_STARTED | APN_REATTACHED => Item::new(Connection, Info).num("index", n("index")),
        APN_APPLIED | APN_VERIFIED => Item::new(Connection, Info)
            .technical()
            .num("index", n("index")),
        APN_COMMITTED => Item::new(Connection, Good).num("index", n("index")),
        APN_REVERTED => {
            let reason = revert_reason_key(s("reason")?)?;
            Item::new(Connection, Warning)
                .num("index", n("index"))
                .param("reason", EventParam::Key(reason.into()))
        }
        APN_REVERT_FAILED => Item::new(Connection, Problem)
            .num("index", n("index"))
            .why("events.why.apnRevertFailed"),
        APN_REJECTED => Item::new(Connection, Warning).param("message", text(s("message")?)),
        APN_RESUMED => Item::new(Connection, Info)
            .num("index", n("index"))
            .param("phase", text(phase_kind(d).unwrap_or(""))),
        APN_PROFILE_ADDED | APN_PROFILE_EDITED | APN_PROFILE_DELETED => {
            Item::new(Connection, Info).num("index", n("index"))
        }
        APN_UNEXPECTED_SWITCH => {
            let item = Item::new(Connection, Warning)
                .num("index", n("index"))
                .num("now", n("now"));
            if b("reverting").unwrap_or(false) {
                item.why("events.why.apnUnexpectedSwitch")
            } else {
                item.key("events.kind.apnUnexpectedSwitchKept".into())
                    .why("events.why.apnUnexpectedSwitchKept")
            }
        }
        NOTIFY => Item::new(App, Info)
            .technical()
            .param("key", EventParam::Key(s("key")?.to_string()))
            .param("result", text(s("result").unwrap_or(""))),
        ALERTS_LAGGED => Item::new(App, Warning)
            .technical()
            .num("skipped", n("skipped")),
        APP_STARTED | APP_STOPPED => {
            let item = Item::new(App, Info);
            match s("version") {
                Some(v) => item.param("version", text(v)),
                None => item,
            }
        }
        HISTORY_PRUNED | HISTORY_IMPORTED => {
            Item::new(App, Info).technical().num("rows", n("rows"))
        }
        HISTORY_CLEARED => Item::new(App, Info),
        HISTORY_RESET => Item::new(App, Warning).why("events.why.historyReset"),
        HISTORY_DROPPED => Item::new(App, Warning).technical().num("count", n("count")),
        HISTORY_EXPORTED => Item::new(App, Info).num("rows", n("rows")).param(
            "dataset",
            EventParam::Key(format!("history.export.dataset.{}", s("dataset")?)),
        ),
        DIAGNOSTICS_PAGE_SCRIPTS_SAVED => Item::new(App, Info)
            .num("files", n("files"))
            .num("skipped", n("skipped")),
        PRIVACY_CHECKED => Item::new(App, Info),
        _ => return None,
    };
    Some(item)
}

fn outcome_of(e: &EventRecord) -> Option<ChangeOutcome> {
    use kinds::*;
    Some(match e.kind.as_str() {
        SAFE_APPLY_COMMITTED => ChangeOutcome::Kept,
        SAFE_APPLY_REVERTED => ChangeOutcome::Reverted {
            reason: e
                .detail
                .get("reason")
                .and_then(Value::as_str)
                .and_then(revert_reason_key)
                .unwrap_or("safeApply.reason.unknown")
                .to_string(),
            fallback: e
                .detail
                .get("fallback")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        SAFE_APPLY_REVERT_FAILED => ChangeOutcome::RevertFailed,
        SAFE_APPLY_REJECTED => ChangeOutcome::Rejected,
        _ => return None,
    })
}

/// Records that belong to an open change.
fn is_change_step(kind: &str) -> bool {
    kind.starts_with("safe_apply.") || kind.starts_with("journal.")
}

fn is_snapshot_read(e: &EventRecord) -> bool {
    e.kind == kinds::SAFE_APPLY_READ && phase_kind(&e.detail) == Some("snapshotting")
}

fn new_group(started_ms: u64, primary: Option<u16>) -> ChangeGroup {
    ChangeGroup {
        started_ms,
        ended_ms: None,
        from: Vec::new(),
        to: Vec::new(),
        primary,
        outcome: ChangeOutcome::Open,
        steps: Vec::new(),
    }
}

fn add_step(g: &mut ChangeGroup, e: &EventRecord) {
    if g.from.is_empty() && is_snapshot_read(e) {
        if let Some(b) = e.detail.get("config").and_then(bands_of) {
            g.from = b;
        }
    }
    if g.to.is_empty()
        && e.kind == kinds::SAFE_APPLY_POST
        && phase_kind(&e.detail) == Some("applying")
    {
        if let Some(b) = e.detail.get("config").and_then(bands_of) {
            g.to = b;
        }
    }
    g.steps.push(describe(e));
}

/// Folds each band change into one [`ChangeGroup`]; every other record stays a single item.
/// Records are taken in time order (oldest first) and the entries come out the same way.
///
/// A change runs from its snapshot read (or `safe_apply.started`) to its end: kept, reverted,
/// revert failed or rejected. One that the log doesn't see end stays `Open`.
pub fn group_changes(records: &[EventRecord]) -> Vec<EventEntry> {
    let mut sorted: Vec<&EventRecord> = records.iter().collect();
    sorted.sort_by_key(|e| e.at_ms);

    let mut out = Vec::new();
    let mut open: Option<ChangeGroup> = None;
    // A snapshot read not yet followed by `started`.
    let mut snapshot: Option<&EventRecord> = None;
    let mut primary: Option<u16> = None;

    let flush_snapshot = |snapshot: &mut Option<&EventRecord>, out: &mut Vec<EventEntry>| {
        if let Some(s) = snapshot.take() {
            out.push(EventEntry::Single { item: describe(s) });
        }
    };

    for e in sorted {
        let kind = e.kind.as_str();
        if kind == kinds::SEQUENCE_STARTED {
            let reattach = e.detail.get("origin").and_then(Value::as_str) == Some("ipReattach");
            primary = e
                .detail
                .get("primary")
                .and_then(Value::as_u64)
                .and_then(|p| u16::try_from(p).ok())
                .filter(|_| !reattach);
        }
        let starts_change = is_snapshot_read(e) || kind == kinds::SAFE_APPLY_STARTED;

        if let Some(g) = open.as_mut() {
            if is_change_step(kind) && !starts_change {
                add_step(g, e);
                if let Some(outcome) = outcome_of(e) {
                    g.outcome = outcome;
                    g.ended_ms = Some(e.at_ms);
                    out.push(EventEntry::Change {
                        group: open.take().expect("open group"),
                    });
                }
                continue;
            }
            if starts_change {
                // A new change without an end for the last one: leave that one open.
                out.push(EventEntry::Change {
                    group: open.take().expect("open group"),
                });
            }
        }

        if is_snapshot_read(e) {
            flush_snapshot(&mut snapshot, &mut out);
            snapshot = Some(e);
        } else if kind == kinds::SAFE_APPLY_STARTED {
            let started = snapshot.map_or(e.at_ms, |s| s.at_ms);
            let mut g = new_group(started, primary);
            if let Some(s) = snapshot.take() {
                add_step(&mut g, s);
            }
            add_step(&mut g, e);
            open = Some(g);
        } else if kind == kinds::SAFE_APPLY_REJECTED && snapshot.is_some() {
            // Refused after the snapshot, before the journal: still one change.
            let s = snapshot.take().expect("snapshot");
            let mut g = new_group(s.at_ms, primary);
            add_step(&mut g, s);
            add_step(&mut g, e);
            g.outcome = ChangeOutcome::Rejected;
            g.ended_ms = Some(e.at_ms);
            out.push(EventEntry::Change { group: g });
        } else {
            out.push(EventEntry::Single { item: describe(e) });
        }

        if matches!(kind, kinds::SEQUENCE_ENDED | kinds::SEQUENCE_CANCELLED) {
            primary = None;
        }
    }
    flush_snapshot(&mut snapshot, &mut out);
    if let Some(g) = open.take() {
        out.push(EventEntry::Change { group: g });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_apply::RevertReason;
    use serde_json::json;

    fn rec(at_ms: u64, kind: &str, detail: Value) -> EventRecord {
        EventRecord {
            at_ms,
            kind: kind.into(),
            detail,
        }
    }

    fn config(bands: &[u8]) -> Value {
        json!({
            "networkMode": "auto",
            "lteBands": LteBandSet::from_bands(bands).unwrap(),
            "legacyBandRaw": "3FFFFFFF",
            "nrOption": null,
        })
    }

    /// A plausible detail for each kind, as the app writes it.
    fn sample(kind: &str) -> Value {
        use kinds::*;
        match kind {
            SAFE_APPLY_REVERTED => {
                json!({"kind": "reverted", "reason": "noReattach", "fallback": false})
            }
            SAFE_APPLY_REJECTED => json!({"kind": "rejected", "message": "refused"}),
            SAFE_APPLY_LOSS_IGNORED => json!({"reason": "lossOfService", "revert_on_loss": false}),
            SAFE_APPLY_READ => {
                json!({"phase": {"kind": "snapshotting"}, "config": config(&[1, 3])})
            }
            SAFE_APPLY_POST => {
                json!({"phase": {"kind": "applying"}, "config": config(&[3]), "ok": true})
            }
            SAFE_APPLY_RESUMED => json!({"phase": {"kind": "probation"}}),
            RELOCK_OUTCOME => json!({"primary": 3, "ok": true, "failures": 0}),
            SPEEDTEST_ACTIVE | SCAN_ACTIVE => json!({"active": true}),
            SPEEDTEST_FAILED => {
                json!({"provider": "cloudflare", "reason": "timeout", "message": "x"})
            }
            SPEEDTEST_AUTO_SKIPPED => json!({"reason": "bandChange", "slotMs": 1}),
            SESSION => json!({"kind": "loggedIn"}),
            CELL_CHANGED => json!({
                "from": {"rat": "lte", "pci": 101, "band": "B3"},
                "to": {"rat": "lte", "pci": 102, "band": "B3"},
            }),
            WAN_RANGE_CHANGED => json!({"from": "10.45", "to": "10.62"}),
            FEATURE_REFUSED => json!({"what": "apn", "error": {"kind": "sessionLost"}}),
            IP_REFRESH_STARTED => json!({"target": "10.45", "max_tries": 3, "from": "10.62"}),
            IP_REFRESH_TRY => json!({"n": 1, "from": "10.62", "to": "10.45", "off_s": 5}),
            IP_REFRESH_ENDED => {
                json!({"outcome": "matched", "tries": 1, "from": "10.62", "to": "10.45"})
            }
            IP_REFRESH_RESUMED => json!({"outcome": "resumed", "tries": 1}),
            IP_REFRESH_DATA_ON_FAILED => json!({"attempt": 2, "error": {"kind": "timeout"}}),
            IP_REFRESH_MANUAL_ON => json!({"ok": true}),
            IP_REFRESH_SEARCH => json!({
                "n": 1, "seconds": 42, "networks": 3, "timed_out": false,
                "service_dropped": true, "error": null,
            }),
            IP_REFRESH_METHOD_FALLBACK => json!({
                "from": "dataSwitch", "to": "networkSearch",
                "error": {"kind": "sessionLost", "code": 100003},
            }),
            IP_REFRESH_BANDS_CHANGED => {
                json!({"before": config(&[3]), "after": config(&[1, 3, 20, 28])})
            }
            IP_REATTACH_STARTED => {
                json!({"from": "10.62", "step1": config(&[3]), "final": config(&[1, 3])})
            }
            IP_REATTACH_STEP1_KEPT => json!({"range": "10.45"}),
            IP_REATTACH_ENDED => json!({
                "outcome": "changed", "from": "10.62", "to": "10.45", "bands_restored": true,
            }),
            APN_STARTED | APN_APPLIED | APN_VERIFIED | APN_REATTACHED | APN_COMMITTED
            | APN_REVERT_FAILED => json!({"index": 2, "phase": {"kind": "applying"}}),
            APN_REVERTED => {
                json!({"index": 2, "phase": {"kind": "reverted"}, "reason": "noReattach"})
            }
            APN_REJECTED => json!({"index": 2, "message": "refused"}),
            APN_RESUMED => json!({"index": 2, "phase": {"kind": "probation"}}),
            APN_PROFILE_ADDED | APN_PROFILE_EDITED | APN_PROFILE_DELETED => json!({"index": 4}),
            APN_UNEXPECTED_SWITCH => json!({"index": 1, "now": 4, "reverting": true}),
            NOTIFY => json!({"key": "notify.cellChanged", "result": "queued", "appId": true}),
            HISTORY_EXPORTED => json!({"dataset": "hourly", "rows": 3, "cellNumbers": false}),
            DIAGNOSTICS_PAGE_SCRIPTS_SAVED => json!({"files": 12, "bytes": 34567, "skipped": 1}),
            PRIVACY_CHECKED => json!({
                "remoteManagement": {"result": "ok", "tags": ["AcsUrl"]},
                "devices": {"result": "refused", "code": 100003},
            }),
            _ => json!({}),
        }
    }

    #[test]
    fn every_kind_has_a_mapping() {
        for kind in kinds::ALL {
            let item = describe(&rec(1, kind, sample(kind)));
            assert_ne!(item.key, "events.kind.unknown", "{kind}");
            assert!(item.key.starts_with("events.kind."), "{kind}: {}", item.key);
        }
    }

    #[test]
    fn kinds_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for k in kinds::ALL {
            assert!(seen.insert(*k), "{k} listed twice");
        }
    }

    #[test]
    fn camel_case_keys() {
        assert_eq!(
            camel("safe_apply.fell_back_to_auto"),
            "safeApplyFellBackToAuto"
        );
        let item = describe(&rec(1, kinds::DEVICE_REBOOT, json!({})));
        assert_eq!(item.key, "events.kind.deviceReboot");
        assert_eq!(item.category, EventCategory::Device);
    }

    #[test]
    fn each_revert_reason_maps_to_its_key() {
        let cases = [
            (RevertReason::ApplyRejected, "safeApply.reason.rejected"),
            (
                RevertReason::VerifyMismatch,
                "safeApply.reason.verifyMismatch",
            ),
            (RevertReason::NoReattach, "safeApply.reason.noReattach"),
            (
                RevertReason::LossOfService,
                "safeApply.reason.connectionLost",
            ),
            (RevertReason::UserRequested, "safeApply.reason.user"),
            (
                RevertReason::ResumedWithoutService,
                "safeApply.reason.resumedWithoutService",
            ),
        ];
        for (reason, key) in cases {
            let event = crate::SafeApplyEvent::Reverted {
                reason,
                fallback: false,
            };
            let detail = serde_json::to_value(&event).unwrap();
            let item = describe(&rec(1, kinds::SAFE_APPLY_REVERTED, detail));
            assert_eq!(item.params["reason"], EventParam::Key(key.into()));
            assert_eq!(item.severity, Severity::Warning);
            assert_eq!(item.detail_key, None);
        }
        let fallback = describe(&rec(
            1,
            kinds::SAFE_APPLY_REVERTED,
            json!({"kind": "reverted", "reason": "noReattach", "fallback": true}),
        ));
        assert_eq!(
            fallback.detail_key.as_deref(),
            Some("events.why.fellBackToAuto")
        );
    }

    #[test]
    fn speedtest_failure_reason_is_a_key() {
        let item = describe(&rec(
            1,
            kinds::SPEEDTEST_FAILED,
            json!({"provider": "ookla", "reason": "noInternet", "message": "dns"}),
        ));
        assert_eq!(
            item.params["reason"],
            EventParam::Key("speedtest.fail.noInternet".into())
        );
        assert!(!item.params.contains_key("message"));
    }

    /// session.lost, feature.refused and ip_refresh.ended name the router's own code when the
    /// error has one, and read as before when it doesn't.
    #[test]
    fn router_codes_are_named() {
        let lost = describe(&rec(
            1,
            kinds::SESSION_LOST,
            json!({"error": {"kind": "sessionLost", "code": 100003}}),
        ));
        assert_eq!(lost.key, "events.kind.sessionLostCode");
        assert_eq!(lost.params["code"], EventParam::Number(100003.0));
        assert_eq!(lost.detail_key.as_deref(), Some("events.why.sessionLost"));
        let plain = describe(&rec(
            1,
            kinds::SESSION_LOST,
            json!({"error": {"kind": "sessionLost"}}),
        ));
        assert_eq!(plain.key, "events.kind.sessionLost");
        assert!(!plain.params.contains_key("code"));

        let refused = describe(&rec(
            1,
            kinds::FEATURE_REFUSED,
            json!({"what": "apn", "error": {"kind": "api", "code": 100002, "errorKind": "notSupported"}}),
        ));
        assert_eq!(refused.key, "events.kind.featureRefusedCode");
        assert_eq!(refused.params["code"], EventParam::Number(100002.0));
        assert_eq!(refused.params["what"], EventParam::Text("apn".into()));

        let ended = describe(&rec(
            1,
            kinds::IP_REFRESH_ENDED,
            json!({"outcome": "offRefused", "tries": 1, "refusal": {"kind": "sessionLost", "code": 125003}}),
        ));
        assert_eq!(ended.key, "events.kind.ipRefreshEndedCode");
        assert_eq!(ended.params["code"], EventParam::Number(125003.0));
        let ended = describe(&rec(
            1,
            kinds::IP_REFRESH_ENDED,
            json!({"outcome": "offRefused", "tries": 1}),
        ));
        assert_eq!(ended.key, "events.kind.ipRefreshEnded");
    }

    #[test]
    fn session_sub_kinds() {
        let key = |detail: Value| describe(&rec(1, kinds::SESSION, detail));
        let ok = key(json!({"kind": "loggedIn"}));
        assert_eq!(ok.key, "events.kind.sessionLoggedIn");
        assert_eq!(ok.severity, Severity::Good);
        let r = key(json!({"kind": "reconnecting", "reason": "x", "retryInS": 30}));
        assert_eq!(r.key, "events.kind.sessionReconnecting");
        assert_eq!(r.params["retryInS"], EventParam::Number(30.0));
        assert!(r.technical);
        let bad = key(json!({"kind": "badCredentials"}));
        assert_eq!(bad.key, "events.kind.sessionBadCredentials");
        assert_eq!(bad.severity, Severity::Problem);
        let locked = key(json!({"kind": "lockedOut", "waitS": 60}));
        assert_eq!(locked.key, "events.kind.sessionLockedOut");
        assert_eq!(locked.params["waitS"], EventParam::Number(60.0));
        let rejected = key(json!({"kind": "loginRejected", "code": 125099}));
        assert_eq!(rejected.params["code"], EventParam::Number(125099.0));
        assert_eq!(
            key(json!({"kind": "confirmRouter", "firstTime": true})).key,
            "events.kind.sessionConfirmRouter"
        );
        assert_eq!(
            key(json!({"kind": "somethingNew"})).key,
            "events.kind.unknown"
        );

        let lost = describe(&rec(
            1,
            kinds::SESSION_LOST,
            json!({"error": {"kind": "sessionLost"}}),
        ));
        assert_eq!(lost.key, "events.kind.sessionLost");
        assert_eq!(lost.category, EventCategory::Connection);
        let refreshed = describe(&rec(1, kinds::SESSION_REFRESHED, json!({"ok": true})));
        assert_eq!(refreshed.key, "events.kind.sessionRefreshed");
        assert!(refreshed.technical);

        let out = key(json!({"kind": "loggedOut"}));
        assert_eq!(out.key, "events.kind.sessionLoggedOut");
        assert_eq!(out.category, EventCategory::Connection);
        assert_eq!(out.severity, Severity::Info);
        assert!(!out.technical);
    }

    #[test]
    fn logout_events() {
        let ok = describe(&rec(
            1,
            kinds::SESSION_LOGOUT,
            json!({"ok": true, "router_state": "loggedOut"}),
        ));
        assert_eq!(ok.key, "events.kind.sessionLogout");
        assert_eq!(ok.severity, Severity::Info);
        assert!(ok.technical);
        let failed = describe(&rec(
            1,
            kinds::SESSION_LOGOUT,
            json!({"ok": false, "error": {"kind": "timeout"}}),
        ));
        assert_eq!(failed.key, "events.kind.sessionLogoutUnconfirmed");
        assert_eq!(failed.severity, Severity::Warning);
        for kind in [
            kinds::SESSION_AUTO_CONNECT_SKIPPED,
            kinds::SESSION_LOGOUT_OVERRIDDEN,
        ] {
            let item = describe(&rec(1, kind, json!({"why": "logged_out"})));
            assert_eq!(item.category, EventCategory::Connection);
            assert_eq!(item.severity, Severity::Info);
            assert!(item.technical, "{kind}");
        }
    }

    #[test]
    fn ip_refresh_events_name_ranges_and_outcomes() {
        let ended = describe(&rec(
            1,
            kinds::IP_REFRESH_ENDED,
            sample(kinds::IP_REFRESH_ENDED),
        ));
        assert_eq!(ended.key, "events.kind.ipRefreshEnded");
        assert_eq!(ended.severity, Severity::Good);
        assert_eq!(
            ended.params["outcome"],
            EventParam::Key("ipRefresh.outcome.matched".into())
        );
        assert_eq!(ended.params["to"], EventParam::Text("10.45".into()));
        let stuck = describe(&rec(
            1,
            kinds::IP_REFRESH_ENDED,
            json!({"outcome": "onlineTimeout", "tries": 2}),
        ));
        assert_eq!(stuck.severity, Severity::Warning);
        assert!(!stuck.params.contains_key("to"));
        let odd = describe(&rec(
            1,
            kinds::IP_REFRESH_ENDED,
            json!({"outcome": "later"}),
        ));
        assert_eq!(odd.key, "events.kind.unknown");
        let started = describe(&rec(
            1,
            kinds::IP_REFRESH_STARTED,
            sample(kinds::IP_REFRESH_STARTED),
        ));
        assert_eq!(started.key, "events.kind.ipRefreshStartedTarget");
        let plain = describe(&rec(1, kinds::IP_REFRESH_STARTED, json!({"max_tries": 1})));
        assert_eq!(plain.key, "events.kind.ipRefreshStarted");
        let tried = describe(&rec(
            1,
            kinds::IP_REFRESH_TRY,
            sample(kinds::IP_REFRESH_TRY),
        ));
        assert!(tried.technical);
        let failed = describe(&rec(
            1,
            kinds::IP_REFRESH_MANUAL_ON,
            json!({"ok": false, "error": {"kind": "timeout"}}),
        ));
        assert_eq!(failed.key, "events.kind.ipRefreshManualOnFailed");
        let foreign = describe(&rec(
            1,
            kinds::JOURNAL_FOREIGN,
            json!({"file": "data_switch", "id": "x"}),
        ));
        assert_eq!(foreign.key, "events.kind.dataJournalForeign");
        assert_eq!(foreign.category, EventCategory::Connection);
    }

    #[test]
    fn cell_change_names_bands_never_pci() {
        let same = describe(&rec(1, kinds::CELL_CHANGED, sample(kinds::CELL_CHANGED)));
        assert_eq!(same.key, "events.kind.cellChangedSameBand");
        assert_eq!(same.params["band"], EventParam::Text("B3".into()));
        assert!(!same.params.contains_key("pci"));
        let moved = describe(&rec(
            1,
            kinds::CELL_CHANGED,
            json!({
                "from": {"rat": "lte", "pci": 101, "band": "B3"},
                "to": {"rat": "lte", "pci": 103, "band": "B20"},
            }),
        ));
        assert_eq!(moved.key, "events.kind.cellChanged");
        assert_eq!(moved.params["from"], EventParam::Text("B3".into()));
        assert_eq!(moved.params["to"], EventParam::Text("B20".into()));
        let json = serde_json::to_string(&moved).unwrap();
        assert!(!json.contains("101") && !json.contains("103"), "{json}");
    }

    #[test]
    fn malformed_details_give_unknown() {
        for (kind, detail) in [
            (kinds::SAFE_APPLY_REVERTED, json!("nonsense")),
            (
                kinds::SAFE_APPLY_REVERTED,
                json!({"reason": 7, "fallback": false}),
            ),
            (kinds::SESSION, json!(null)),
            (kinds::CELL_CHANGED, json!({"from": [], "to": 3})),
            (kinds::SPEEDTEST_ACTIVE, json!({"active": "yes"})),
            (kinds::SAFE_APPLY_READ, json!([1, 2])),
            ("brand.new_kind", json!({})),
        ] {
            let item = describe(&rec(5, kind, detail));
            assert_eq!(item.key, "events.kind.unknown", "{kind}");
            assert_eq!(item.params["code"], EventParam::Text(kind.into()));
            assert!(item.technical);
        }
    }

    #[test]
    fn read_post_carry_bands() {
        let item = describe(&rec(
            1,
            kinds::SAFE_APPLY_READ,
            sample(kinds::SAFE_APPLY_READ),
        ));
        assert_eq!(item.params["bands"], EventParam::Bands(vec![1, 3]));
        let failed = describe(&rec(
            1,
            kinds::SAFE_APPLY_POST,
            json!({"phase": {"kind": "applying"}, "config": config(&[3]), "error": {"kind": "timeout"}}),
        ));
        assert_eq!(failed.key, "events.kind.safeApplyPostFailed");
    }

    /// A change of bands [1, 3] -> [3] ending in `end`.
    fn change(t0: u64, end: Option<(&str, Value)>) -> Vec<EventRecord> {
        let mut v = vec![
            rec(
                t0,
                kinds::SAFE_APPLY_READ,
                json!({"phase": {"kind": "snapshotting"}, "config": config(&[1, 3])}),
            ),
            rec(
                t0 + 1,
                kinds::SAFE_APPLY_STARTED,
                json!({"kind": "started"}),
            ),
            rec(
                t0 + 2,
                kinds::SAFE_APPLY_POST,
                json!({"phase": {"kind": "applying"}, "config": config(&[3]), "ok": true}),
            ),
            rec(
                t0 + 3,
                kinds::SAFE_APPLY_APPLIED,
                json!({"kind": "applied"}),
            ),
        ];
        if let Some((kind, detail)) = end {
            v.push(rec(t0 + 10, kind, detail));
        }
        v
    }

    fn only_group(entries: &[EventEntry]) -> &ChangeGroup {
        let groups: Vec<_> = entries
            .iter()
            .filter_map(|e| match e {
                EventEntry::Change { group } => Some(group),
                _ => None,
            })
            .collect();
        assert_eq!(groups.len(), 1, "{entries:?}");
        groups[0]
    }

    #[test]
    fn group_kept() {
        let entries = group_changes(&change(100, Some((kinds::SAFE_APPLY_COMMITTED, json!({})))));
        assert_eq!(entries.len(), 1);
        let g = only_group(&entries);
        assert_eq!(g.started_ms, 100);
        assert_eq!(g.ended_ms, Some(110));
        assert_eq!(g.from, vec![1, 3]);
        assert_eq!(g.to, vec![3]);
        assert_eq!(g.primary, None);
        assert_eq!(g.outcome, ChangeOutcome::Kept);
        assert_eq!(g.steps.len(), 5);
    }

    #[test]
    fn group_reverted_and_fallback() {
        let entries = group_changes(&change(
            100,
            Some((
                kinds::SAFE_APPLY_REVERTED,
                json!({"kind": "reverted", "reason": "lossOfService", "fallback": false}),
            )),
        ));
        assert_eq!(
            only_group(&entries).outcome,
            ChangeOutcome::Reverted {
                reason: "safeApply.reason.connectionLost".into(),
                fallback: false,
            }
        );

        let mut records = change(100, None);
        records.push(rec(
            105,
            kinds::SAFE_APPLY_POST,
            json!({"phase": {"kind": "reverting", "attempt": 1, "fallback": true}, "config": config(&[1, 3, 20, 28]), "ok": true}),
        ));
        records.push(rec(106, kinds::SAFE_APPLY_FELL_BACK_TO_AUTO, json!({})));
        records.push(rec(
            107,
            kinds::SAFE_APPLY_REVERTED,
            json!({"kind": "reverted", "reason": "noReattach", "fallback": true}),
        ));
        let entries = group_changes(&records);
        let g = only_group(&entries);
        assert_eq!(g.to, vec![3], "the revert POST isn't the requested set");
        assert_eq!(
            g.outcome,
            ChangeOutcome::Reverted {
                reason: "safeApply.reason.noReattach".into(),
                fallback: true,
            }
        );
    }

    #[test]
    fn group_open_and_interleaved_singles() {
        let mut records = change(100, None);
        records.push(rec(104, kinds::CELL_CHANGED, sample(kinds::CELL_CHANGED)));
        let entries = group_changes(&records);
        assert_eq!(entries.len(), 2);
        assert!(matches!(&entries[0], EventEntry::Single { item } if item.kind == "cell.changed"));
        let g = only_group(&entries);
        assert_eq!(g.outcome, ChangeOutcome::Open);
        assert_eq!(g.ended_ms, None);
    }

    #[test]
    fn group_in_a_sequence_has_its_primary() {
        let mut records = vec![rec(
            90,
            kinds::SEQUENCE_STARTED,
            json!({"origin": "user", "primary": 3, "step1": config(&[3]), "final": config(&[1, 3])}),
        )];
        records.extend(change(100, Some((kinds::SAFE_APPLY_COMMITTED, json!({})))));
        records.push(rec(
            200,
            kinds::SEQUENCE_ENDED,
            json!({"ok": true, "outcome": "done"}),
        ));
        records.extend(change(300, Some((kinds::SAFE_APPLY_COMMITTED, json!({})))));
        let entries = group_changes(&records);
        let groups: Vec<_> = entries
            .iter()
            .filter_map(|e| match e {
                EventEntry::Change { group } => Some(group),
                _ => None,
            })
            .collect();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].primary, Some(3));
        assert_eq!(groups[1].primary, None);
        assert!(
            matches!(&entries[0], EventEntry::Single { item } if item.kind == "sequence.started")
        );
    }

    /// A band switch for a new IP has no primary band: its changes carry none, and its
    /// sequence start is a technical line.
    #[test]
    fn a_band_switch_for_a_new_ip_has_no_primary() {
        let started = rec(
            90,
            kinds::SEQUENCE_STARTED,
            json!({"origin": "ipReattach", "primary": 3, "step1": config(&[3]), "final": config(&[1, 3])}),
        );
        assert!(describe(&started).technical);
        let mut records = vec![started];
        records.extend(change(100, Some((kinds::SAFE_APPLY_COMMITTED, json!({})))));
        let entries = group_changes(&records);
        assert_eq!(only_group(&entries).primary, None);
    }

    #[test]
    fn rejected_without_a_snapshot_is_a_single() {
        let entries = group_changes(&[rec(
            1,
            kinds::SAFE_APPLY_REJECTED,
            json!({"kind": "rejected", "message": "a speed test is running"}),
        )]);
        assert!(matches!(&entries[..], [EventEntry::Single { .. }]));
    }

    #[test]
    fn rejected_after_a_snapshot_is_a_change() {
        let entries = group_changes(&[
            rec(
                1,
                kinds::SAFE_APPLY_READ,
                json!({"phase": {"kind": "snapshotting"}, "config": config(&[1, 3])}),
            ),
            rec(
                2,
                kinds::SAFE_APPLY_REJECTED,
                json!({"kind": "rejected", "message": "no"}),
            ),
        ]);
        let g = only_group(&entries);
        assert_eq!(g.outcome, ChangeOutcome::Rejected);
        assert_eq!(g.from, vec![1, 3]);
        assert!(g.to.is_empty());
    }

    #[test]
    fn a_change_without_bands_has_empty_sets() {
        let entries = group_changes(&[
            rec(1, kinds::SAFE_APPLY_STARTED, json!({})),
            rec(2, kinds::SAFE_APPLY_COMMITTED, json!({})),
        ]);
        let g = only_group(&entries);
        assert!(g.from.is_empty() && g.to.is_empty());
        assert_eq!(g.outcome, ChangeOutcome::Kept);
    }

    #[test]
    fn param_json_shape() {
        assert_eq!(
            serde_json::to_value(EventParam::Bands(vec![1, 3])).unwrap(),
            json!({"t": "bands", "v": [1, 3]})
        );
        assert_eq!(
            serde_json::to_value(ChangeOutcome::Reverted {
                reason: "safeApply.reason.user".into(),
                fallback: false
            })
            .unwrap(),
            json!({"kind": "reverted", "reason": "safeApply.reason.user", "fallback": false})
        );
    }

    /// A network search reads as a search, never as data that was off.
    #[test]
    fn network_search_events_read_as_searches() {
        let tried = describe(&rec(
            1,
            kinds::IP_REFRESH_TRY,
            json!({"n": 1, "from": "10.134", "to": "10.118", "off_s": 0, "method": "networkSearch"}),
        ));
        assert_eq!(tried.key, "events.kind.ipRefreshTrySearch");
        let search = |detail: Value| describe(&rec(1, kinds::IP_REFRESH_SEARCH, detail));
        let ok = search(sample(kinds::IP_REFRESH_SEARCH));
        assert_eq!(ok.key, "events.kind.ipRefreshSearch");
        assert_eq!(ok.params["networks"], EventParam::Number(3.0));
        let timed_out = search(
            json!({"n": 1, "seconds": 200, "timed_out": true, "error": {"kind": "timeout"}}),
        );
        assert_eq!(timed_out.key, "events.kind.ipRefreshSearchTimedOut");
        let refused = search(
            json!({"n": 1, "seconds": 1, "timed_out": false, "error": {"kind": "sessionLost", "code": 100003}}),
        );
        assert_eq!(refused.key, "events.kind.ipRefreshSearchFailedCode");
        assert_eq!(refused.params["code"], EventParam::Number(100003.0));
        let no_count = search(
            json!({"n": 2, "seconds": 30, "timed_out": false, "networks": null, "error": null}),
        );
        assert_eq!(no_count.key, "events.kind.ipRefreshSearchNoCount");

        let fallback = describe(&rec(
            1,
            kinds::IP_REFRESH_METHOD_FALLBACK,
            sample(kinds::IP_REFRESH_METHOD_FALLBACK),
        ));
        assert_eq!(fallback.key, "events.kind.ipRefreshMethodFallbackCode");
        assert_eq!(fallback.params["code"], EventParam::Number(100003.0));

        let changed = describe(&rec(
            1,
            kinds::IP_REFRESH_BANDS_CHANGED,
            sample(kinds::IP_REFRESH_BANDS_CHANGED),
        ));
        assert_eq!(changed.params["from"], EventParam::Bands(vec![3]));
        assert_eq!(changed.params["to"], EventParam::Bands(vec![1, 3, 20, 28]));
        assert_eq!(changed.severity, Severity::Warning);
        assert_eq!(
            changed.detail_key.as_deref(),
            Some("events.why.ipRefreshBandsChanged")
        );

        let ended = describe(&rec(
            1,
            kinds::IP_REFRESH_ENDED,
            json!({"outcome": "searchRefused", "tries": 1, "method": "networkSearch",
                   "refusal": {"kind": "sessionLost", "code": 100003}}),
        ));
        assert_eq!(ended.key, "events.kind.ipRefreshEndedCode");
        assert_eq!(ended.severity, Severity::Warning);
    }

    /// ip_reattach.ended reads by outcome, with the new range when one was read.
    #[test]
    fn ip_reattach_end_reads_by_outcome() {
        let changed = describe(&rec(
            1,
            kinds::IP_REATTACH_ENDED,
            json!({"outcome": "changed", "from": "10.62", "to": "10.45"}),
        ));
        assert_eq!(changed.key, "events.kind.ipReattachEndedTo");
        assert_eq!(changed.severity, Severity::Good);
        assert_eq!(changed.params["to"], EventParam::Text("10.45".into()));
        let failed = describe(&rec(
            1,
            kinds::IP_REATTACH_ENDED,
            json!({"outcome": "step1Failed", "from": "10.62", "to": null}),
        ));
        assert_eq!(failed.key, "events.kind.ipReattachEnded");
        assert_eq!(failed.severity, Severity::Warning);
        assert_eq!(
            failed.params["outcome"],
            EventParam::Key("ipReattach.outcome.step1Failed".into())
        );
        let odd = describe(&rec(1, kinds::IP_REATTACH_ENDED, json!({"outcome": "x"})));
        assert_eq!(odd.key, "events.kind.unknown");
    }
}
