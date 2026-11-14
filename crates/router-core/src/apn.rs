//! APN profiles: the router's list of access point settings, and which one it dials with.
//!
//! Privacy: a profile's password never leaves the driver's parser. Only whether one is set
//! (`has_password`) is kept. Nothing here goes into settings, events, diagnostics or speed-test
//! records, except a profile's index.
//!
//! A profile's `fingerprint` is the first 16 hex characters of SHA-256 over its name, APN,
//! username, authentication and IP type (never the password). A write checks it before and
//! after, so the app never acts on a profile that changed under it.
//!
//! Switching the default profile is a safe-apply change over `ApnTarget` (index and
//! fingerprint), with its own journal (`pending-apn-change.json`) and fixed timers
//! (`APN_TIMERS`). It has no fallback: when the old profile can't be restored the change ends in
//! `RevertFailed` and is retried every 30 s. The loss policy never applies, so a profile that
//! gives no data is always undone.
//!
//! Adding, editing and deleting (`ApnDraft`) is only for profiles that aren't in use, complete
//! and not the operator's (`check_write`, before the POST, and `check_after_write` after it). A
//! draft's password is a `Secret`: deserialised from the UI, never serialised, `Debug` as `***`.
//! "Keep the password" copies the router's own saved text inside the driver call, and is refused
//! when the router shows it empty or as stars.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

use crate::safe_apply::{
    ChangeTarget, Evidence, Phase, SafeApplyTimers, SafeApplyView, ServingLte,
};
use crate::Capabilities;

/// The refusal from a driver that can't read APN profiles.
pub const APN_NOT_SUPPORTED: &str = "reading APN profiles is not supported";
/// The refusal from a driver (or router) that can't switch the default profile.
pub const APN_SWITCH_NOT_SUPPORTED: &str = "switching the APN is not supported on this router";
/// The refusal while the router's own Auto APN picks the profile.
pub const APN_AUTO_ON: &str = "the router picks the APN by itself (Auto APN is on)";
/// The refusal when the profile isn't on the router (any more).
pub const APN_PROFILE_MISSING: &str = "that APN profile isn't on the router";
/// The refusal when the profile changed since it was chosen.
pub const APN_PROFILE_CHANGED: &str = "that APN profile changed on the router";
/// The refusal for a profile the router marks as incomplete.
pub const APN_PROFILE_INVALID: &str = "that APN profile is incomplete";
/// The refusal before the first read of the profiles.
pub const APN_PROFILES_UNKNOWN: &str = "the APN profiles haven't been read yet";
/// The refusal of a malformed target (index 0, or not a fingerprint).
pub const APN_TARGET_INVALID: &str = "that isn't an APN profile";

/// Timers of an APN switch, the same on every router: the settings' band timers don't apply.
pub const APN_TIMERS: SafeApplyTimers = SafeApplyTimers {
    grace: Duration::from_secs(90),
    loss: Duration::from_secs(60),
    probation: Duration::from_secs(120),
    poll_interval: Duration::from_secs(1),
    settle: Duration::from_secs(10),
};

/// The APN journal and its archive, next to the band journal.
pub const APN_JOURNAL_FILE: &str = "pending-apn-change.json";
pub const APN_ARCHIVE_DIR: &str = "journal-archive-apn";

/// Longest name and APN text kept from the router; the rest is cut off.
const MAX_NAME: usize = 64;
const MAX_APN: usize = 100;

/// What the driver can do with APN profiles on this router.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ApnSupport {
    /// The profile list can be read.
    pub read: bool,
    /// The default profile can be switched (with automatic undo).
    pub switch: bool,
    /// Profiles that aren't in use can be added, edited and deleted.
    pub edit: bool,
    /// The calls were seen working on this model. While false the UI says so.
    pub confirmed: bool,
}

/// How a profile authenticates (`AuthMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ApnAuth {
    /// 0
    Auto,
    /// 1
    Pap,
    /// 2
    Chap,
    Other,
}

impl ApnAuth {
    pub fn from_code(code: &str) -> Self {
        match code.trim() {
            "0" => Self::Auto,
            "1" => Self::Pap,
            "2" => Self::Chap,
            _ => Self::Other,
        }
    }
}

/// Which IP versions a profile asks for (`iptype`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ApnIpType {
    /// 0
    Ipv4,
    /// 1
    Ipv6,
    /// 2
    Ipv4v6,
    Other,
}

impl ApnIpType {
    pub fn from_code(code: &str) -> Self {
        match code.trim() {
            "0" => Self::Ipv4,
            "1" => Self::Ipv6,
            "2" => Self::Ipv4v6,
            _ => Self::Other,
        }
    }
}

/// One profile, without its password. `Debug` is safe to print.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ApnProfile {
    pub index: u32,
    pub name: String,
    /// The APN when the profile sets one; `None` means the network chooses it.
    pub apn: Option<String>,
    pub apn_static: bool,
    pub username: String,
    /// A password is saved on the router. The password itself is never read into the app.
    pub has_password: bool,
    pub auth: ApnAuth,
    pub ip_type: ApnIpType,
    /// Set by the operator or the firmware: the app never edits or deletes it.
    pub read_only: bool,
    pub valid: bool,
    /// See the module docs.
    pub fingerprint: String,
}

/// The router's profiles and which one is in use.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ApnProfiles {
    /// Index of the profile in use; `None` when the router names none.
    pub current: Option<u32>,
    pub profiles: Vec<ApnProfile>,
    /// The router picks the APN by itself (`AutoAPN`); `None` when it doesn't say.
    pub auto_apn: Option<bool>,
}

impl ApnProfiles {
    pub fn get(&self, index: u32) -> Option<&ApnProfile> {
        self.profiles.iter().find(|p| p.index == index)
    }

    /// The profile in use, as a change target.
    pub fn current_target(&self) -> Option<ApnTarget> {
        let p = self.get(self.current?)?;
        Some(ApnTarget {
            index: p.index,
            fingerprint: p.fingerprint.clone(),
        })
    }
}

/// A profile to switch to (or back to): its index and the fingerprint it had when chosen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ApnTarget {
    pub index: u32,
    pub fingerprint: String,
}

impl ChangeTarget for ApnTarget {
    /// A profile that gives no data is always undone.
    const LOSS_POLICY_APPLIES: bool = false;

    /// The router must be able to switch, and the target must be well formed. Whether the
    /// profile exists, is valid and unchanged, and whether Auto APN is on, needs the profile
    /// list: the actor checks that before a start, and the driver again before its POST.
    fn validate(&self, caps: &Capabilities) -> Result<(), String> {
        if !caps.apn.switch {
            return Err(APN_SWITCH_NOT_SUPPORTED.to_string());
        }
        if self.index == 0 || !is_fingerprint(&self.fingerprint) {
            return Err(APN_TARGET_INVALID.to_string());
        }
        Ok(())
    }

    /// In service is checked by the machine; this also needs an internet address, unless the
    /// last read doesn't know.
    fn evidence_ok(&self, obs: &Evidence) -> bool {
        obs.wan_ip_present != Some(false)
    }

    /// The cell doesn't matter to an APN.
    fn settle_key(_obs: &Evidence) -> Option<ServingLte> {
        None
    }

    fn fallback(_previous: &Self) -> Option<Self> {
        None
    }
}

/// Whether `target` may be switched to, from a fresh read of the profiles: the index is there,
/// the profile is complete, and its fingerprint is the one the user chose.
pub fn check_target(profiles: &ApnProfiles, target: &ApnTarget) -> Result<(), crate::RouterError> {
    let refuse = |m: &str| crate::RouterError::Validation {
        message: m.to_string(),
    };
    let Some(p) = profiles.get(target.index) else {
        return Err(refuse(APN_PROFILE_MISSING));
    };
    if p.fingerprint != target.fingerprint {
        return Err(refuse(APN_PROFILE_CHANGED));
    }
    if !p.valid {
        return Err(refuse(APN_PROFILE_INVALID));
    }
    Ok(())
}

/// The open APN switch, for the snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ApnApplyView {
    pub phase: Phase,
    pub seconds_left: Option<u32>,
    /// What a revert goes back to.
    pub previous: Option<ApnTarget>,
    pub requested: Option<ApnTarget>,
}

impl From<SafeApplyView<ApnTarget>> for ApnApplyView {
    fn from(v: SafeApplyView<ApnTarget>) -> Self {
        Self {
            phase: v.phase,
            seconds_left: v.seconds_left,
            previous: v.previous,
            requested: v.requested,
        }
    }
}

/// See the module docs. The raw codes go in, so an unknown code still counts.
pub fn fingerprint(name: &str, apn: &str, username: &str, auth: &str, ip_type: &str) -> String {
    let mut h = Sha256::new();
    for part in [name, apn, username, auth, ip_type] {
        h.update(part.as_bytes());
        h.update([0u8]);
    }
    let digest = h.finalize();
    hex::encode(&digest[..8])
}

/// A 16-character lower-case hex fingerprint.
pub fn is_fingerprint(s: &str) -> bool {
    s.len() == 16
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Router text bound for the screen: control characters dropped, trimmed, at most `max` chars.
pub fn clean_text(raw: &str, max: usize) -> String {
    raw.trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect()
}

/// The values of one `<Profile>` element, by tag name. The password is never stored here.
#[derive(Default)]
pub struct RawProfile {
    pub index: String,
    pub is_valid: String,
    pub name: String,
    pub apn_is_static: String,
    pub apn_name: String,
    pub username: String,
    pub has_password: bool,
    pub auth_mode: String,
    pub read_only: String,
    pub ip_type: String,
}

impl RawProfile {
    /// `None` without a usable index.
    pub fn build(self) -> Option<ApnProfile> {
        let index: u32 = self.index.trim().parse().ok()?;
        let name = clean_text(&self.name, MAX_NAME);
        let apn_text = clean_text(&self.apn_name, MAX_APN);
        let username = clean_text(&self.username, MAX_NAME);
        let apn_static = self.apn_is_static.trim() == "1";
        let fingerprint = fingerprint(
            &name,
            &apn_text,
            &username,
            self.auth_mode.trim(),
            self.ip_type.trim(),
        );
        Some(ApnProfile {
            index,
            name,
            apn: (apn_static && !apn_text.is_empty()).then_some(apn_text),
            apn_static,
            username,
            has_password: self.has_password,
            auth: ApnAuth::from_code(&self.auth_mode),
            ip_type: ApnIpType::from_code(&self.ip_type),
            // Anything but an explicit 0 is treated as the operator's: never edited.
            read_only: self.read_only.trim() != "0",
            valid: self.is_valid.trim() == "1",
            fingerprint,
        })
    }
}

// --- Adding, editing and deleting profiles that aren't in use -------------------------------

/// The refusal from a driver (or router) that can't add, edit or delete profiles.
pub const APN_EDIT_NOT_SUPPORTED: &str = "editing APN profiles is not supported on this router";
/// The refusal when the profile in use isn't the one the app read last.
pub const APN_CURRENT_MOVED: &str = "the APN in use changed on the router";
/// After a write, the router names another profile in use than before.
pub const APN_ROUTER_SWITCHED: &str = "the router changed the active APN";
/// The refusal to edit or delete the profile the router dials with.
pub const APN_PROFILE_IN_USE: &str = "that APN profile is in use";
/// The refusal to edit or delete a profile set by the operator or the firmware.
pub const APN_PROFILE_READ_ONLY: &str = "that APN profile is set by the operator";
/// The router refused a plain request; it may want an encrypted one, which the app can't send.
pub const APN_NEEDS_ENCRYPTION: &str =
    "the router refused the request; it may need an encrypted request, which the app can't send";
/// After a write, the profile on the router isn't the one that was sent.
pub const APN_SAVE_MISMATCH: &str = "the router didn't save the profile as sent";
/// "Keep the password" while the router shows it empty or as stars.
pub const APN_PASSWORD_HIDDEN: &str =
    "the router hides this profile's saved password, so it can't be kept";
/// "Keep the password" on a profile that doesn't exist yet.
pub const APN_NOTHING_TO_KEEP: &str = "a new profile has no saved password to keep";
/// Draft refusals; the dialog checks the same rules first.
pub const APN_NAME_INVALID: &str = "the profile name must be 1 to 64 characters";
pub const APN_APN_INVALID: &str =
    "the APN may hold only letters, digits, dots and dashes, up to 100";
pub const APN_USERNAME_INVALID: &str =
    "the user name must be at most 64 characters, without control characters";
pub const APN_PASSWORD_INVALID: &str =
    "the password must be 1 to 64 characters, without control characters";
pub const APN_KIND_INVALID: &str = "that sign-in method or IP type can't be saved";

/// Longest user name and password the app sends.
const MAX_SECRET: usize = 64;

/// A password the user typed, on its way to the router. Deserialize only: it is never
/// serialised, has no `Display`, and `Debug` prints `***`.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// The text itself, for the request body only.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("***")
    }
}

/// What happens to a profile's saved password.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum PasswordChange {
    /// The router's saved password stays (edit only).
    Keep,
    /// No password.
    Clear,
    Set {
        #[ts(type = "string")]
        value: Secret,
    },
}

/// A profile to add, or what an edit makes of one. Deserialize only, since it may carry a
/// password; `Debug` hides it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ApnDraft {
    pub name: String,
    /// Empty: the network chooses the APN.
    pub apn: String,
    pub username: String,
    pub password: PasswordChange,
    pub auth: ApnAuth,
    pub ip_type: ApnIpType,
}

impl ApnAuth {
    /// The router's code; `None` for `Other`, which is never written.
    pub fn code(self) -> Option<&'static str> {
        match self {
            Self::Auto => Some("0"),
            Self::Pap => Some("1"),
            Self::Chap => Some("2"),
            Self::Other => None,
        }
    }
}

impl ApnIpType {
    /// The router's code; `None` for `Other`, which is never written.
    pub fn code(self) -> Option<&'static str> {
        match self {
            Self::Ipv4 => Some("0"),
            Self::Ipv6 => Some("1"),
            Self::Ipv4v6 => Some("2"),
            Self::Other => None,
        }
    }
}

fn refuse(m: &str) -> crate::RouterError {
    crate::RouterError::Validation {
        message: m.to_string(),
    }
}

fn apn_char_ok(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'.' || b == b'-'
}

impl ApnDraft {
    /// The draft with its texts trimmed, or why it can't be sent. `is_new`: an add, where there
    /// is no saved password to keep.
    pub fn checked(&self, is_new: bool) -> Result<ApnDraft, crate::RouterError> {
        let name = self.name.trim();
        let n = name.chars().count();
        if n == 0 || n > MAX_NAME || name.chars().any(char::is_control) {
            return Err(refuse(APN_NAME_INVALID));
        }
        let apn = self.apn.trim();
        if apn.len() > MAX_APN || !apn.bytes().all(apn_char_ok) {
            return Err(refuse(APN_APN_INVALID));
        }
        let username = self.username.trim();
        if username.chars().count() > MAX_SECRET || username.chars().any(char::is_control) {
            return Err(refuse(APN_USERNAME_INVALID));
        }
        match &self.password {
            PasswordChange::Keep if is_new => return Err(refuse(APN_NOTHING_TO_KEEP)),
            PasswordChange::Set { value } => {
                let p = value.expose();
                if p.is_empty() || p.chars().count() > MAX_SECRET || p.chars().any(char::is_control)
                {
                    return Err(refuse(APN_PASSWORD_INVALID));
                }
            }
            _ => {}
        }
        if self.auth.code().is_none() || self.ip_type.code().is_none() {
            return Err(refuse(APN_KIND_INVALID));
        }
        Ok(ApnDraft {
            name: name.to_string(),
            apn: apn.to_string(),
            username: username.to_string(),
            password: self.password.clone(),
            auth: self.auth,
            ip_type: self.ip_type,
        })
    }

    /// The fingerprint the router's profile has once this (checked) draft is saved.
    pub fn fingerprint(&self) -> String {
        fingerprint(
            &self.name,
            &self.apn,
            &self.username,
            self.auth.code().unwrap_or(""),
            self.ip_type.code().unwrap_or(""),
        )
    }

    /// The password to send. `saved` is the router's own `Password` text of the profile, read
    /// inside the same driver call; only `Keep` uses it, and it never leaves the call.
    pub fn password_text(&self, saved: Option<&str>) -> Result<String, crate::RouterError> {
        match &self.password {
            PasswordChange::Clear => Ok(String::new()),
            PasswordChange::Set { value } => Ok(value.expose().to_string()),
            PasswordChange::Keep => {
                let Some(saved) = saved else {
                    return Err(refuse(APN_NOTHING_TO_KEEP));
                };
                let t = saved.trim();
                if t.is_empty() || t.chars().all(|c| c == '*') {
                    return Err(refuse(APN_PASSWORD_HIDDEN));
                }
                Ok(saved.to_string())
            }
        }
    }

    /// The profile tags this draft sets, in the router's order, with `password` from
    /// `password_text`. Values are raw; the request builder escapes them.
    pub fn tags(&self, password: &str) -> Vec<(&'static str, String)> {
        let is_static = if self.apn.is_empty() { "0" } else { "1" };
        vec![
            ("Name", self.name.clone()),
            ("ApnIsStatic", is_static.to_string()),
            ("ApnName", self.apn.clone()),
            ("Username", self.username.clone()),
            ("Password", password.to_string()),
            ("AuthMode", self.auth.code().unwrap_or("0").to_string()),
            ("iptype", self.ip_type.code().unwrap_or("2").to_string()),
        ]
    }
}

/// Whether an add, edit or delete may go ahead, from a fresh read: the profile in use is still
/// `expect_current`, and the profile written to (`target`, none for an add) is there, unchanged,
/// complete, not in use and not the operator's.
pub fn check_write(
    profiles: &ApnProfiles,
    expect_current: &ApnTarget,
    target: Option<&ApnTarget>,
) -> Result<(), crate::RouterError> {
    if profiles.current_target().as_ref() != Some(expect_current) {
        return Err(refuse(APN_CURRENT_MOVED));
    }
    let Some(target) = target else {
        return Ok(());
    };
    let Some(p) = profiles.get(target.index) else {
        return Err(refuse(APN_PROFILE_MISSING));
    };
    if p.index == expect_current.index {
        return Err(refuse(APN_PROFILE_IN_USE));
    }
    if p.read_only {
        return Err(refuse(APN_PROFILE_READ_ONLY));
    }
    if p.fingerprint != target.fingerprint {
        return Err(refuse(APN_PROFILE_CHANGED));
    }
    if !p.valid {
        return Err(refuse(APN_PROFILE_INVALID));
    }
    Ok(())
}

/// After a write: the router still dials with the profile it had before.
pub fn check_after_write(
    profiles: &ApnProfiles,
    expect_current: &ApnTarget,
) -> Result<(), crate::RouterError> {
    if profiles.current != Some(expect_current.index) {
        return Err(refuse(APN_ROUTER_SWITCHED));
    }
    Ok(())
}

/// After an edit: the profile now has the draft's fingerprint.
pub fn check_edited(
    after: &ApnProfiles,
    index: u32,
    draft: &ApnDraft,
) -> Result<(), crate::RouterError> {
    match after.get(index) {
        Some(p) if p.fingerprint == draft.fingerprint() => Ok(()),
        _ => Err(refuse(APN_SAVE_MISMATCH)),
    }
}

/// After a delete: the profile is gone.
pub fn check_deleted(after: &ApnProfiles, index: u32) -> Result<(), crate::RouterError> {
    match after.get(index) {
        None => Ok(()),
        Some(_) => Err(refuse(APN_SAVE_MISMATCH)),
    }
}

/// The index of the profile an add created: one that wasn't there `before`, with the draft's
/// fingerprint.
pub fn added_index(
    before: &ApnProfiles,
    after: &ApnProfiles,
    draft: &ApnDraft,
) -> Result<u32, crate::RouterError> {
    let fp = draft.fingerprint();
    after
        .profiles
        .iter()
        .find(|p| before.get(p.index).is_none() && p.fingerprint == fp)
        .map(|p| p.index)
        .ok_or_else(|| refuse(APN_SAVE_MISMATCH))
}
