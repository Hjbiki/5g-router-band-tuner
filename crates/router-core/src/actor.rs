//! RouterActor (§6): the single owner of the router driver.
//!
//! One tokio task runs a `select!` over the command channel and a single timer. The timer fires
//! at the earliest of:
//! - the next *attempt*: a poll when logged in, a login otherwise;
//! - a `RetryAfter` deadline requested by `SafeApply`;
//! - the 1 s safe-apply tick, only while a change is active.
//!
//! Every wake sends `Input::Tick` to `SafeApply` first. A poll runs `signal` + `status`, then
//! `net-mode` (every 10 s) and `heartbeat` (every 5 s), strictly one request at a time. A
//! successful signal+status poll becomes `Input::Observation`; every failed poll (whatever the
//! error) and every failed login becomes `Input::SessionUnavailable`, so a commit is never
//! decided without evidence of service. While `capabilities` are unknown they are fetched again
//! after a successful poll, at most every 10 s.
//!
//! Cadence: 1 s visible, 5 s hidden; after a failed poll the interval doubles up to 30 s and
//! resets after one success. Session loss triggers re-login after 2, 5, 10, then every 30 s
//! (every 5 s while a change is active). A Huawei "no rights" API error (100003) counts as a
//! session loss. `BadCredentials`, `LoginRejected`, `LockedOut`, `LoginAlgorithmMismatch` and
//! `UnrecognizedRouter` stop retrying until the next `Connect`. A login refusal that rejects
//! the password writes `password-rejected` first (see `password_marker`).
//!
//! Effects from `SafeApply` run in order. A driver effect's result is fed straight back and the
//! effects it produces run before the remaining ones. While the router is known to be offline
//! (logged out, or the last poll failed), `Post` and `ReadConfig` are not sent: they're answered
//! with `SessionUnavailable`, which parks a due revert as `RevertPending` until the next
//! successful poll instead of burning revert attempts on a dead link.
//!
//! The actor never calls `apply_net_config` except for an `Effect::Post`, and never calls
//! `reboot` except for a valid `ConfirmReboot`.
//!
//! Automation (`ActorConfig::automation`, live via `SetAutomation`):
//! - Loss policy: passed to `SafeApply::set_revert_on_loss`.
//! - Primary band P: an Apply whose LTE set holds P and other bands runs as two safe-applies.
//!   Step 1 is the same config with P alone; when it commits, step 2 applies the full config and
//!   reverts, if it must, to the config from before step 1. Step 1 failing ends the sequence. A
//!   step 2 that fails is reported (`AutomationEvent::WidenFailed`) with the config it ended on.
//! - Re-lock watchdog (P set and `relock_primary`): when no change is open, the allowed set
//!   holds P and other bands, and the serving LTE band has been another band for
//!   `RELOCK_AFTER`, it runs the same two steps with the current allowed set as step 2. At most
//!   once per `RELOCK_EVERY`; after `RELOCK_MAX_FAILURES` failed attempts in a row it stops and
//!   alerts until the automation settings change or P serves again for `RELOCK_RESET_AFTER`.
//!   An attempt fails when a step doesn't commit or the router isn't on P afterwards. It never
//!   runs with a foreign or unreadable journal, or while `band-runner.lock` exists. Serving P
//!   only counts toward the reset while no change or sequence is open, so the watchdog's own
//!   step 1 never clears its failure count.
//! - Persistence: a sequence is kept in `pending-sequence.json` (see `sequence_file`) from its
//!   start until it ends. At launch a step-1 journal that resumes brings the sequence back, so
//!   step 2 follows its commit; with no journal open, a sequence whose step 1 committed (or
//!   whose step 2 was due) runs step 2 once connected. Either way `SequenceResumed` is raised.
//!   A sequence file that is corrupt, for another host or driver, or doesn't match the journal
//!   is moved aside and reported (`SequenceDiscarded`); it never causes a band change.
//! - Speed test (`SetSpeedTestActive`): while a test runs, a user Apply is refused with
//!   `SPEED_TEST_RUNNING`, and step 2 and the watchdog wait. The flag is ignored after
//!   `SPEED_TEST_CAP`, so a test that never reports its end can't block automation for good.
//!
//! Refresh IP (`StartIpRefresh`, see `ip_refresh`): refused while anything else holds the router,
//! when not logged in or not in service, when the driver has no method left (its
//! `Capabilities::ip_methods`, minus `data_switch_write` and `network_search` once the router
//! refused them this session), or when the hourly budget is spent. While a run is open, Apply is
//! refused with `IP_REFRESH_RUNNING`, a reboot, factory reset or logout is refused, and step 2 and
//! the re-lock watchdog wait. The run ticks every second; its "off" and "on" POSTs are direct
//! calls, never preempted, and an effect the actor can't send (not logged in) is answered with
//! `Deferred`. A refused "off" puts `data_switch_write` in `refused` (`feature.refused`) instead of
//! counting as a lost session. `pending-data-switch.json` is on disk before every "off". At launch
//! it is loaded before the band journal: an own record is resumed (data is turned on once logged
//! in), anything else is shown and never acted on. A switch that reads 0 at login with no record
//! only raises `data_switch_off`; `TurnDataOn` is the user's button for it. At shutdown with data
//! off, one "on" is sent (at most 5 s).
//!
//! A network search (H122's method) runs in a task of its own, so the loop keeps answering
//! commands and snapshots while the router takes minutes: its result comes back on a channel of
//! `run`, and the task is aborted at shutdown (there is nothing to undo; the router reconnects by
//! itself). The session is refreshed first when the last refresh is over 60 s old. While it runs,
//! polls slow to every 3 s, skip the WAN, APN and net-mode reads, and a timeout raises no offline
//! notice; a poll without service marks the search `service_dropped`. A refusal within 5 s puts
//! `network_search` in `refused`. After a run that searched, `net-mode` is read once: a config
//! that differs from the one at the start is logged (`ip_refresh.bands_changed`) and kept in the
//! result for the UI to offer back. Nothing is changed by itself.
//!
//! New IP by a band switch (`StartIpReattach`, a button the UI offers after a network search
//! that kept the range; never automatic): a two-step sequence (`SequenceOrigin::IpReattach`)
//! through the same safe apply, journal and `pending-sequence.json` as a primary-band apply.
//! Step 1 is `reattach_step1`: the serving band alone when the allowed set has two or more bands
//! (or is Auto), otherwise every supported band, so the band in use is never lost; step 2 is the
//! config from before. Step 1 is kept early, once it re-attached and a WAN read after that shows
//! an address (the read runs every 2 s then); step 2 has the full probation and reverts to step
//! 1's bands, which worked. Refused like Refresh IP while anything holds the router, and also
//! without a known config, when band editing isn't allowed, on a mode without LTE, and after
//! `REATTACH_PER_HOUR` starts in the last hour. The first WAN range read within
//! `REATTACH_TO_WAIT` after the sequence ends goes into `ip_reattach.ended` and the alert.
//!
//! Log out (`Logout`): refused while anything holds the router (`busy_refusal`). It writes the
//! `logged-out` marker first, asks the router to end the session (at most `LOGOUT_TIMEOUT`), then
//! forgets the driver's cookies and tokens and every value read in the session. It always ends in
//! `LoggedOut`, even when the router didn't answer; the saved password and the router identity
//! stay. No call reaches the router until the next `Connect`. A launch `ConnectAutomatic` with the
//! marker set stays `LoggedOut`, unless a band change, sequence or foreign journal needs watching.
//!
//! APN profiles: read right after a login, every 60 s while the window is shown, and on
//! `ApnRefresh`, only when the driver says it can (`Capabilities::apn.read`). Never in the 1 s
//! poll. The list (without passwords) is in the snapshot and never in the event log.
//!
//! APN switch (`ApnSwitch`, see `apn`): a second safe-apply over `ApnTarget`, with its own journal
//! (`pending-apn-change.json`, loaded at launch after the band journal) and the fixed
//! `APN_TIMERS`. One change at a time: while it is open, Apply, a sequence's step 2, the re-lock
//! watchdog, logout, Refresh IP, a reboot and a factory reset are refused or wait
//! (`APN_CHANGE_RUNNING`), and it is refused while any of those is open. It needs the profile list:
//! the target must be there, complete and unchanged, and Auto APN off. Keep and Revert go to
//! whichever change is open. Evidence of service also needs an internet address, read every 5 s
//! while the switch is open. A foreign or unreadable APN journal blocks APN switches only. Events
//! hold the index, phase and reason, never a name or an APN.
//!
//! APN add, edit and delete (`ApnAdd`, `ApnEdit`, `ApnDelete`): only on profiles that aren't in
//! use, complete and not the operator's, refused like a switch while anything else holds the
//! router (and while an APN record that can't be used is on file). The profile in use from the
//! last read goes to the driver, which reads again before and after its POST. The list is read
//! once more afterwards; when the router now dials with another profile, an APN switch back to
//! the one from before starts by itself (`apn.unexpected_switch`, `Alert::ApnUnexpectedSwitch`).
//! Events hold the index only; a draft's password goes to the driver and nowhere else.

use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::time::{sleep_until, Instant};
use ts_rs::TS;

use crate::apn::{
    check_target, ApnApplyView, ApnDraft, ApnProfiles, ApnTarget, APN_ARCHIVE_DIR, APN_AUTO_ON,
    APN_EDIT_NOT_SUPPORTED, APN_JOURNAL_FILE, APN_PROFILES_UNKNOWN, APN_ROUTER_SWITCHED,
    APN_SWITCH_NOT_SUPPORTED, APN_TIMERS,
};
use crate::driver::FACTORY_RESET_NOT_VERIFIED;
use crate::event_text::kinds;
use crate::ip_refresh::{
    IpEffect, IpInput, IpRefresh, IpRefreshEvent, IpRefreshOutcome, IpTarget, IP_NO_METHOD,
    SEARCH_CALL_LIMIT,
};
pub use crate::ip_refresh::{IpRefreshResult, IpRefreshView};
use crate::ip_refresh_file::{
    DataSwitchPhase, DataSwitchRecord, DataSwitchStore, DATA_SWITCH_VERSION,
};
use crate::journal::{Journal, JournalPhase, JournalRecord};
use crate::safe_apply::{ApplySequenceView, SequenceOrigin};
use crate::sequence_file::{SequenceRecord, SequenceStore, SEQUENCE_VERSION};
use crate::NetworkMode;
use crate::{
    in_service, ApiErrorKind, Capabilities, Credentials, DeviceInfo, Effect, EventLog, EventRecord,
    ForeignJournal, Input, IpMethod, LoginGate, LteBandSet, NetConfig, NetworkSearch, Phase, Rat,
    RevertReason, RouterDriver, RouterError, RouterIdentity, SafeApply, SafeApplyEvent,
    SafeApplyTimers, SafeApplyView, SessionState, Signal, Status, WanAddress, WanView,
};

const VISIBLE_POLL: Duration = Duration::from_secs(1);
const HIDDEN_POLL: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const NET_CONFIG_EVERY: Duration = Duration::from_secs(10);
const HEARTBEAT_EVERY: Duration = Duration::from_secs(5);
/// The WAN address is read right after a login and then this often.
const WAN_EVERY: Duration = Duration::from_secs(30);
/// The APN profiles are read right after a login, on `ApnRefresh`, and this often while the
/// window is shown. Never in the 1 s poll.
const APN_EVERY: Duration = Duration::from_secs(60);
/// While an APN switch is open, the WAN address (its evidence of data) is read this often.
const APN_WAN_EVERY: Duration = Duration::from_secs(5);
/// The H122 drops a session 300 s after the last connect, even while polls and heartbeats
/// run; repeating the connect step's reads restarted that clock (observed 2026-10-29).
/// Refreshed silently a little earlier, so the user never sees a reconnect.
const SESSION_REFRESH_AFTER: Duration = Duration::from_secs(270);
const CAPABILITIES_RETRY: Duration = Duration::from_secs(10);
const SAFE_TICK: Duration = Duration::from_secs(1);
const RELOGIN_DELAYS: [Duration; 4] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
];
const ACTIVE_RELOGIN: Duration = Duration::from_secs(5);
/// A session that lasted this long counts as working: losing it starts the re-login delays
/// over. A shorter one keeps climbing them, so a session that dies right after each login
/// can't turn into a login every 2 s.
const STABLE_SESSION: Duration = Duration::from_secs(60);
/// Names of the optional reads in `Actor::refused` and the `feature.refused` event.
const APN: &str = "apn";
const WAN: &str = "wan";
const DATA_SWITCH: &str = "data_switch";
/// The router refused the data switch's "off": Refresh IP doesn't use it again this session.
const DATA_SWITCH_WRITE: &str = "data_switch_write";
/// The router refused a network search at once.
const NETWORK_SEARCH: &str = "network_search";
/// While a network search runs, the signal and status are polled this often at most.
const SEARCH_POLL: Duration = Duration::from_secs(3);
/// A search starts with a session refresh when the last one is older than this, so the
/// router's 300 s window covers the whole search.
const SEARCH_REFRESH_AFTER: Duration = Duration::from_secs(60);
const REBOOT_TICKET_TTL: Duration = Duration::from_secs(30);
const REBOOT_RECONNECT_AFTER: Duration = Duration::from_secs(60);
/// The serving LTE band must be off the primary band this long before a re-lock.
const RELOCK_AFTER: Duration = Duration::from_secs(120);
/// At most one re-lock attempt per this interval.
const RELOCK_EVERY: Duration = Duration::from_secs(30 * 60);
const RELOCK_MAX_FAILURES: u8 = 3;
/// Serving the primary band this long clears the failure count (and a given-up watchdog).
const RELOCK_RESET_AFTER: Duration = Duration::from_secs(10 * 60);
/// Written by `band-runner` next to the journal while it runs.
const BAND_RUNNER_LOCK: &str = "band-runner.lock";
/// SafeApply's refusal when the snapshot already equals the request.
const ALREADY_HAS: &str = "the router already has the requested configuration";
/// The `Rejected` message for a user Apply while a speed test runs.
pub const SPEED_TEST_RUNNING: &str = "a speed test is running";
/// The refusal while a band change, a primary-band sequence or a scan is open. The UI maps
/// this exact text (`ui/src/lib/errors.ts`).
pub const CHANGE_IN_PROGRESS: &str = "a band change is in progress";
/// The refusal while an IP refresh runs. The UI maps this exact text.
pub const IP_REFRESH_RUNNING: &str = "an IP refresh is running";
/// The refusal while the router's page scripts are being read. The UI maps this exact text.
pub const PAGE_SCRIPTS_RUNNING: &str = "the router's page scripts are being read";
/// The refusal while an APN switch is open. The UI maps this exact text.
pub const APN_CHANGE_RUNNING: &str = "an APN change is in progress";
/// The refusal of an APN switch while an APN record that can't be used is on file.
pub const APN_JOURNAL_BLOCKED: &str = "an APN change record from before can't be used";
/// The refusal of an IP refresh while the router has no service.
pub const IP_NOT_IN_SERVICE: &str = "the router has no service right now";
/// The refusal of an IP refresh while a reboot waits for its confirmation.
pub const REBOOT_PENDING: &str = "a reboot is waiting for confirmation";
/// The refusal of an IP refresh while a data-switch record that can't be used is on file.
pub const DATA_JOURNAL_BLOCKED: &str = "a mobile data record from before can't be used";
/// `TurnDataOn` posted "on", but the switch still reads off.
pub const DATA_STILL_OFF: &str = "mobile data is still off";
/// The refusal of an APN switch while mobile data is off on the router (not by this app).
pub const DATA_SWITCH_OFF: &str = "mobile data is off on the router";
/// The refusal of anything that needs the router while not logged in.
pub const NOT_CONNECTED: &str = "not connected to the router";
/// The refusal of a band switch for a new IP after `REATTACH_PER_HOUR` in the last hour.
pub const IP_REATTACH_COOLDOWN: &str = "too many band switches for a new IP in the last hour";
/// The refusal of a band switch for a new IP on a network mode without 4G.
pub const IP_REATTACH_NO_LTE: &str = "the network mode has no 4G bands to switch";
/// The refusal of a band switch for a new IP when no other band set is possible.
pub const IP_REATTACH_NO_OTHER_BANDS: &str = "there are no other bands to switch to for a moment";
/// The refusal of a band switch for a new IP while automatic undo (`revert_on_loss`) is off:
/// without it a step that finds no service would be kept. The UI maps this exact text.
pub const IP_REATTACH_UNDO_OFF: &str = "automatic undo is off";
/// The refusal of a band change before the router's config and capabilities were read.
pub const BANDS_NOT_READ: &str = "the router's capabilities aren't known yet";
/// The refusal of a band change on a router that doesn't allow band editing right now.
pub const BAND_EDIT_NOT_ALLOWED: &str = "band editing isn't allowed on this router right now";
/// At most this many band switches for a new IP in a rolling hour.
const REATTACH_PER_HOUR: usize = 3;
const HOUR: Duration = Duration::from_secs(3600);
/// While step 1 waits for its early keep, and after the sequence ends, the WAN address is
/// read this often.
const REATTACH_WAN_EVERY: Duration = Duration::from_secs(2);
/// After the sequence ends, the new range is the first one read within this long.
const REATTACH_TO_WAIT: Duration = Duration::from_secs(60);
/// At shutdown with data off, one "on" gets this long.
const EXIT_DATA_ON_TIMEOUT: Duration = Duration::from_secs(5);
const IP_TICK: Duration = Duration::from_secs(1);
/// A speed test that hasn't reported its end after this long no longer holds anything back.
const SPEED_TEST_CAP: Duration = Duration::from_secs(10 * 60);
/// The page-script read is limited to 180 s; past this its flag is ignored, so a read that never
/// reports its end can't block the router for good.
const PAGE_SCRIPTS_CAP: Duration = Duration::from_secs(240);
/// The router gets this long to answer a logout; the app logs out locally either way.
const LOGOUT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct ActorConfig {
    pub host: String,
    pub timers: SafeApplyTimers,
    pub data_dir: PathBuf,
    pub cell_alerts: bool,
    pub automation: AutomationConfig,
}

/// What the actor may do by itself (`settings.automation`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutomationConfig {
    /// The starred LTE band.
    pub primary_band: Option<u8>,
    /// Re-lock to the primary band when the router drifts off it.
    pub relock_primary: bool,
    /// Revert a change that loses service (the default).
    pub revert_on_loss: bool,
}

impl Default for AutomationConfig {
    fn default() -> Self {
        Self {
            primary_band: None,
            relock_primary: false,
            revert_on_loss: true,
        }
    }
}

#[derive(Debug)]
pub enum Command {
    /// The user pressed Connect: a first login goes ahead, a changed router asks.
    Connect(Credentials),
    /// The app is connecting by itself: only to the router recorded before; otherwise asks.
    ConnectAutomatic(Credentials),
    /// The user said this is their router: logs in, and records what it finds.
    ConnectTrusting(Credentials),
    Apply(NetConfig),
    KeepNow,
    RevertNow,
    /// The auto-scan stops waiting for a band that gives no service.
    GiveUp,
    RequestReboot(oneshot::Sender<Result<RebootTicket, RouterError>>),
    ConfirmReboot(String, oneshot::Sender<Result<(), RouterError>>),
    RequestFactoryReset(oneshot::Sender<Result<RebootTicket, RouterError>>),
    ConfirmFactoryReset(String, oneshot::Sender<Result<(), RouterError>>),
    /// The user pressed Log out.
    Logout(oneshot::Sender<Result<LogoutOutcome, RouterError>>),
    SetVisible(bool),
    UpdateTimers(SafeApplyTimers),
    SetCellAlerts(bool),
    /// New automation settings; they apply at once.
    SetAutomation(AutomationConfig),
    /// A speed test started (`true`) or ended (`false`).
    SetSpeedTestActive(bool),
    /// An auto-scan is driving band changes: no two-step applies, no re-lock, no reboot.
    SetScanActive(bool),
    /// The app is about to read the router's page scripts (many GETs on this session). Refused
    /// while anything holds the router; once granted, the user's changes, Refresh IP, APN
    /// changes, reboots and logouts are refused with `PAGE_SCRIPTS_RUNNING`, and step 2 and the
    /// re-lock watchdog wait, until `EndPageScripts` (or `PAGE_SCRIPTS_CAP`).
    BeginPageScripts(oneshot::Sender<Result<(), RouterError>>),
    EndPageScripts,
    /// Refresh IP: turn mobile data off and on, until the range starts with `target` (one or
    /// two numbers) or `max_tries` (1 to 5) is used up. Without a target, one try.
    StartIpRefresh {
        target: Option<String>,
        max_tries: u8,
        reply: oneshot::Sender<Result<(), RouterError>>,
    },
    /// Stops a Refresh IP run; data is turned back on first when it is off.
    StopIpRefresh,
    /// The user's "Turn mobile data on" button.
    TurnDataOn(oneshot::Sender<Result<(), RouterError>>),
    /// The user's "Get a new IP by switching bands": a two-step band sequence through safe
    /// apply. The reply says whether it started.
    StartIpReattach(oneshot::Sender<Result<(), RouterError>>),
    /// Read the APN profiles again now.
    ApnRefresh,
    /// Switch the APN profile in use, through safe apply. The reply says whether it started.
    ApnSwitch {
        target: ApnTarget,
        reply: oneshot::Sender<Result<(), RouterError>>,
    },
    /// Add a profile; the reply holds its index. `Debug` hides the draft's password.
    ApnAdd {
        draft: ApnDraft,
        reply: oneshot::Sender<Result<u32, RouterError>>,
    },
    /// Rewrite a profile that isn't in use; `target` is the one the user saw.
    ApnEdit {
        target: ApnTarget,
        draft: ApnDraft,
        reply: oneshot::Sender<Result<(), RouterError>>,
    },
    /// Delete a profile that isn't in use; `target` is the one the user saw.
    ApnDelete {
        target: ApnTarget,
        reply: oneshot::Sender<Result<(), RouterError>>,
    },
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RebootTicket {
    pub id: String,
    pub expires_in_s: u32,
}

/// How a logout went on the router's side. The app is logged out either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LogoutOutcome {
    /// The router confirmed the session ended. `false` when it refused or didn't answer, and
    /// its session then simply expires.
    pub confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum SessionView {
    NotConfigured,
    Connecting,
    LoggedIn,
    #[serde(rename_all = "camelCase")]
    Reconnecting {
        reason: String,
        retry_in_s: u32,
    },
    TakenOver,
    BadCredentials,
    #[serde(rename_all = "camelCase")]
    LockedOut {
        wait_s: u32,
    },
    AlgorithmMismatch,
    /// The router refused the login with a code the app doesn't recognise; not retried.
    LoginRejected {
        code: u32,
    },
    /// Stopped before sending a login proof: the user has to say this is their router.
    #[serde(rename_all = "camelCase")]
    ConfirmRouter {
        first_time: bool,
    },
    /// The user logged out; nothing is sent to the router until Connect.
    LoggedOut,
}

/// Who asked for the login being attempted, which decides how a router that isn't recorded is
/// treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginTrust {
    Automatic,
    UserAsked,
    UserTrusted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppSnapshot {
    pub session: SessionView,
    /// Fetched once per login.
    pub device: Option<DeviceInfo>,
    pub status: Option<Status>,
    pub signal: Option<Signal>,
    pub config: Option<NetConfig>,
    pub capabilities: Option<Capabilities>,
    pub safe_apply: SafeApplyView,
    pub in_service: bool,
    /// A journal for another host or driver: shown, never acted on.
    pub foreign_journal: Option<JournalRecord>,
    #[ts(type = "number")]
    pub captured_at_ms: u64,
    /// The re-lock watchdog, when a primary band is set and re-lock is on.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub relock: Option<RelockView>,
    /// The router's internet address, from the last read. The full address is in this live
    /// view only; nothing stores more than its range.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub wan: Option<WanView>,
    /// Refresh IP: a run, the last result, or a data-switch record that can't be used.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub ip_refresh: Option<IpRefreshView>,
    /// The switch read off at login with no record of the app turning it off. Only a notice:
    /// the app never turns it on by itself.
    #[serde(default)]
    pub data_switch_off: bool,
    /// The APN profiles from the last read; passwords are never part of it.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub apn: Option<ApnProfiles>,
    /// An APN switch in progress, or how the last one ended. `None` when there has been none.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub apn_apply: Option<ApnApplyView>,
    /// A band switch for a new IP in progress, or how the last one ended.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub ip_reattach: Option<IpReattachView>,
}

/// How a band switch for a new IP ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum IpReattachOutcome {
    /// The bands are back and the range is new.
    Changed,
    /// The bands are back, with the same range.
    Unchanged,
    /// The bands are back; the range before or after isn't known.
    Unknown,
    /// Step 1 didn't commit; safe apply undid it.
    Step1Failed,
    /// Step 2 didn't commit; the router stays on step 1's bands (or Auto).
    Step2Failed,
    /// The user pressed Revert.
    Reverted,
    /// Another change replaced it, or the bands changed between the steps.
    Cancelled,
}

/// Ranges only ("a.b"), never an address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IpReattachResult {
    pub outcome: IpReattachOutcome,
    pub from: Option<String>,
    pub to: Option<String>,
    /// The bands from before, when the router may not have ended on them; `None` when it did.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub restore: Option<NetConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IpReattachView {
    /// The sequence is open (its steps show in the safe-apply banner).
    pub active: bool,
    /// The range when it started; `None` after a restart.
    pub from_range: Option<String>,
    pub last: Option<IpReattachResult>,
}

/// The rolling-hour budget and the end of the current band switch for a new IP.
#[derive(Default)]
struct Reattach {
    starts: VecDeque<Instant>,
    from: Option<String>,
    /// Step 1 re-attached then (safe apply's `Reattached`).
    reattached_at: Option<Instant>,
    /// The sequence ended; the result waits for the first WAN read.
    finishing: Option<Finishing>,
    last: Option<IpReattachResult>,
}

struct Finishing {
    /// `None` for a sequence that ended with the bands back: decided by the ranges.
    outcome: Option<IpReattachOutcome>,
    from: Option<String>,
    restore: Option<NetConfig>,
    ended_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RelockView {
    pub primary: u8,
    /// Failed attempts in a row.
    pub failures: u8,
    /// Stopped after too many failures, until the settings change or the primary band serves
    /// again for 10 minutes.
    pub given_up: bool,
    /// Seconds until the rate limit allows the next attempt; `None` when it already does.
    pub next_try_in_s: Option<u32>,
}

/// How a failed step 2 ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum WidenFallback {
    /// Back on the config from before step 1.
    Original,
    /// Still on the primary band alone (step 1's config), which had service.
    PrimaryOnly,
    /// The revert couldn't restore the bands and fell back to Auto.
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum AutomationEvent {
    /// Auto-revert is off: service was lost (or never came back) and the change was kept.
    LossIgnored { reason: RevertReason },
    /// Step 1 (primary band alone) didn't commit; the sequence stopped.
    PrimaryLockFailed { primary: u8, origin: SequenceOrigin },
    /// Step 2 (the full set) didn't commit.
    #[serde(rename_all = "camelCase")]
    WidenFailed {
        primary: u8,
        origin: SequenceOrigin,
        ended_on: WidenFallback,
    },
    /// The watchdog started a re-lock.
    #[serde(rename_all = "camelCase")]
    RelockStarted { primary: u8, serving_band: u16 },
    /// The watchdog stopped after this many failed attempts in a row.
    RelockGaveUp { primary: u8, attempts: u8 },
    /// The app was closed during a two-step apply; it carries on through safe apply.
    SequenceResumed {
        primary: u8,
        origin: SequenceOrigin,
        step: u8,
    },
    /// A saved two-step apply couldn't be used (unreadable, for another router, or not matching
    /// the journal). It was moved aside and no band was changed for it.
    SequenceDiscarded,
}

impl AppSnapshot {
    fn initial() -> Self {
        Self {
            session: SessionView::NotConfigured,
            device: None,
            status: None,
            signal: None,
            config: None,
            capabilities: None,
            safe_apply: SafeApplyView {
                phase: Phase::Idle,
                seconds_left: None,
                previous: None,
                requested: None,
                policy: None,
                sequence: None,
            },
            in_service: false,
            foreign_journal: None,
            captured_at_ms: wall_ms(),
            relock: None,
            wan: None,
            ip_refresh: None,
            data_switch_off: false,
            apn: None,
            apn_apply: None,
            ip_reattach: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Alert {
    CellChanged {
        from: CellKey,
        to: CellKey,
    },
    SafeApply {
        event: SafeApplyEvent,
    },
    Session {
        view: SessionView,
    },
    Automation {
        event: AutomationEvent,
    },
    IpRefresh {
        event: IpRefreshAlert,
    },
    /// An APN switch moved on (refusals are the command's reply instead).
    Apn {
        event: SafeApplyEvent,
    },
    /// After an add, edit or delete the router dialled with another profile. `reverting`: an
    /// APN switch back to the one from before has started.
    #[serde(rename_all = "camelCase")]
    ApnUnexpectedSwitch {
        reverting: bool,
    },
    /// A band switch for a new IP ended. Ranges only.
    IpReattachEnded {
        result: IpReattachResult,
    },
}

/// Refresh IP news for a toast or a notification. Ranges only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum IpRefreshAlert {
    /// A run ended (`Resumed` when it turned data back on after a restart).
    Ended { result: IpRefreshResult },
    /// Turning data back on failed; the app keeps trying.
    DataOnFailed { attempt: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CellKey {
    pub rat: Rat,
    pub pci: Option<u16>,
    #[ts(type = "number | null")]
    pub cell_id: Option<u64>,
    /// "B3" for LTE, "n78" for NR; empty when unknown.
    pub band: String,
}

impl CellKey {
    /// The cell for `events.jsonl`: rat, PCI and band, never the cell ID.
    pub fn for_log(&self) -> serde_json::Value {
        serde_json::json!({ "rat": self.rat, "pci": self.pci, "band": self.band })
    }
}

#[derive(Clone)]
pub struct ActorHandle {
    pub commands: mpsc::Sender<Command>,
    pub snapshots: watch::Receiver<AppSnapshot>,
    pub alerts: broadcast::Sender<Alert>,
    task: Arc<std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl ActorHandle {
    /// Ask the actor to stop and wait until its task has exited. After `timeout` the
    /// task is aborted (the journal is crash-safe; the next actor resumes any change)
    /// and `false` is returned. `true` means a clean exit.
    pub async fn shutdown(self, timeout: Duration) -> bool {
        let task = self.task.lock().unwrap().take();
        let _ = self.commands.send(Command::Shutdown).await;
        let Some(mut task) = task else { return true };
        match tokio::time::timeout(timeout, &mut task).await {
            Ok(_) => true,
            Err(_) => {
                task.abort();
                let _ = task.await;
                false
            }
        }
    }
}

pub fn spawn_actor(driver: Arc<dyn RouterDriver>, cfg: ActorConfig) -> ActorHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel(32);
    let (snap_tx, snap_rx) = watch::channel(AppSnapshot::initial());
    let (alert_tx, _) = broadcast::channel(64);
    let actor = Actor::new(driver, cfg, snap_tx, alert_tx.clone());
    let task = tokio::spawn(actor.run(cmd_rx));
    ActorHandle {
        commands: cmd_tx,
        snapshots: snap_rx,
        alerts: alert_tx,
        task: Arc::new(std::sync::Mutex::new(Some(task))),
    }
}

/// Wall-clock stamps of the current journal record (P6).
struct Stamps {
    id: String,
    created_ms: u64,
    changed_ms: u64,
}

/// Cell-change debounce: a new cell counts once it's seen on 2 consecutive polls.
#[derive(Default)]
struct CellTracker {
    current: Option<CellKey>,
    candidate: Option<(CellKey, u8)>,
}

impl CellTracker {
    /// Returns `(from, to)` when a change becomes stable. Polls without a serving cell are
    /// ignored, so a re-attach gap is not a change.
    fn observe(&mut self, key: Option<CellKey>) -> Option<(CellKey, CellKey)> {
        let key = key?;
        let Some(current) = &self.current else {
            self.current = Some(key);
            return None;
        };
        if *current == key {
            self.candidate = None;
            return None;
        }
        let seen = match &self.candidate {
            Some((k, n)) if *k == key => n + 1,
            _ => 1,
        };
        if seen >= 2 {
            self.candidate = None;
            let from = self.current.replace(key.clone())?;
            Some((from, key))
        } else {
            self.candidate = Some((key, seen));
            None
        }
    }
}

/// A primary-band apply: step 1 (P alone, or `step1`), then step 2 (`final_config`).
struct Sequence {
    origin: SequenceOrigin,
    primary: u8,
    final_config: NetConfig,
    /// Step 1's own config (a band switch for a new IP).
    step1: Option<NetConfig>,
    step: u8,
    /// Change id of the running step; `None` while step 2 waits to start.
    change: Option<String>,
    /// The config from before step 1 (step 1's snapshot).
    original: Option<NetConfig>,
    /// Step 2 reverts to `original` rather than to step 1's config.
    widen_reverts_to_original: bool,
    /// Wall clock of the start, for `pending-sequence.json`.
    created_ms: u64,
}

impl Sequence {
    fn view(&self) -> ApplySequenceView {
        ApplySequenceView {
            step: self.step,
            total: 2,
            primary: self.primary,
            final_config: self.final_config.clone(),
            origin: self.origin,
            step1: self.step1.clone(),
        }
    }

    /// Step 1's config: its own, else the final config with the primary band alone.
    fn step1_config(&self) -> NetConfig {
        self.step1
            .clone()
            .unwrap_or_else(|| primary_only(&self.final_config, self.primary))
    }

    fn record(&self, host: &str, driver: &str) -> SequenceRecord {
        SequenceRecord {
            version: SEQUENCE_VERSION,
            driver: driver.to_string(),
            host: host.to_string(),
            origin: self.origin,
            primary: self.primary,
            step: self.step,
            final_config: self.final_config.clone(),
            original: self.original.clone(),
            change: self.change.clone(),
            created_at_ms: self.created_ms,
            step1: self.step1.clone(),
        }
    }

    fn from_record(r: SequenceRecord) -> Self {
        let widen_reverts_to_original = r.original.as_ref().is_some_and(|o| *o != r.final_config);
        Self {
            origin: r.origin,
            primary: r.primary,
            final_config: r.final_config,
            step1: r.step1,
            step: r.step,
            change: r.change,
            original: r.original,
            widen_reverts_to_original,
            created_ms: r.created_at_ms,
        }
    }
}

/// What the launch found in the journal, for restoring a sequence.
enum JournalAtStart {
    None,
    /// Our own journal, resumed.
    Resumed {
        id: String,
        phase: JournalPhase,
        requested: NetConfig,
    },
    /// Foreign or unreadable: nothing may change.
    Blocked,
}

#[derive(Default)]
struct Relock {
    /// The serving LTE band has been off the primary band since then (no change open).
    off_since: Option<Instant>,
    /// The primary band has served since then.
    on_since: Option<Instant>,
    last_attempt: Option<Instant>,
    failures: u8,
    given_up: bool,
}

struct Actor {
    driver: Arc<dyn RouterDriver>,
    cfg: ActorConfig,
    driver_id: String,
    sa: SafeApply,
    pending_timers: Option<SafeApplyTimers>,
    snapshots: watch::Sender<AppSnapshot>,
    alerts: broadcast::Sender<Alert>,

    creds: Option<Credentials>,
    /// The username of the last Connect, for the `logged-out` marker.
    account_user: Option<String>,
    login_trust: LoginTrust,
    /// A login in this run has verified the server signature.
    verified_this_run: bool,
    logged_in: bool,
    session: SessionView,
    /// Failed re-logins since the session was lost (session-type failures).
    relogin_failures: u32,
    /// When the current login succeeded.
    logged_in_at: Option<Instant>,
    /// Optional reads and actions the router refused in this run ("apn", "wan", "data_switch",
    /// "data_switch_write", "network_search"). They
    /// stay off until the next Connect instead of counting as a lost session.
    refused: std::collections::BTreeSet<&'static str>,
    /// Consecutive failed polls or unreachable logins; drives the doubling back-off.
    poll_failures: u32,
    visible: bool,
    next_attempt: Option<Instant>,
    last_net_config: Option<Instant>,
    last_heartbeat: Option<Instant>,
    last_wan: Option<Instant>,
    /// When `wan` was last read successfully. An APN switch counts only a read after its POST.
    wan_read_at: Option<Instant>,
    /// A WAN read has succeeded since the last APN switch POST (the switch or its revert).
    wan_read_since_apn_post: bool,
    /// Logged at most once per session, so a failing heartbeat shows in the event log.
    heartbeat_failure_logged: bool,
    session_refreshed: Option<Instant>,
    last_capabilities_try: Option<Instant>,
    retry_ticks: Vec<Instant>,
    next_safe_tick: Option<Instant>,

    device: Option<DeviceInfo>,
    status: Option<Status>,
    signal: Option<Signal>,
    config: Option<NetConfig>,
    capabilities: Option<Capabilities>,
    wan: Option<WanView>,
    apn_profiles: Option<ApnProfiles>,
    last_apn: Option<Instant>,
    /// The APN switch (see the module docs).
    apn_sa: SafeApply<ApnTarget>,
    apn_stamps: Option<Stamps>,
    /// The profile index of the switch being started, for its events before the journal.
    apn_index: Option<u32>,
    /// The latest `Rejected` of the APN switch, for the command's reply.
    apn_rejected: Option<String>,
    /// An APN journal for another host or driver, or one that can't be read.
    apn_journal_blocked: bool,
    /// The last range seen, kept while data is off, so a change across a reconnect is logged.
    last_wan_range: Option<String>,
    in_service: bool,
    foreign_journal: Option<JournalRecord>,
    /// The journal file exists but can't be read: never overwrite it with a new change.
    journal_unreadable: bool,
    stamps: Option<Stamps>,
    reboot_ticket: Option<(String, Instant)>,
    factory_reset_ticket: Option<(String, Instant)>,
    cells: CellTracker,
    /// A command that arrived during a routine read; the main loop runs it next.
    preempted: Option<Command>,
    seq: Option<Sequence>,
    relock: Relock,
    /// The latest `Rejected` message.
    last_rejected: Option<String>,
    /// A speed test runs since then (`SetSpeedTestActive`).
    speed_test_since: Option<Instant>,
    scan_active: bool,
    /// The page scripts are being read since then (`BeginPageScripts`).
    page_scripts_since: Option<Instant>,
    ipr: IpRefresh,
    /// The open data-switch record of the current run (or the resumed one).
    data_journal: Option<DataSwitchRecord>,
    /// A data-switch record for another router, or one that can't be read.
    data_journal_blocked: bool,
    data_switch_off: bool,
    next_ip_tick: Option<Instant>,
    /// Where a network search task sends its result; the receiver is `run`'s.
    search_tx: Option<mpsc::Sender<Result<NetworkSearch, RouterError>>>,
    /// The running network search task, aborted at shutdown.
    search_task: Option<tokio::task::AbortHandle>,
    /// A poll during the running search saw no service.
    search_service_dropped: bool,
    /// The band config when the current IP refresh started.
    ip_config_before: Option<NetConfig>,
    /// Band switches for a new IP.
    reattach: Reattach,
    /// Raised during startup; sent after the first loop iteration, when the app listens.
    startup_alerts: Vec<AutomationEvent>,
}

/// The command channel, lent to routine polls so a user command can interrupt their reads.
type Commands<'a> = Option<&'a mut mpsc::Receiver<Command>>;

impl Actor {
    fn new(
        driver: Arc<dyn RouterDriver>,
        cfg: ActorConfig,
        snapshots: watch::Sender<AppSnapshot>,
        alerts: broadcast::Sender<Alert>,
    ) -> Self {
        let driver_id = driver.info().id.to_string();
        let mut sa = SafeApply::new(cfg.timers, cfg.host.clone(), driver_id.clone());
        sa.set_revert_on_loss(cfg.automation.revert_on_loss, now_std());
        let apn_sa = SafeApply::new(APN_TIMERS, cfg.host.clone(), driver_id.clone());
        Self {
            driver,
            cfg,
            driver_id,
            sa,
            pending_timers: None,
            snapshots,
            alerts,
            creds: None,
            account_user: None,
            login_trust: LoginTrust::Automatic,
            verified_this_run: false,
            logged_in: false,
            session: SessionView::NotConfigured,
            relogin_failures: 0,
            logged_in_at: None,
            refused: std::collections::BTreeSet::new(),
            poll_failures: 0,
            visible: true,
            next_attempt: None,
            last_net_config: None,
            last_heartbeat: None,
            last_wan: None,
            wan_read_at: None,
            wan_read_since_apn_post: false,
            heartbeat_failure_logged: false,
            session_refreshed: None,
            last_capabilities_try: None,
            retry_ticks: Vec::new(),
            next_safe_tick: None,
            device: None,
            status: None,
            signal: None,
            config: None,
            capabilities: None,
            wan: None,
            apn_profiles: None,
            last_apn: None,
            apn_sa,
            apn_stamps: None,
            apn_index: None,
            apn_rejected: None,
            apn_journal_blocked: false,
            last_wan_range: None,
            in_service: false,
            foreign_journal: None,
            journal_unreadable: false,
            stamps: None,
            reboot_ticket: None,
            factory_reset_ticket: None,
            cells: CellTracker::default(),
            preempted: None,
            seq: None,
            relock: Relock::default(),
            last_rejected: None,
            speed_test_since: None,
            scan_active: false,
            page_scripts_since: None,
            ipr: IpRefresh::new(),
            data_journal: None,
            data_journal_blocked: false,
            data_switch_off: false,
            next_ip_tick: None,
            search_tx: None,
            search_task: None,
            search_service_dropped: false,
            ip_config_before: None,
            reattach: Reattach::default(),
            startup_alerts: Vec::new(),
        }
    }

    async fn run(mut self, mut commands: mpsc::Receiver<Command>) {
        let (search_tx, mut search_rx) = mpsc::channel(1);
        self.search_tx = Some(search_tx);
        self.startup().await;
        self.after_iteration();
        loop {
            let wake = self.next_wake();
            tokio::select! {
                cmd = commands.recv() => match cmd {
                    None | Some(Command::Shutdown) => break,
                    Some(c) => self.on_command(c).await,
                },
                Some(result) = search_rx.recv() => self.on_search_done(result).await,
                _ = sleep_until_opt(wake) => self.on_wake(&mut commands).await,
            }
            // A command that interrupted a routine poll runs now, not after the poll.
            match self.preempted.take() {
                Some(Command::Shutdown) => break,
                Some(c) => self.on_command(c).await,
                None => {}
            }
            self.automation().await;
            for event in std::mem::take(&mut self.startup_alerts) {
                self.emit_automation(event).await;
            }
            self.after_iteration();
        }
        if let Some(task) = self.search_task.take() {
            task.abort();
        }
        self.data_on_at_exit().await;
    }

    // --- startup -----------------------------------------------------------------------------

    async fn startup(&mut self) {
        self.resume_data_switch().await;
        let journal = self.resume_journal().await;
        self.resume_apn_journal().await;
        self.restore_sequence(journal).await;
    }

    /// Loads `pending-data-switch.json` before anything else: an own record means data may be
    /// off, and is turned back on once logged in.
    async fn resume_data_switch(&mut self) {
        let dir = self.cfg.data_dir.clone();
        match blocking(move || DataSwitchStore::new(dir).load()).await {
            Ok(None) => {}
            Ok(Some(rec)) if rec.is_ours(&self.cfg.host, &self.driver_id) => {
                self.ipr.resume(rec.id.clone(), now_std());
                self.data_journal = Some(rec);
            }
            Ok(Some(rec)) => {
                self.data_journal_blocked = true;
                let detail = serde_json::json!({ "file": "data_switch", "id": rec.id });
                self.log_event(kinds::JOURNAL_FOREIGN, detail).await;
            }
            Err(e) => {
                self.data_journal_blocked = true;
                let detail = serde_json::json!({ "file": "data_switch", "error": e.to_string() });
                self.log_event(kinds::JOURNAL_UNREADABLE, detail).await;
            }
        }
    }

    async fn resume_journal(&mut self) -> JournalAtStart {
        let dir = self.cfg.data_dir.clone();
        match blocking(move || Journal::new(dir).load()).await {
            Ok(None) => JournalAtStart::None,
            Ok(Some(rec)) => {
                let (id, phase, requested) = (rec.id.clone(), rec.phase, rec.requested.clone());
                let stamps = Stamps {
                    id: rec.id.clone(),
                    created_ms: rec.created_at_ms,
                    changed_ms: rec.phase_changed_at_ms,
                };
                let resumed = SafeApply::resume(
                    rec,
                    self.cfg.timers,
                    &self.cfg.host,
                    &self.driver_id,
                    now_std(),
                );
                match resumed {
                    Ok((mut sa, effects)) => {
                        sa.set_revert_on_loss(self.cfg.automation.revert_on_loss, now_std());
                        self.sa = sa;
                        self.stamps = Some(stamps);
                        let detail = serde_json::json!({ "phase": self.sa.phase() });
                        self.log_event(kinds::SAFE_APPLY_RESUMED, detail).await;
                        self.run_effects(effects).await;
                        JournalAtStart::Resumed {
                            id,
                            phase,
                            requested,
                        }
                    }
                    Err(ForeignJournal(rec)) => {
                        let detail = serde_json::json!({ "id": rec.id, "phase": rec.phase });
                        self.foreign_journal = Some(rec);
                        self.log_event(kinds::JOURNAL_FOREIGN, detail).await;
                        JournalAtStart::Blocked
                    }
                }
            }
            Err(e) => {
                self.journal_unreadable = true;
                let detail = serde_json::json!({ "error": e.to_string() });
                self.log_event(kinds::JOURNAL_UNREADABLE, detail).await;
                JournalAtStart::Blocked
            }
        }
    }

    /// Loads `pending-apn-change.json`: an own record resumes the APN switch (its reads wait for
    /// the login); anything else blocks APN switches and is never acted on.
    async fn resume_apn_journal(&mut self) {
        let dir = self.cfg.data_dir.clone();
        match blocking(move || apn_journal(dir).load_record::<ApnTarget>()).await {
            Ok(None) => {}
            Ok(Some(rec)) => {
                let index = rec.requested.index;
                let stamps = Stamps {
                    id: rec.id.clone(),
                    created_ms: rec.created_at_ms,
                    changed_ms: rec.phase_changed_at_ms,
                };
                let resumed =
                    SafeApply::resume(rec, APN_TIMERS, &self.cfg.host, &self.driver_id, now_std());
                match resumed {
                    Ok((sa, effects)) => {
                        self.apn_sa = sa;
                        self.apn_stamps = Some(stamps);
                        self.apn_index = Some(index);
                        let detail = serde_json::json!({
                            "index": index,
                            "phase": self.apn_sa.phase(),
                        });
                        self.log_event(kinds::APN_RESUMED, detail).await;
                        self.run_apn_effects(effects).await;
                    }
                    Err(ForeignJournal(rec)) => {
                        self.apn_journal_blocked = true;
                        let detail =
                            serde_json::json!({ "file": "apn", "id": rec.id, "phase": rec.phase });
                        self.log_event(kinds::JOURNAL_FOREIGN, detail).await;
                    }
                }
            }
            Err(e) => {
                self.apn_journal_blocked = true;
                let detail = serde_json::json!({ "file": "apn", "error": e.to_string() });
                self.log_event(kinds::JOURNAL_UNREADABLE, detail).await;
            }
        }
    }

    /// Brings back a two-step apply from `pending-sequence.json` (see the module docs). Nothing
    /// here posts a config: a restored sequence moves on through `advance_sequence`, and step 2
    /// only through `start_change` once connected.
    async fn restore_sequence(&mut self, journal: JournalAtStart) {
        let dir = self.cfg.data_dir.clone();
        let rec = match blocking(move || SequenceStore::new(dir).load()).await {
            Ok(None) => return,
            Ok(Some(r)) => r,
            Err(e) => return self.set_sequence_aside("unreadable", &e.to_string()).await,
        };
        if rec.version != SEQUENCE_VERSION
            || rec.host != self.cfg.host
            || rec.driver != self.driver_id
        {
            let detail = format!("driver {} v{}", rec.driver, rec.version);
            return self.set_sequence_aside("foreign", &detail).await;
        }
        if !rec.is_well_formed() {
            return self.set_sequence_aside("invalid", "").await;
        }
        let q = Sequence::from_record(rec);
        let step_config = if q.step == 1 {
            q.step1_config()
        } else {
            q.final_config.clone()
        };
        match journal {
            JournalAtStart::Blocked => self.set_sequence_aside("journal_blocked", "").await,
            JournalAtStart::Resumed {
                id,
                phase,
                requested,
            } => {
                let same = match q.change.as_deref() {
                    Some(c) => c == id,
                    None => requested == step_config,
                };
                if !same {
                    return self.set_sequence_aside("journal_mismatch", "").await;
                }
                match (q.step, phase) {
                    // Closed between step 1's commit and the archive: step 2 is due.
                    (1, JournalPhase::Committed) => self.resume_sequence(q, None).await,
                    (_, JournalPhase::Committed | JournalPhase::Reverted) => {
                        self.forget_sequence(&q, "ended_before_launch").await;
                    }
                    _ => self.resume_sequence(q, Some(id)).await,
                }
            }
            JournalAtStart::None => match (q.step, q.change.clone()) {
                (1, Some(id)) => {
                    let dir = self.cfg.data_dir.clone();
                    let archived = blocking(move || Journal::new(dir).archived(&id)).await;
                    match archived {
                        Ok(Some(r)) if r.phase == JournalPhase::Committed => {
                            self.resume_sequence(q, None).await;
                        }
                        // Step 1 was reverted (and announced then): nothing is due.
                        Ok(Some(_)) => self.forget_sequence(&q, "step1_not_committed").await,
                        Ok(None) => {
                            self.set_sequence_aside("step1_outcome_unknown", "").await;
                        }
                        Err(e) => {
                            self.set_sequence_aside("step1_outcome_unknown", &e.to_string())
                                .await;
                        }
                    }
                }
                // Step 1 never got its journal: nothing was posted for it.
                (1, None) => self.set_sequence_aside("step1_never_started", "").await,
                // Step 2 was due and hadn't started.
                (_, None) => self.resume_sequence(q, None).await,
                // Step 2 ran and its journal was archived: it is over.
                (_, Some(_)) => self.forget_sequence(&q, "ended_before_launch").await,
            },
        }
    }

    /// `change`: the resumed journal of the running step, or `None` when step 2 is due.
    async fn resume_sequence(&mut self, mut q: Sequence, change: Option<String>) {
        if change.is_none() {
            q.step = 2;
        }
        if q.original.is_none() && q.step == 1 {
            q.original = self.sa.record().map(|r| r.previous);
        }
        q.change = change;
        let (primary, origin, step) = (q.primary, q.origin, q.step);
        let detail = serde_json::json!({
            "origin": origin,
            "primary": primary,
            "step": step,
            "waiting": q.change.is_none(),
            "final": q.final_config,
        });
        self.seq = Some(q);
        self.persist_sequence().await;
        self.log_event(kinds::SEQUENCE_RESUMED, detail).await;
        self.startup_alerts.push(AutomationEvent::SequenceResumed {
            primary,
            origin,
            step,
        });
    }

    /// A sequence on file that is already over: removed, nothing to do.
    async fn forget_sequence(&mut self, q: &Sequence, outcome: &str) {
        let detail = serde_json::json!({
            "origin": q.origin,
            "primary": q.primary,
            "step": q.step,
            "outcome": outcome,
        });
        self.log_event(kinds::SEQUENCE_ENDED, detail).await;
        self.clear_sequence_file().await;
    }

    /// Moves an unusable sequence file out of the way and reports it. Never changes a band.
    async fn set_sequence_aside(&mut self, why: &str, error: &str) {
        let dir = self.cfg.data_dir.clone();
        let at = wall_ms();
        let moved = blocking(move || SequenceStore::new(dir).set_aside(at)).await;
        let moved_to = moved
            .as_ref()
            .ok()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned());
        let detail = serde_json::json!({
            "why": why,
            "error": error,
            "moved_to": moved_to,
            "move_error": moved.as_ref().err().map(|e| e.to_string()),
        });
        self.log_event(kinds::SEQUENCE_DISCARDED, detail).await;
        self.startup_alerts.push(AutomationEvent::SequenceDiscarded);
    }

    async fn persist_sequence(&mut self) {
        let Some(q) = self.seq.as_ref() else { return };
        let rec = q.record(&self.cfg.host, &self.driver_id);
        let dir = self.cfg.data_dir.clone();
        if let Err(e) = blocking(move || SequenceStore::new(dir).write(&rec)).await {
            let detail = serde_json::json!({ "error": e.to_string() });
            self.log_event(kinds::SEQUENCE_WRITE_FAILED, detail).await;
        }
    }

    async fn clear_sequence_file(&mut self) {
        let dir = self.cfg.data_dir.clone();
        if let Err(e) = blocking(move || SequenceStore::new(dir).clear()).await {
            let detail = serde_json::json!({ "error": e.to_string() });
            self.log_event(kinds::SEQUENCE_CLEAR_FAILED, detail).await;
        }
    }

    /// A speed test runs (and hasn't outlived `SPEED_TEST_CAP`).
    fn speed_test_active(&self) -> bool {
        self.speed_test_since
            .is_some_and(|t| t.elapsed() < SPEED_TEST_CAP)
    }

    /// The page scripts are being read (and the read hasn't outlived `PAGE_SCRIPTS_CAP`).
    fn page_scripts_active(&self) -> bool {
        self.page_scripts_since
            .is_some_and(|t| t.elapsed() < PAGE_SCRIPTS_CAP)
    }

    // --- loop plumbing -----------------------------------------------------------------------

    fn next_wake(&self) -> Option<Instant> {
        let reattach_end = self
            .reattach
            .finishing
            .as_ref()
            .map(|f| f.ended_at + REATTACH_TO_WAIT);
        [
            self.next_attempt,
            self.next_safe_tick,
            self.next_ip_tick,
            reattach_end,
        ]
        .into_iter()
        .flatten()
        .chain(self.retry_ticks.iter().copied())
        .min()
    }

    async fn on_wake(&mut self, commands: &mut mpsc::Receiver<Command>) {
        let now = Instant::now();
        self.retry_ticks.retain(|t| *t > now);
        self.step(Input::Tick).await;
        self.apn_step(Input::Tick).await;
        if self.next_ip_tick.is_some_and(|t| t <= now) {
            self.next_ip_tick = None;
            self.ip_step(IpInput::Tick).await;
        }
        if self.next_attempt.is_some_and(|t| t <= now) {
            self.next_attempt = None;
            if self.logged_in {
                self.poll_with(Some(commands)).await;
            } else {
                self.try_login().await;
            }
        }
    }

    fn after_iteration(&mut self) {
        self.maybe_apply_pending_timers();
        self.next_safe_tick =
            (self.change_active() || self.apn_active()).then(|| Instant::now() + SAFE_TICK);
        if !self.ipr.active() {
            self.next_ip_tick = None;
        } else if self.next_ip_tick.is_none() {
            self.next_ip_tick = Some(Instant::now() + IP_TICK);
        }
        self.publish();
    }

    fn publish(&self) {
        let mut safe_apply = self.sa.view(now_std());
        safe_apply.sequence = self.seq.as_ref().map(Sequence::view);
        let snap = AppSnapshot {
            session: self.session.clone(),
            device: self.device.clone(),
            status: self.status.clone(),
            signal: self.signal.clone(),
            config: self.config.clone(),
            capabilities: self.capabilities.clone(),
            safe_apply,
            in_service: self.in_service,
            foreign_journal: self.foreign_journal.clone(),
            captured_at_ms: wall_ms(),
            relock: self.relock_view(),
            wan: self.wan.clone(),
            ip_refresh: self.ip_refresh_view(),
            data_switch_off: self.data_switch_off,
            apn: self.apn_profiles.clone(),
            apn_apply: (*self.apn_sa.phase() != Phase::Idle)
                .then(|| ApnApplyView::from(self.apn_sa.view(now_std()))),
            ip_reattach: self.ip_reattach_view(),
        };
        self.snapshots.send_replace(snap);
    }

    fn ip_refresh_view(&self) -> Option<IpRefreshView> {
        let show = self.ipr.active() || self.ipr.last().is_some() || self.data_journal_blocked;
        show.then(|| self.ipr.view(now_std(), self.data_journal_blocked))
    }

    /// A change is in flight: anything but Idle, Committed or Reverted.
    fn change_active(&self) -> bool {
        !change_closed(self.sa.phase())
    }

    /// An APN switch is in flight.
    fn apn_active(&self) -> bool {
        !change_closed(self.apn_sa.phase())
    }

    /// Logged in and the last poll succeeded.
    fn online(&self) -> bool {
        self.logged_in && self.poll_failures == 0
    }

    fn base_interval(&self) -> Duration {
        let base = if self.visible {
            VISIBLE_POLL
        } else {
            HIDDEN_POLL
        };
        if self.ipr.searching() {
            base.max(SEARCH_POLL)
        } else {
            base
        }
    }

    fn backoff_interval(&self) -> Duration {
        let base = self.base_interval();
        if self.poll_failures == 0 {
            return base;
        }
        let factor = 1u32 << self.poll_failures.min(5);
        (base * factor).min(MAX_BACKOFF)
    }

    fn relogin_delay(&self) -> Duration {
        // Data may be off during a Refresh IP run: log back in quickly to turn it on.
        if self.change_active()
            || self.apn_active()
            || self.ipr.active()
            || self.ipr.data_may_be_off()
        {
            ACTIVE_RELOGIN
        } else {
            RELOGIN_DELAYS[(self.relogin_failures as usize).min(RELOGIN_DELAYS.len() - 1)]
        }
    }

    /// Schedules the next login/poll attempt and tells `SafeApply` the cadence.
    fn schedule_attempt(&mut self, delay: Duration) {
        self.next_attempt = Some(Instant::now() + delay);
        self.sa.set_poll_interval(delay);
        self.apn_sa.set_poll_interval(delay);
    }

    fn maybe_apply_pending_timers(&mut self) {
        if self.change_active() {
            return;
        }
        if let Some(t) = self.pending_timers.take() {
            self.cfg.timers = t;
            self.sa = SafeApply::new(t, self.cfg.host.clone(), self.driver_id.clone());
            self.sa.set_poll_interval(self.backoff_interval());
            self.sa
                .set_revert_on_loss(self.cfg.automation.revert_on_loss, now_std());
        }
    }

    // --- commands ----------------------------------------------------------------------------

    async fn on_command(&mut self, cmd: Command) {
        match cmd {
            Command::Connect(creds) => self.connect(creds, LoginTrust::UserAsked).await,
            Command::ConnectAutomatic(creds) => self.connect(creds, LoginTrust::Automatic).await,
            Command::ConnectTrusting(creds) => self.connect(creds, LoginTrust::UserTrusted).await,
            Command::Apply(requested) => self.apply(requested).await,
            // One change at a time: Keep and Revert go to the one that is open.
            Command::KeepNow if self.apn_active() => self.apn_step(Input::KeepNow).await,
            Command::RevertNow if self.apn_active() => self.apn_step(Input::RevertNow).await,
            Command::KeepNow => self.step(Input::KeepNow).await,
            Command::RevertNow => {
                // Between the steps there is nothing to revert: drop the pending step 2.
                if self.seq.as_ref().is_some_and(|q| q.change.is_none()) {
                    self.cancel_sequence("revert_now").await;
                }
                self.step(Input::RevertNow).await
            }
            Command::GiveUp => self.step(Input::GiveUp).await,
            Command::RequestReboot(reply) => {
                let r = self.request_reboot();
                let _ = reply.send(r);
            }
            Command::RequestFactoryReset(reply) => {
                let r = self.request_factory_reset();
                let _ = reply.send(r);
            }
            Command::ConfirmFactoryReset(id, reply) => {
                let r = self.confirm_factory_reset(&id).await;
                let _ = reply.send(r);
            }
            Command::ConfirmReboot(id, reply) => {
                let r = self.confirm_reboot(&id).await;
                let _ = reply.send(r);
            }
            Command::Logout(reply) => {
                let r = self.logout().await;
                let _ = reply.send(r);
            }
            Command::SetVisible(visible) => {
                self.visible = visible;
                if self.logged_in {
                    let interval = self.backoff_interval();
                    let soon = Instant::now() + interval;
                    if self.next_attempt.is_none_or(|t| t > soon) {
                        self.next_attempt = Some(soon);
                    }
                    self.sa.set_poll_interval(interval);
                    self.apn_sa.set_poll_interval(interval);
                }
            }
            Command::UpdateTimers(t) => self.pending_timers = Some(t),
            Command::SetCellAlerts(on) => self.cfg.cell_alerts = on,
            Command::SetAutomation(a) => self.set_automation(a).await,
            Command::SetSpeedTestActive(on) => {
                if on == self.speed_test_since.is_some() {
                    return;
                }
                self.speed_test_since = on.then(Instant::now);
                let mut detail = serde_json::json!({ "active": on });
                // The app's own guards refuse this; a test that starts anyway is noted.
                if on && self.ipr.active() {
                    detail["conflict"] = "ip_refresh".into();
                }
                self.log_event(kinds::SPEEDTEST_ACTIVE, detail).await;
            }
            Command::BeginPageScripts(reply) => {
                let r = match self.busy_refusal(Op::PageScripts) {
                    Some(e) => Err(e),
                    None if self.ticket_open() => Err(validation(REBOOT_PENDING)),
                    None if !self.online() => Err(validation(NOT_CONNECTED)),
                    None => {
                        self.page_scripts_since = Some(Instant::now());
                        Ok(())
                    }
                };
                let _ = reply.send(r);
            }
            Command::EndPageScripts => self.page_scripts_since = None,
            Command::SetScanActive(on) => {
                if on != self.scan_active {
                    self.scan_active = on;
                    let detail = serde_json::json!({ "active": on });
                    self.log_event(kinds::SCAN_ACTIVE, detail).await;
                }
            }
            Command::StartIpRefresh {
                target,
                max_tries,
                reply,
            } => {
                let r = self.start_ip_refresh(target.as_deref(), max_tries).await;
                let _ = reply.send(r);
            }
            Command::StopIpRefresh => self.ip_step(IpInput::Stop).await,
            Command::TurnDataOn(reply) => {
                let r = self.turn_data_on().await;
                let _ = reply.send(r);
            }
            Command::StartIpReattach(reply) => {
                let r = self.start_ip_reattach().await;
                let _ = reply.send(r);
            }
            Command::ApnRefresh => {
                if self.logged_in {
                    self.read_apn().await;
                }
            }
            Command::ApnSwitch { target, reply } => {
                let r = self.apn_switch(target).await;
                let _ = reply.send(r);
            }
            Command::ApnAdd { draft, reply } => {
                let r = self.apn_write(ApnWrite::Add(draft)).await;
                let _ = reply.send(r);
            }
            Command::ApnEdit {
                target,
                draft,
                reply,
            } => {
                let r = self.apn_write(ApnWrite::Edit(target, draft)).await;
                let _ = reply.send(r.map(|_| ()));
            }
            Command::ApnDelete { target, reply } => {
                let r = self.apn_write(ApnWrite::Delete(target)).await;
                let _ = reply.send(r.map(|_| ()));
            }
            // Handled by the loop.
            Command::Shutdown => {}
        }
    }

    async fn apply(&mut self, requested: NetConfig) {
        let busy = if self.ipr.active() {
            Some(IP_REFRESH_RUNNING)
        } else if self.apn_active() {
            Some(APN_CHANGE_RUNNING)
        } else if self.page_scripts_active() {
            Some(PAGE_SCRIPTS_RUNNING)
        } else {
            None
        };
        if let Some(message) = busy {
            let event = SafeApplyEvent::Rejected {
                message: message.to_string(),
            };
            self.emit_safe_event(event).await;
            return;
        }
        // A band change would spoil the measurement (and the test the change).
        if self.speed_test_active() {
            let event = SafeApplyEvent::Rejected {
                message: SPEED_TEST_RUNNING.to_string(),
            };
            self.emit_safe_event(event).await;
            return;
        }
        // A new Apply replaces a sequence that waits between its steps.
        if self.seq.is_some() && !self.change_active() {
            self.cancel_sequence("replaced").await;
        }
        if !self.change_active() {
            if let Some(primary) = self.two_step_primary(&requested.lte_bands) {
                let step1 = primary_only(&requested, primary);
                if self.config.as_ref() != Some(&step1) {
                    self.begin_sequence(SequenceOrigin::User, primary, requested, step1)
                        .await;
                    return;
                }
            }
        }
        self.start_change(requested, None).await;
    }

    /// The safe-apply `Start`, unless something blocks a change. Returns whether a new change
    /// started (it may already have closed, e.g. when the snapshot read failed).
    async fn start_change(&mut self, requested: NetConfig, revert_to: Option<NetConfig>) -> bool {
        let blocker = if self.ipr.active() {
            Some(IP_REFRESH_RUNNING)
        } else if self.apn_active() {
            Some(APN_CHANGE_RUNNING)
        } else if self.foreign_journal.is_some() {
            Some("a pending change for another router is on file")
        } else if self.journal_unreadable {
            Some("the pending-change journal can't be read")
        } else if !self.online() {
            Some(NOT_CONNECTED)
        } else if self.capabilities.is_none() {
            Some("the router's capabilities aren't known yet")
        } else {
            None
        };
        if let Some(message) = blocker {
            let event = SafeApplyEvent::Rejected {
                message: message.to_string(),
            };
            self.emit_safe_event(event).await;
            return false;
        }
        let before = self.sa.change_id().map(str::to_owned);
        let input = Input::Start {
            requested,
            caps: self.capabilities.clone().expect("checked above"),
            current: self.config.clone(),
            revert_to,
        };
        self.step(input).await;
        self.sa.change_id() != before.as_deref()
    }

    // --- automation --------------------------------------------------------------------------

    async fn set_automation(&mut self, a: AutomationConfig) {
        if a == self.cfg.automation {
            return;
        }
        self.cfg.automation = a;
        self.sa.set_revert_on_loss(a.revert_on_loss, now_std());
        // New settings give the watchdog a fresh start (the rate limit stays).
        self.relock.failures = 0;
        self.relock.given_up = false;
        self.relock.off_since = None;
        self.relock.on_since = None;
        let detail = serde_json::json!({
            "primary_band": a.primary_band,
            "relock_primary": a.relock_primary,
            "revert_on_loss": a.revert_on_loss,
        });
        self.log_event(kinds::AUTOMATION_SETTINGS, detail).await;
    }

    /// The primary band, when `bands` holds it and at least one other band.
    fn two_step_primary(&self, bands: &LteBandSet) -> Option<u8> {
        // A scan measures exactly the set it asks for.
        if self.scan_active {
            return None;
        }
        let p = self.cfg.automation.primary_band?;
        let only = LteBandSet::from_bands(&[p]).ok()?;
        (bands.contains(p) && *bands != only).then_some(p)
    }

    async fn begin_sequence(
        &mut self,
        origin: SequenceOrigin,
        primary: u8,
        final_config: NetConfig,
        step1: NetConfig,
    ) {
        let detail = serde_json::json!({
            "origin": origin,
            "primary": primary,
            "step1": step1,
            "final": final_config,
        });
        self.log_event(kinds::SEQUENCE_STARTED, detail).await;
        let own_step1 = (origin == SequenceOrigin::IpReattach).then(|| step1.clone());
        self.seq = Some(Sequence {
            origin,
            primary,
            final_config,
            step1: own_step1,
            step: 1,
            change: None,
            original: None,
            widen_reverts_to_original: false,
            created_ms: wall_ms(),
        });
        // On file before step 1's journal, so a crash in between is caught at launch.
        self.persist_sequence().await;
        self.last_rejected = None;
        if self.start_change(step1, None).await {
            let id = self.sa.change_id().map(str::to_owned);
            let original = self.sa.record().map(|r| r.previous);
            if let Some(q) = self.seq.as_mut() {
                q.change = id;
                q.original = original;
            }
        } else {
            // Refused before anything was sent (the Rejected alert says why).
            self.end_sequence(false, "step1_rejected").await;
        }
    }

    /// Runs after every loop iteration: moves a sequence on, then the watchdog.
    async fn automation(&mut self) {
        self.advance_sequence().await;
        self.maybe_relock().await;
        self.reattach_tick().await;
    }

    async fn advance_sequence(&mut self) {
        let Some(q) = self.seq.as_ref() else { return };
        let Some(id) = q.change.clone() else {
            // Step 2 waits until a change can start, and not during a speed test.
            if self.online()
                && self.capabilities.is_some()
                && !self.change_active()
                && !self.speed_test_active()
                && !self.page_scripts_active()
                && !self.ipr.active()
                && !self.apn_active()
            {
                // Someone else changed the bands meanwhile (or after a restart): step 2 would
                // widen from a config the sequence didn't set.
                let step1 = q.step1_config();
                match self.config.as_ref() {
                    Some(c) if *c == step1 => self.start_widen().await,
                    Some(_) => {
                        self.cancel_sequence("config_changed").await;
                        self.emit_automation(AutomationEvent::SequenceDiscarded)
                            .await;
                    }
                    // Not read yet: the next poll reads it.
                    None => {}
                }
            }
            return;
        };
        if self.sa.change_id() != Some(id.as_str()) {
            self.cancel_sequence("superseded").await;
            return;
        }
        let (step, primary, origin) = (q.step, q.primary, q.origin);
        // A band switch for a new IP needs step 1 only until the router has a new address: it
        // is kept once it re-attached and a WAN read since then shows a range.
        if origin == SequenceOrigin::IpReattach && step == 1 && self.reattach_keep_due() {
            let detail = serde_json::json!({ "range": self.wan_range() });
            self.log_event(kinds::IP_REATTACH_STEP1_KEPT, detail).await;
            self.step(Input::KeepNow).await;
        }
        if self.change_active() {
            return;
        }
        let phase = self.sa.phase().clone();
        let already_had =
            phase == Phase::Idle && self.last_rejected.as_deref() == Some(ALREADY_HAS);
        match (step, phase) {
            (1, Phase::Committed) => self.start_widen().await,
            // The router was already on the primary band alone: go on to step 2.
            (1, _) if already_had => self.start_widen().await,
            // The user pressed Revert: no failure alert, the revert notification says it all.
            (
                _,
                Phase::Reverted {
                    reason: RevertReason::UserRequested,
                    ..
                },
            ) => {
                self.end_sequence(false, "reverted_by_user").await;
            }
            (1, _) => {
                // A band switch for a new IP reports its own end (`ip_reattach.ended`).
                if origin != SequenceOrigin::IpReattach {
                    self.emit_automation(AutomationEvent::PrimaryLockFailed { primary, origin })
                        .await;
                }
                self.end_sequence(false, "step1_failed").await;
            }
            (_, Phase::Committed) => {
                // Only the watchdog's own sequence has to end on the primary band.
                let on_primary = self.serving_lte_band() == Some(u16::from(primary));
                self.end_sequence(on_primary || origin != SequenceOrigin::Relock, "committed")
                    .await;
            }
            (_, phase) => {
                let to_original = self
                    .seq
                    .as_ref()
                    .is_some_and(|q| q.widen_reverts_to_original);
                let ended_on = match phase {
                    Phase::Reverted { fallback: true, .. } => WidenFallback::Auto,
                    Phase::Reverted { .. } if to_original => WidenFallback::Original,
                    _ => WidenFallback::PrimaryOnly,
                };
                self.emit_automation(AutomationEvent::WidenFailed {
                    primary,
                    origin,
                    ended_on,
                })
                .await;
                self.end_sequence(false, "step2_failed").await;
            }
        }
    }

    /// Step 2: the full config, reverting to the config from before step 1 when that differs.
    async fn start_widen(&mut self) {
        let ready = self.online()
            && self.capabilities.is_some()
            && !self.speed_test_active()
            && !self.page_scripts_active()
            && !self.ipr.active()
            && !self.apn_active();
        let Some(q) = self.seq.as_mut() else { return };
        let moved_on = q.step != 2 || q.change.is_some();
        q.step = 2;
        q.change = None;
        if moved_on {
            self.persist_sequence().await;
        }
        if !ready {
            // Waits for the session (or a speed test); `advance_sequence` retries.
            return;
        }
        let Some(q) = self.seq.as_mut() else { return };
        let final_config = q.final_config.clone();
        let revert_to = q.original.clone().filter(|o| *o != final_config);
        q.widen_reverts_to_original = revert_to.is_some();
        let detail = serde_json::json!({ "final": final_config, "revert_to": revert_to });
        self.log_event(kinds::SEQUENCE_STEP2, detail).await;
        self.last_rejected = None;
        if self.start_change(final_config, revert_to).await {
            let id = self.sa.change_id().map(str::to_owned);
            if let Some(q) = self.seq.as_mut() {
                q.change = id;
            }
        } else {
            let (primary, origin) = match self.seq.as_ref() {
                Some(q) => (q.primary, q.origin),
                None => return,
            };
            self.emit_automation(AutomationEvent::WidenFailed {
                primary,
                origin,
                ended_on: WidenFallback::PrimaryOnly,
            })
            .await;
            self.end_sequence(false, "step2_rejected").await;
        }
    }

    /// Ends the sequence; a watchdog attempt counts toward the failure limit.
    async fn end_sequence(&mut self, ok: bool, outcome: &str) {
        let Some(q) = self.seq.take() else { return };
        self.clear_sequence_file().await;
        let detail = serde_json::json!({
            "origin": q.origin,
            "primary": q.primary,
            "step": q.step,
            "outcome": outcome,
            "ok": ok,
        });
        self.log_event(kinds::SEQUENCE_ENDED, detail).await;
        self.relock.off_since = None;
        if q.origin == SequenceOrigin::IpReattach {
            let ended = match outcome {
                "committed" => None,
                "step1_failed" | "step1_rejected" => Some(IpReattachOutcome::Step1Failed),
                "step2_failed" | "step2_rejected" => Some(IpReattachOutcome::Step2Failed),
                "reverted_by_user" => Some(IpReattachOutcome::Reverted),
                _ => Some(IpReattachOutcome::Cancelled),
            };
            self.reattach_sequence_ended(&q, ended, outcome == "step1_rejected");
            return;
        }
        if q.origin != SequenceOrigin::Relock {
            return;
        }
        if ok {
            self.relock.failures = 0;
        } else {
            self.relock.failures = self.relock.failures.saturating_add(1);
            if self.relock.failures >= RELOCK_MAX_FAILURES && !self.relock.given_up {
                self.relock.given_up = true;
                let attempts = self.relock.failures;
                self.log_event(
                    kinds::RELOCK_GAVE_UP,
                    serde_json::json!({ "primary": q.primary, "attempts": attempts }),
                )
                .await;
                self.emit_automation(AutomationEvent::RelockGaveUp {
                    primary: q.primary,
                    attempts,
                })
                .await;
            }
        }
        let detail = serde_json::json!({
            "primary": q.primary,
            "ok": ok,
            "failures": self.relock.failures,
        });
        self.log_event(kinds::RELOCK_OUTCOME, detail).await;
    }

    async fn cancel_sequence(&mut self, why: &str) {
        if let Some(q) = self.seq.take() {
            self.clear_sequence_file().await;
            let detail = serde_json::json!({ "origin": q.origin, "step": q.step, "why": why });
            self.log_event(kinds::SEQUENCE_CANCELLED, detail).await;
            if q.origin == SequenceOrigin::IpReattach {
                self.reattach_sequence_ended(&q, Some(IpReattachOutcome::Cancelled), false);
            }
        }
    }

    // --- new IP by a band switch -------------------------------------------------------------

    /// Why a band switch for a new IP can't start now (the band config checks come later).
    fn ip_reattach_refusal(&self) -> Option<RouterError> {
        if let Some(e) = self.busy_refusal(Op::IpReattach) {
            return Some(e);
        }
        if self.ticket_open() {
            return Some(validation(REBOOT_PENDING));
        }
        if self.foreign_journal.is_some() {
            return Some(validation("a pending change for another router is on file"));
        }
        if self.journal_unreadable {
            return Some(validation("the pending-change journal can't be read"));
        }
        if self.data_journal_blocked {
            return Some(validation(DATA_JOURNAL_BLOCKED));
        }
        if !self.online() {
            return Some(validation(NOT_CONNECTED));
        }
        if !self.in_service || self.wan_range().is_none() {
            return Some(validation(IP_NOT_IN_SERVICE));
        }
        // The switch promises to undo itself; with the loss policy off a step without service
        // would move on to probation and be kept.
        if !self.cfg.automation.revert_on_loss {
            return Some(validation(IP_REATTACH_UNDO_OFF));
        }
        None
    }

    /// The range from the last WAN read.
    fn wan_range(&self) -> Option<String> {
        self.wan.as_ref().and_then(|w| w.range.clone())
    }

    async fn start_ip_reattach(&mut self) -> Result<(), RouterError> {
        if let Some(e) = self.ip_reattach_refusal() {
            return Err(e);
        }
        let (Some(config), Some(caps)) = (self.config.clone(), self.capabilities.clone()) else {
            return Err(validation(BANDS_NOT_READ));
        };
        if !caps.band_edit_allowed {
            return Err(validation(BAND_EDIT_NOT_ALLOWED));
        }
        let step1 = reattach_step1(&config, &caps, self.serving_lte_band())?;
        let now = Instant::now();
        while self
            .reattach
            .starts
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= HOUR)
        {
            self.reattach.starts.pop_front();
        }
        if self.reattach.starts.len() >= REATTACH_PER_HOUR {
            return Err(validation(IP_REATTACH_COOLDOWN));
        }
        // The one before is still waiting for its new range: it ends now, without one.
        if self.reattach.finishing.is_some() {
            self.finish_reattach(None).await;
        }
        let from = self.wan_range();
        self.reattach.starts.push_back(now);
        self.reattach.from = from.clone();
        self.reattach.reattached_at = None;
        let detail = serde_json::json!({ "from": from, "step1": step1, "final": config });
        self.log_event(kinds::IP_REATTACH_STARTED, detail).await;
        let first = step1.lte_bands.bands().first().copied().unwrap_or(0);
        self.begin_sequence(SequenceOrigin::IpReattach, first, config, step1)
            .await;
        if self.seq.is_none() {
            // Refused before anything was sent; `ip_reattach.ended` follows as for any end.
            let message = self
                .last_rejected
                .clone()
                .unwrap_or_else(|| CHANGE_IN_PROGRESS.to_string());
            return Err(validation(&message));
        }
        Ok(())
    }

    /// Step 1 of a band switch for a new IP re-attached, and a WAN read since then shows a
    /// range: it can be kept without the rest of its probation.
    fn reattach_keep_due(&self) -> bool {
        let Some(at) = self.reattach.reattached_at else {
            return false;
        };
        *self.sa.phase() == Phase::Probation
            && self.wan_read_at.is_some_and(|r| r > at)
            && self.wan_range().is_some()
    }

    /// The WAN address is read every `REATTACH_WAN_EVERY` (step 1 waiting for its early keep,
    /// or the new range after the end).
    fn reattach_wan_window(&self) -> bool {
        let waiting_keep = self.reattach.reattached_at.is_some()
            && *self.sa.phase() == Phase::Probation
            && self
                .seq
                .as_ref()
                .is_some_and(|q| q.origin == SequenceOrigin::IpReattach && q.step == 1);
        waiting_keep || self.reattach.finishing.is_some()
    }

    /// The sequence of a band switch for a new IP ended (`outcome` `None`: with the bands
    /// back). The result waits for the first WAN read, at most `REATTACH_TO_WAIT`.
    /// `nothing_sent`: step 1 was refused before its POST.
    fn reattach_sequence_ended(
        &mut self,
        q: &Sequence,
        outcome: Option<IpReattachOutcome>,
        nothing_sent: bool,
    ) {
        // On the bands from before: step 2 kept (not merely due after step 1's commit), or step
        // 1 undone.
        let restored = nothing_sent
            || match self.sa.phase() {
                Phase::Committed => q.step == 2 && q.change.is_some(),
                // Idle: step 1 closed before its POST (its snapshot read failed).
                Phase::Reverted {
                    fallback: false, ..
                }
                | Phase::Idle => q.step == 1,
                _ => false,
            };
        self.reattach.reattached_at = None;
        self.reattach.finishing = Some(Finishing {
            outcome,
            from: self.reattach.from.take(),
            restore: (!restored).then(|| q.final_config.clone()),
            ended_at: Instant::now(),
        });
        // The next poll reads the address.
        self.last_wan = None;
    }

    /// Ends the band switch for a new IP with the range read after it (`None`: none in time).
    async fn finish_reattach(&mut self, to: Option<String>) {
        let Some(f) = self.reattach.finishing.take() else {
            return;
        };
        let outcome = f.outcome.unwrap_or(match (&f.from, &to) {
            (Some(a), Some(b)) if a != b => IpReattachOutcome::Changed,
            (Some(_), Some(_)) => IpReattachOutcome::Unchanged,
            _ => IpReattachOutcome::Unknown,
        });
        let result = IpReattachResult {
            outcome,
            from: f.from,
            to,
            restore: f.restore,
        };
        let detail = serde_json::json!({
            "outcome": result.outcome,
            "from": result.from,
            "to": result.to,
            "bands_restored": result.restore.is_none(),
        });
        self.log_event(kinds::IP_REATTACH_ENDED, detail).await;
        let _ = self.alerts.send(Alert::IpReattachEnded {
            result: result.clone(),
        });
        self.reattach.last = Some(result);
    }

    /// No WAN read in time after the end: the result goes out without a new range.
    async fn reattach_tick(&mut self) {
        let late = self
            .reattach
            .finishing
            .as_ref()
            .is_some_and(|f| f.ended_at.elapsed() >= REATTACH_TO_WAIT);
        if late {
            self.finish_reattach(None).await;
        }
    }

    fn ip_reattach_view(&self) -> Option<IpReattachView> {
        let active = self
            .seq
            .as_ref()
            .is_some_and(|q| q.origin == SequenceOrigin::IpReattach);
        let from_range = if active {
            self.reattach.from.clone()
        } else {
            self.reattach
                .finishing
                .as_ref()
                .and_then(|f| f.from.clone())
        };
        let show = active || self.reattach.finishing.is_some() || self.reattach.last.is_some();
        show.then(|| IpReattachView {
            active,
            from_range,
            last: self.reattach.last.clone(),
        })
    }

    fn serving_lte_band(&self) -> Option<u16> {
        self.signal.as_ref()?.lte.as_ref()?.band
    }

    /// Off/on-primary timers, fed by each successful poll.
    async fn track_primary(&mut self) {
        let now = Instant::now();
        let Some(p) = self.cfg.automation.primary_band else {
            self.relock.off_since = None;
            self.relock.on_since = None;
            return;
        };
        // Only the router's own choice counts: a change or sequence (the watchdog's own step 1
        // among them) holds it on P, which says nothing about whether P sticks.
        let settled = !self.change_active() && self.seq.is_none();
        match self.serving_lte_band() {
            Some(b) if settled && b == u16::from(p) => {
                self.relock.off_since = None;
                let since = *self.relock.on_since.get_or_insert(now);
                let stuck = self.relock.failures > 0 || self.relock.given_up;
                if stuck && now.saturating_duration_since(since) >= RELOCK_RESET_AFTER {
                    self.relock.failures = 0;
                    self.relock.given_up = false;
                    self.log_event(kinds::RELOCK_RESET, serde_json::json!({ "primary": p }))
                        .await;
                }
            }
            Some(_) if settled => {
                self.relock.on_since = None;
                self.relock.off_since.get_or_insert(now);
            }
            _ => {
                self.relock.on_since = None;
                self.relock.off_since = None;
            }
        }
    }

    async fn maybe_relock(&mut self) {
        let a = self.cfg.automation;
        let Some(p) = a.primary_band else { return };
        if !a.relock_primary
            || self.relock.given_up
            || self.seq.is_some()
            || self.change_active()
            || !self.online()
            || self.capabilities.is_none()
            || self.foreign_journal.is_some()
            || self.journal_unreadable
            || self.speed_test_active()
            || self.page_scripts_active()
            || self.scan_active
            || self.ipr.active()
            || self.apn_active()
        {
            return;
        }
        let Some(current) = self.config.clone() else {
            return;
        };
        if self.two_step_primary(&current.lte_bands).is_none() {
            return;
        }
        let now = Instant::now();
        let off_long = self
            .relock
            .off_since
            .is_some_and(|t| now.saturating_duration_since(t) >= RELOCK_AFTER);
        let rate_ok = self
            .relock
            .last_attempt
            .is_none_or(|t| now.saturating_duration_since(t) >= RELOCK_EVERY);
        if !off_long || !rate_ok {
            return;
        }
        let dir = self.cfg.data_dir.clone();
        if blocking(move || Ok(dir.join(BAND_RUNNER_LOCK).exists()))
            .await
            .unwrap_or(true)
        {
            return;
        }
        self.relock.last_attempt = Some(now);
        let serving_band = self.serving_lte_band().unwrap_or(0);
        let detail = serde_json::json!({
            "primary": p,
            "serving_band": serving_band,
            "bands": current.lte_bands.label(),
            "failures": self.relock.failures,
        });
        self.log_event(kinds::RELOCK_STARTED, detail).await;
        self.emit_automation(AutomationEvent::RelockStarted {
            primary: p,
            serving_band,
        })
        .await;
        let step1 = primary_only(&current, p);
        self.begin_sequence(SequenceOrigin::Relock, p, current, step1)
            .await;
    }

    fn relock_view(&self) -> Option<RelockView> {
        let a = self.cfg.automation;
        let primary = a.primary_band.filter(|_| a.relock_primary)?;
        let now = Instant::now();
        let next_try_in_s = self
            .relock
            .last_attempt
            .map(|t| (t + RELOCK_EVERY).saturating_duration_since(now))
            .filter(|d| !d.is_zero())
            .map(|d| u32::try_from(d.as_secs()).unwrap_or(u32::MAX));
        Some(RelockView {
            primary,
            failures: self.relock.failures,
            given_up: self.relock.given_up,
            next_try_in_s,
        })
    }

    async fn emit_automation(&mut self, event: AutomationEvent) {
        let _ = self.alerts.send(Alert::Automation { event });
    }

    /// Why `op` can't start now because something else holds the router. The one shared
    /// mutual-exclusion check: a band change, sequence, scan or IP refresh refuses every
    /// operation, and a speed test refuses a factory reset, a logout and an IP refresh (a reboot
    /// is still allowed during one). An open APN switch refuses everything too.
    fn busy_refusal(&self, op: Op) -> Option<RouterError> {
        if !reboot_allowed(self.sa.phase()) || self.seq.is_some() || self.scan_active {
            return Some(validation(CHANGE_IN_PROGRESS));
        }
        if self.ipr.active() {
            return Some(validation(IP_REFRESH_RUNNING));
        }
        if self.apn_active() {
            return Some(validation(APN_CHANGE_RUNNING));
        }
        let test_blocks = matches!(
            op,
            Op::FactoryReset
                | Op::Logout
                | Op::IpRefresh
                | Op::IpReattach
                | Op::ApnSwitch
                | Op::ApnEdit
                | Op::PageScripts
        );
        if self.speed_test_active() && test_blocks {
            return Some(validation(SPEED_TEST_RUNNING));
        }
        // Turning mobile data back on is never held up.
        if self.page_scripts_active() && op != Op::DataOn {
            return Some(validation(PAGE_SCRIPTS_RUNNING));
        }
        None
    }

    /// A reboot or factory reset waits for its second click.
    fn ticket_open(&self) -> bool {
        let open = |t: &Option<(String, Instant)>| {
            t.as_ref().is_some_and(|(_, exp)| Instant::now() <= *exp)
        };
        open(&self.reboot_ticket) || open(&self.factory_reset_ticket)
    }

    /// Why an APN switch to `target` can't start now, beyond `busy_refusal`.
    fn apn_switch_refusal(&self, target: &ApnTarget) -> Option<RouterError> {
        if let Some(e) = self.busy_refusal(Op::ApnSwitch) {
            return Some(e);
        }
        if self.ticket_open() {
            return Some(validation(REBOOT_PENDING));
        }
        if self.apn_journal_blocked {
            return Some(validation(APN_JOURNAL_BLOCKED));
        }
        if !self.online() {
            return Some(validation(NOT_CONNECTED));
        }
        // With data off there's no internet address, so any profile would fail its check.
        if self.data_switch_off {
            return Some(validation(DATA_SWITCH_OFF));
        }
        if !self.capabilities.as_ref().is_some_and(|c| c.apn.switch) {
            return Some(validation(APN_SWITCH_NOT_SUPPORTED));
        }
        let Some(profiles) = self.apn_profiles.as_ref() else {
            return Some(validation(APN_PROFILES_UNKNOWN));
        };
        if profiles.auto_apn == Some(true) {
            return Some(validation(APN_AUTO_ON));
        }
        check_target(profiles, target).err()
    }

    /// Starts an APN switch through safe apply. `Ok` once it has started; a refusal, including
    /// the safe-apply's own (the profile is already in use, the read failed), is the error.
    async fn apn_switch(&mut self, target: ApnTarget) -> Result<(), RouterError> {
        if let Some(e) = self.apn_switch_refusal(&target) {
            return Err(e);
        }
        let caps = self.capabilities.clone().expect("checked above");
        let current = self
            .apn_profiles
            .as_ref()
            .and_then(ApnProfiles::current_target);
        self.apn_index = Some(target.index);
        self.apn_rejected = None;
        let input = Input::Start {
            requested: target,
            caps,
            current,
            revert_to: None,
        };
        self.apn_step(input).await;
        match self.apn_rejected.take() {
            Some(message) => Err(RouterError::Validation { message }),
            None => Ok(()),
        }
    }

    /// The profile in use from the last read, when an add, edit or delete may go ahead now.
    fn apn_write_refusal(&self) -> Result<ApnTarget, RouterError> {
        if let Some(e) = self.busy_refusal(Op::ApnEdit) {
            return Err(e);
        }
        if self.ticket_open() {
            return Err(validation(REBOOT_PENDING));
        }
        if self.apn_journal_blocked {
            return Err(validation(APN_JOURNAL_BLOCKED));
        }
        if !self.online() {
            return Err(validation(NOT_CONNECTED));
        }
        if !self.capabilities.as_ref().is_some_and(|c| c.apn.edit) {
            return Err(validation(APN_EDIT_NOT_SUPPORTED));
        }
        self.apn_profiles
            .as_ref()
            .and_then(ApnProfiles::current_target)
            .ok_or_else(|| validation(APN_PROFILES_UNKNOWN))
    }

    /// One add, edit or delete. Returns the profile's index. The list is read again afterwards,
    /// and a profile in use that moved meanwhile is switched back (see the module docs).
    async fn apn_write(&mut self, write: ApnWrite) -> Result<u32, RouterError> {
        let expect = self.apn_write_refusal()?;
        let (result, kind) = match &write {
            ApnWrite::Add(draft) => (
                self.driver.add_apn_profile(&expect, draft).await,
                kinds::APN_PROFILE_ADDED,
            ),
            ApnWrite::Edit(target, draft) => (
                self.driver
                    .edit_apn_profile(&expect, target, draft)
                    .await
                    .map(|()| target.index),
                kinds::APN_PROFILE_EDITED,
            ),
            ApnWrite::Delete(target) => (
                self.driver
                    .delete_apn_profile(&expect, target)
                    .await
                    .map(|()| target.index),
                kinds::APN_PROFILE_DELETED,
            ),
        };
        drop(write);
        let result = result.map_err(normalize);
        match &result {
            Ok(index) => {
                self.log_event(kind, serde_json::json!({ "index": index }))
                    .await;
            }
            Err(e) => self.note_call_error(e).await,
        }
        let read = self.read_apn().await;
        let moved = read
            && self
                .apn_profiles
                .as_ref()
                .is_some_and(|p| p.current != Some(expect.index));
        let said_moved = matches!(
            &result,
            Err(RouterError::Validation { message }) if message == APN_ROUTER_SWITCHED
        );
        if moved || said_moved {
            self.undo_unexpected_switch(expect).await;
        }
        result
    }

    /// The router dials with another profile after a write: switch back to `previous` through
    /// safe apply, unless something refuses that (Auto APN on, the profile changed).
    async fn undo_unexpected_switch(&mut self, previous: ApnTarget) {
        let now = self
            .apn_profiles
            .as_ref()
            .and_then(|p| p.current)
            .map(serde_json::Value::from)
            .unwrap_or(serde_json::Value::Null);
        let index = previous.index;
        let refusal = self.apn_switch(previous).await.err();
        let reverting = refusal.is_none();
        let mut detail = serde_json::json!({
            "index": index,
            "now": now,
            "reverting": reverting,
        });
        if let Some(RouterError::Validation { message }) = &refusal {
            detail["message"] = message.clone().into();
        }
        self.log_event(kinds::APN_UNEXPECTED_SWITCH, detail).await;
        let _ = self.alerts.send(Alert::ApnUnexpectedSwitch { reverting });
    }

    /// Why an IP refresh can't start now, beyond `busy_refusal`.
    fn ip_refresh_refusal(&self) -> Option<RouterError> {
        if let Some(e) = self.busy_refusal(Op::IpRefresh) {
            return Some(e);
        }
        if self.ticket_open() {
            return Some(validation(REBOOT_PENDING));
        }
        if self.foreign_journal.is_some() {
            return Some(validation("a pending change for another router is on file"));
        }
        if self.journal_unreadable {
            return Some(validation("the pending-change journal can't be read"));
        }
        if self.data_journal_blocked {
            return Some(validation(DATA_JOURNAL_BLOCKED));
        }
        if !self.online() {
            return Some(validation(NOT_CONNECTED));
        }
        if self.ip_methods().is_empty() {
            return Some(validation(IP_NO_METHOD));
        }
        if !self.in_service || self.wan.as_ref().and_then(|w| w.range.as_ref()).is_none() {
            return Some(validation(IP_NOT_IN_SERVICE));
        }
        None
    }

    /// The driver's IP refresh methods, in its order, minus what the router refused this
    /// session. The data switch also needs its read.
    fn ip_methods(&self) -> Vec<IpMethod> {
        let Some(caps) = self.capabilities.as_ref() else {
            return Vec::new();
        };
        caps.ip_methods
            .iter()
            .copied()
            .filter(|m| match m {
                IpMethod::DataSwitch => {
                    caps.data_switch
                        && !self.refused.contains(DATA_SWITCH)
                        && !self.refused.contains(DATA_SWITCH_WRITE)
                }
                IpMethod::NetworkSearch => !self.refused.contains(NETWORK_SEARCH),
            })
            .collect()
    }

    async fn start_ip_refresh(
        &mut self,
        target: Option<&str>,
        max_tries: u8,
    ) -> Result<(), RouterError> {
        let target = match target.map(str::trim).filter(|t| !t.is_empty()) {
            Some(t) => Some(IpTarget::parse(t)?),
            None => None,
        };
        if let Some(e) = self.ip_refresh_refusal() {
            return Err(e);
        }
        let range = self.wan.as_ref().and_then(|w| w.range.clone());
        let methods = self.ip_methods();
        let effects = self
            .ipr
            .start(target, max_tries, range, &methods, now_std())?;
        self.ip_config_before = self.config.clone();
        self.next_ip_tick = Some(Instant::now() + IP_TICK);
        self.run_ip_effects(effects).await;
        Ok(())
    }

    /// The user's button for data that is off: one "on", then a read of the switch.
    async fn turn_data_on(&mut self) -> Result<(), RouterError> {
        if let Some(e) = self.busy_refusal(Op::DataOn) {
            return Err(e);
        }
        if self.ticket_open() {
            return Err(validation(REBOOT_PENDING));
        }
        if !self.logged_in {
            return Err(validation(NOT_CONNECTED));
        }
        let posted = self.driver.set_data_switch(true).await;
        let read = match &posted {
            Ok(()) => self.driver.data_switch().await,
            Err(e) => Err(e.clone()),
        };
        let detail = match &read {
            Ok(on) => serde_json::json!({ "ok": *on }),
            Err(e) => serde_json::json!({ "ok": false, "error": e }),
        };
        self.log_event(kinds::IP_REFRESH_MANUAL_ON, detail).await;
        match read {
            Ok(true) => {
                self.data_switch_off = false;
                Ok(())
            }
            Ok(false) => Err(validation(DATA_STILL_OFF)),
            Err(e) => {
                self.note_call_error(&e).await;
                Err(e)
            }
        }
    }

    /// At shutdown with data possibly off: one "on", at most 5 s. The record stays, so the
    /// next launch confirms the switch either way.
    async fn data_on_at_exit(&mut self) {
        if !self.ipr.data_may_be_off() || !self.logged_in {
            return;
        }
        let driver = self.driver.clone();
        let r = tokio::time::timeout(EXIT_DATA_ON_TIMEOUT, driver.set_data_switch(true))
            .await
            .unwrap_or(Err(RouterError::Timeout));
        if let Err(e) = r {
            let detail = serde_json::json!({ "attempt": 0, "at_exit": true, "error": e });
            self.log_event(kinds::IP_REFRESH_DATA_ON_FAILED, detail)
                .await;
        }
    }

    async fn ip_step(&mut self, input: IpInput) {
        let effects = self.ipr.step(input, now_std());
        self.run_ip_effects(effects).await;
    }

    async fn run_ip_effects(&mut self, effects: Vec<IpEffect>) {
        let mut queue: VecDeque<IpEffect> = effects.into();
        while let Some(effect) = queue.pop_front() {
            let input = match effect {
                IpEffect::WriteJournal { id } => Some(self.write_data_journal(id).await),
                IpEffect::PostSwitch(on) => {
                    // "Off" needs a working, in-service link; "on" only a session.
                    let can = if on {
                        self.logged_in
                    } else {
                        self.online() && self.in_service
                    };
                    if can {
                        let r = self.driver.set_data_switch(on).await.map_err(normalize);
                        match &r {
                            // A refused "off" is the router saying no to this call, not a lost
                            // session: the run may go on with a search.
                            Err(e) if !on => self.note_refused_action(DATA_SWITCH_WRITE, e).await,
                            Err(e) => self.note_call_error(e).await,
                            Ok(()) => {}
                        }
                        Some(IpInput::PostResult(r))
                    } else {
                        Some(IpInput::Deferred)
                    }
                }
                IpEffect::ReadSwitch => {
                    if self.logged_in {
                        let r = self.driver.data_switch().await.map_err(normalize);
                        match &r {
                            Ok(true) => self.data_switch_off = false,
                            Ok(false) => {}
                            Err(e) => self.note_call_error(e).await,
                        }
                        Some(IpInput::SwitchRead(r))
                    } else {
                        Some(IpInput::Deferred)
                    }
                }
                IpEffect::ReadWan => {
                    if self.logged_in {
                        self.last_wan = Some(Instant::now());
                        let r = self.driver.wan_address().await;
                        let ok = r.is_ok();
                        self.note_wan(r).await;
                        // After a search, "back" also needs service: the router reconnects by
                        // itself and may report an address from before for a moment.
                        let back = ok
                            && (self.ipr.method() != Some(IpMethod::NetworkSearch)
                                || self.in_service);
                        let range = self
                            .wan
                            .as_ref()
                            .and_then(|w| w.range.clone())
                            .filter(|_| back);
                        Some(IpInput::WanRead(range))
                    } else {
                        Some(IpInput::Deferred)
                    }
                }
                IpEffect::StartSearch => {
                    if self.online()
                        && due(self.session_refreshed, SEARCH_REFRESH_AFTER, Instant::now())
                    {
                        self.refresh_session().await;
                    }
                    if self.online() && self.spawn_search() {
                        Some(IpInput::SearchSent)
                    } else {
                        Some(IpInput::Deferred)
                    }
                }
                IpEffect::ArchiveJournal => {
                    self.archive_data_journal().await;
                    None
                }
                IpEffect::Event(e) => {
                    self.ip_event(e).await;
                    None
                }
            };
            if let Some(input) = input {
                let follow = self.ipr.step(input, now_std());
                for e in follow.into_iter().rev() {
                    queue.push_front(e);
                }
            }
        }
    }

    /// Starts the network search in a task of its own; its result comes back through `run`'s
    /// channel. Whether it started.
    fn spawn_search(&mut self) -> bool {
        let Some(tx) = self.search_tx.clone() else {
            return false;
        };
        let driver = self.driver.clone();
        self.search_service_dropped = false;
        let task = tokio::spawn(async move {
            let r = tokio::time::timeout(SEARCH_CALL_LIMIT, driver.network_search())
                .await
                .unwrap_or(Err(RouterError::Timeout))
                .map_err(normalize);
            let _ = tx.send(r).await;
        });
        self.search_task = Some(task.abort_handle());
        true
    }

    /// The network search task answered.
    async fn on_search_done(&mut self, result: Result<NetworkSearch, RouterError>) {
        self.search_task = None;
        let service_dropped = std::mem::take(&mut self.search_service_dropped);
        if let Err(e) = &result {
            // A lapsed session or token (no code, or 125001 to 125003) is the normal re-login;
            // any other refusal is handled when the run ends.
            if crate::ip_refresh::is_token_loss(e) {
                self.note_call_error(e).await;
            }
        }
        self.ip_step(IpInput::SearchDone {
            result,
            service_dropped,
        })
        .await;
    }

    /// `pending-data-switch.json`, fsynced before the "off" POST.
    async fn write_data_journal(&mut self, id: String) -> IpInput {
        let rec = DataSwitchRecord {
            version: DATA_SWITCH_VERSION,
            id,
            host: self.cfg.host.clone(),
            driver: self.driver_id.clone(),
            created_at_ms: wall_ms(),
            phase: DataSwitchPhase::Off,
        };
        let dir = self.cfg.data_dir.clone();
        let to_write = rec.clone();
        let written = blocking(move || DataSwitchStore::new(dir).write(&to_write)).await;
        match written {
            Ok(()) => {
                self.data_journal = Some(rec);
                IpInput::JournalWritten(true)
            }
            Err(e) => {
                let detail = serde_json::json!({ "file": "data_switch", "error": e.to_string() });
                self.log_event(kinds::JOURNAL_WRITE_FAILED, detail).await;
                IpInput::JournalWritten(false)
            }
        }
    }

    async fn archive_data_journal(&mut self) {
        let Some(rec) = self.data_journal.take() else {
            return;
        };
        let dir = self.cfg.data_dir.clone();
        if let Err(e) = blocking(move || DataSwitchStore::new(dir).archive(&rec)).await {
            let detail = serde_json::json!({ "file": "data_switch", "error": e.to_string() });
            self.log_event(kinds::JOURNAL_ARCHIVE_FAILED, detail).await;
        }
    }

    async fn ip_event(&mut self, e: IpRefreshEvent) {
        match e {
            IpRefreshEvent::Started {
                target,
                max_tries,
                from,
                method,
            } => {
                let detail = serde_json::json!({
                    "target": target,
                    "max_tries": max_tries,
                    "from": from,
                    "method": method,
                });
                self.log_event(kinds::IP_REFRESH_STARTED, detail).await;
            }
            IpRefreshEvent::Search {
                n,
                seconds,
                networks,
                timed_out,
                service_dropped,
                error,
            } => {
                let detail = serde_json::json!({
                    "n": n,
                    "seconds": seconds,
                    "networks": networks,
                    "timed_out": timed_out,
                    "service_dropped": service_dropped,
                    "error": error,
                });
                self.log_event(kinds::IP_REFRESH_SEARCH, detail).await;
            }
            IpRefreshEvent::MethodFallback { from, to, error } => {
                let detail = serde_json::json!({ "from": from, "to": to, "error": error });
                self.log_event(kinds::IP_REFRESH_METHOD_FALLBACK, detail)
                    .await;
            }
            IpRefreshEvent::Try {
                n,
                from,
                to,
                off_s,
                method,
            } => {
                let detail = serde_json::json!({
                    "n": n,
                    "from": from,
                    "to": to,
                    "off_s": off_s,
                    "method": method,
                });
                self.log_event(kinds::IP_REFRESH_TRY, detail).await;
            }
            IpRefreshEvent::DataOnFailed { attempt, error } => {
                let detail = serde_json::json!({ "attempt": attempt, "error": error });
                self.log_event(kinds::IP_REFRESH_DATA_ON_FAILED, detail)
                    .await;
                // Once at the first failure and again when the retries slow down.
                if attempt == 1 || attempt == 5 {
                    let event = IpRefreshAlert::DataOnFailed { attempt };
                    let _ = self.alerts.send(Alert::IpRefresh { event });
                }
            }
            IpRefreshEvent::Ended(mut result) => {
                if result.outcome == IpRefreshOutcome::SearchRefused {
                    if let Some(e) = result.refusal.clone() {
                        self.note_refused_action(NETWORK_SEARCH, &e).await;
                    }
                }
                let before = self.ip_config_before.take();
                if result.method == IpMethod::NetworkSearch
                    && result.outcome != IpRefreshOutcome::SearchRefused
                {
                    if let Some(before) = before {
                        self.check_bands_after_search(before, &mut result).await;
                    }
                }
                let kind = if result.outcome == IpRefreshOutcome::Resumed {
                    kinds::IP_REFRESH_RESUMED
                } else {
                    kinds::IP_REFRESH_ENDED
                };
                let detail = serde_json::json!({
                    "outcome": result.outcome,
                    "tries": result.tries,
                    "target": result.target,
                    "from": result.from,
                    "to": result.to,
                    "method": result.method,
                    "refusal": result.refusal,
                    "bands_changed": result.bands_changed,
                });
                self.log_event(kind, detail).await;
                let _ = self.alerts.send(Alert::IpRefresh {
                    event: IpRefreshAlert::Ended { result },
                });
            }
        }
    }

    /// After a run that searched: one `net-mode` read. A config that differs from the one at the
    /// start is logged and kept in the result; nothing is changed back by itself.
    async fn check_bands_after_search(&mut self, before: NetConfig, result: &mut IpRefreshResult) {
        if !self.logged_in {
            return;
        }
        let after = match self.driver.net_config().await {
            Ok(c) => c,
            Err(e) => {
                self.note_call_error(&e).await;
                return;
            }
        };
        self.config = Some(after.clone());
        self.last_net_config = Some(Instant::now());
        if after == before {
            return;
        }
        let detail = serde_json::json!({ "before": before, "after": after });
        self.log_event(kinds::IP_REFRESH_BANDS_CHANGED, detail)
            .await;
        self.ipr.note_bands_changed(before.clone());
        result.bands_changed = true;
        result.bands_before = Some(before);
    }

    fn request_reboot(&mut self) -> Result<RebootTicket, RouterError> {
        // Between the steps of a primary-band apply the change isn't finished either.
        if let Some(e) = self.busy_refusal(Op::Reboot) {
            return Err(e);
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.reboot_ticket = Some((id.clone(), Instant::now() + REBOOT_TICKET_TTL));
        Ok(RebootTicket {
            id,
            expires_in_s: REBOOT_TICKET_TTL.as_secs() as u32,
        })
    }

    async fn confirm_reboot(&mut self, id: &str) -> Result<(), RouterError> {
        let now = Instant::now();
        if self
            .reboot_ticket
            .as_ref()
            .is_some_and(|(_, exp)| now > *exp)
        {
            self.reboot_ticket = None;
        }
        match &self.reboot_ticket {
            Some((t, _)) if t == id => {}
            _ => return Err(validation("the reboot confirmation is invalid or expired")),
        }
        self.reboot_ticket = None;
        if let Some(e) = self.busy_refusal(Op::Reboot) {
            return Err(e);
        }
        if !self.logged_in {
            return Err(RouterError::SessionLost { code: None });
        }
        match self.driver.reboot().await {
            Ok(()) => {
                self.log_event(kinds::DEVICE_REBOOT, serde_json::json!({}))
                    .await;
                self.logged_in = false;
                self.poll_failures = 0;
                self.relogin_failures = 0;
                self.in_service = false;
                self.last_net_config = None;
                self.last_heartbeat = None;
                self.schedule_attempt(REBOOT_RECONNECT_AFTER);
                self.set_session(SessionView::Reconnecting {
                    reason: "rebooting".to_string(),
                    retry_in_s: REBOOT_RECONNECT_AFTER.as_secs() as u32,
                })
                .await;
                Ok(())
            }
            Err(e) => {
                self.note_call_error(&e).await;
                Err(e)
            }
        }
    }

    /// Why a factory reset can't run now. Checked at both steps, before the driver is called.
    fn factory_reset_refusal(&self) -> Option<RouterError> {
        if !self
            .capabilities
            .as_ref()
            .is_some_and(|c| c.factory_reset_verified)
        {
            return Some(validation(FACTORY_RESET_NOT_VERIFIED));
        }
        if let Some(e) = self.busy_refusal(Op::FactoryReset) {
            return Some(e);
        }
        if !self.logged_in {
            return Some(RouterError::SessionLost { code: None });
        }
        None
    }

    fn request_factory_reset(&mut self) -> Result<RebootTicket, RouterError> {
        if let Some(e) = self.factory_reset_refusal() {
            return Err(e);
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.factory_reset_ticket = Some((id.clone(), Instant::now() + REBOOT_TICKET_TTL));
        Ok(RebootTicket {
            id,
            expires_in_s: REBOOT_TICKET_TTL.as_secs() as u32,
        })
    }

    async fn confirm_factory_reset(&mut self, id: &str) -> Result<(), RouterError> {
        // One use, whatever happens next: a refused or failed reset needs a new confirmation.
        match self.factory_reset_ticket.take() {
            Some((t, exp)) if t == id && Instant::now() <= exp => {}
            _ => {
                return Err(validation(
                    "the factory reset confirmation is invalid or expired",
                ))
            }
        }
        if let Some(e) = self.factory_reset_refusal() {
            return Err(e);
        }
        match self.driver.factory_reset().await {
            Ok(()) => {
                self.log_event(kinds::DEVICE_FACTORY_RESET, serde_json::json!({}))
                    .await;
                self.logged_in = false;
                self.poll_failures = 0;
                self.relogin_failures = 0;
                self.in_service = false;
                self.last_net_config = None;
                self.last_heartbeat = None;
                // The saved password no longer matches: never try it against the reset router.
                self.creds = None;
                self.verified_this_run = false;
                crate::identity_file::remove(&self.cfg.data_dir);
                self.set_session(SessionView::NotConfigured).await;
                Ok(())
            }
            Err(e) => {
                self.note_call_error(&e).await;
                Err(e)
            }
        }
    }

    // --- session -----------------------------------------------------------------------------

    /// Something the app must keep watching, which a logout marker doesn't stop.
    fn needs_watching(&self) -> bool {
        self.ipr.active()
            || self.change_active()
            || self.apn_active()
            || self.apn_journal_blocked
            || self.seq.is_some()
            || self.foreign_journal.is_some()
            || self.journal_unreadable
    }

    async fn logout(&mut self) -> Result<LogoutOutcome, RouterError> {
        if let Some(e) = self.busy_refusal(Op::Logout) {
            return Err(e);
        }
        // On disk first, so a crash right after still keeps the next launch logged out.
        if let Some(user) = self.account_user.clone() {
            let dir = self.cfg.data_dir.clone();
            let host = self.cfg.host.clone();
            let at = wall_ms();
            let saved = blocking(move || crate::logout_marker::mark(&dir, &host, &user, at)).await;
            if let Err(e) = saved {
                let detail = serde_json::json!({ "error": e.to_string() });
                self.log_event(kinds::SESSION_LOGOUT_MARKER_NOT_SAVED, detail)
                    .await;
            }
        }
        let mut detail = serde_json::json!({ "ok": true });
        if self.logged_in {
            let driver = self.driver.clone();
            let answer = tokio::time::timeout(LOGOUT_TIMEOUT, driver.logout())
                .await
                .unwrap_or(Err(RouterError::Timeout));
            match answer {
                Ok(()) => {
                    let state = tokio::time::timeout(LOGOUT_TIMEOUT, driver.session_state())
                        .await
                        .ok()
                        .and_then(Result::ok);
                    let still_in = state == Some(SessionState::LoggedIn);
                    detail["ok"] = (!still_in).into();
                    if let Some(state) = state {
                        detail["router_state"] = router_state_name(state).into();
                    }
                }
                Err(e) => {
                    detail["ok"] = false.into();
                    detail["error"] = serde_json::to_value(&e).unwrap_or_default();
                }
            }
        }
        self.driver.forget_session();
        self.forget_session_state();
        let confirmed = detail["ok"].as_bool().unwrap_or(false);
        self.log_event(kinds::SESSION_LOGOUT, detail).await;
        self.set_session(SessionView::LoggedOut).await;
        Ok(LogoutOutcome { confirmed })
    }

    /// Drops the login and everything read in it, so no stale band or signal is shown and no
    /// timer sends anything until the next Connect.
    fn forget_session_state(&mut self) {
        self.creds = None;
        self.next_attempt = None;
        self.logged_in = false;
        self.poll_failures = 0;
        self.relogin_failures = 0;
        self.in_service = false;
        self.signal = None;
        self.status = None;
        self.config = None;
        self.capabilities = None;
        self.device = None;
        self.wan = None;
        self.wan_read_at = None;
        self.last_wan = None;
        self.apn_profiles = None;
        self.last_apn = None;
        self.last_net_config = None;
        self.last_heartbeat = None;
        self.heartbeat_failure_logged = false;
        self.session_refreshed = None;
        self.last_capabilities_try = None;
        self.reboot_ticket = None;
        self.factory_reset_ticket = None;
        self.cells = CellTracker::default();
    }

    /// Whether this Connect goes ahead: a user's Connect clears the `logged-out` marker, and a
    /// launch auto-connect respects it unless something needs watching.
    async fn connect_allowed(&mut self, creds: &Credentials, trust: LoginTrust) -> bool {
        let dir = self.cfg.data_dir.clone();
        if trust != LoginTrust::Automatic {
            let _ = blocking(move || {
                crate::logout_marker::clear(&dir);
                Ok(())
            })
            .await;
            return true;
        }
        let host = self.cfg.host.clone();
        let user = creds.username.clone();
        let marked = blocking(move || Ok(crate::logout_marker::is_marked(&dir, &host, &user)))
            .await
            .unwrap_or(false);
        if !marked {
            return true;
        }
        if self.needs_watching() {
            let detail = serde_json::json!({ "why": "change_open" });
            self.log_event(kinds::SESSION_LOGOUT_OVERRIDDEN, detail)
                .await;
            return true;
        }
        let detail = serde_json::json!({ "why": "logged_out" });
        self.log_event(kinds::SESSION_AUTO_CONNECT_SKIPPED, detail)
            .await;
        false
    }

    async fn connect(&mut self, creds: Credentials, trust: LoginTrust) {
        self.account_user = Some(creds.username.clone());
        if !self.connect_allowed(&creds, trust).await {
            self.set_session(SessionView::LoggedOut).await;
            return;
        }
        self.creds = Some(creds);
        self.login_trust = trust;
        self.verified_this_run = false;
        self.relogin_failures = 0;
        self.refused.clear();
        self.poll_failures = 0;
        self.next_attempt = None;
        self.set_session(SessionView::Connecting).await;
        self.try_login().await;
    }

    /// Whether this login may send its proof to the router it finds.
    ///
    /// Once this run has verified the router, later logins don't ask: a salt that changes under
    /// a running app mustn't leave a band change that can't log back in to revert.
    fn login_gate(&self, creds: &Credentials) -> LoginGate {
        if self.verified_this_run || self.login_trust == LoginTrust::UserTrusted {
            return LoginGate::Open;
        }
        match crate::identity_file::load(&self.cfg.data_dir, &self.cfg.host, &creds.username) {
            Some(known) => LoginGate::Expect(known),
            None if self.login_trust == LoginTrust::UserAsked => LoginGate::Open,
            None => LoginGate::Ask,
        }
    }

    /// A login whose server signature verified: this is the router from now on.
    async fn note_verified(&mut self, creds: &Credentials, seen: Option<RouterIdentity>) {
        let Some(seen) = seen else { return };
        self.verified_this_run = true;
        self.login_trust = LoginTrust::UserAsked;
        let saved =
            crate::identity_file::save(&self.cfg.data_dir, &self.cfg.host, &creds.username, &seen);
        if let Err(e) = saved {
            let detail = serde_json::json!({ "error": e.to_string() });
            self.log_event(kinds::SESSION_IDENTITY_NOT_SAVED, detail)
                .await;
        }
    }

    async fn try_login(&mut self) {
        let Some(creds) = self.creds.clone() else {
            return;
        };
        let gate = self.login_gate(&creds);
        match self.driver.login_gated(&creds, &gate).await {
            Ok(seen) => {
                self.note_verified(&creds, seen).await;
                self.logged_in = true;
                // `relogin_failures` is reset in `session_lost` once the session proved stable.
                self.logged_in_at = Some(Instant::now());
                self.poll_failures = 0;
                self.session_refreshed = Some(Instant::now());
                self.heartbeat_failure_logged = false;
                self.set_session(SessionView::LoggedIn).await;
                match self.driver.device_info().await {
                    Ok(d) => self.device = Some(d),
                    Err(e) => self.note_call_error(&e).await,
                }
                if self.logged_in {
                    self.fetch_capabilities().await;
                }
                if self.logged_in {
                    self.check_data_switch().await;
                }
                if self.logged_in && !self.refused.contains(WAN) {
                    self.last_wan = Some(Instant::now());
                    let r = self.driver.wan_address().await;
                    self.note_wan(r).await;
                }
                if self.logged_in {
                    self.read_apn().await;
                }
                if self.logged_in {
                    self.poll().await;
                }
            }
            Err(e) => {
                if let Some(view) = login_stop_view(&e) {
                    self.creds = None;
                    self.next_attempt = None;
                    // On disk before anyone hears of the refusal, so no Connect can resend it.
                    if crate::password_marker::rejects_password(&view) {
                        let dir = self.cfg.data_dir.clone();
                        let host = self.cfg.host.clone();
                        let user = creds.username.clone();
                        let saved =
                            blocking(move || crate::password_marker::mark(&dir, &host, &user))
                                .await;
                        if let Err(err) = saved {
                            let detail = serde_json::json!({ "error": err.to_string() });
                            self.log_event(kinds::SESSION_MARKER_NOT_SAVED, detail)
                                .await;
                        }
                    }
                    self.set_session(view).await;
                } else if is_reachability(&e) {
                    self.poll_failures += 1;
                    let delay = self.backoff_interval();
                    self.schedule_attempt(delay);
                    self.set_session(reconnecting(&e, delay)).await;
                } else {
                    self.relogin_failures += 1;
                    let delay = self.relogin_delay();
                    self.schedule_attempt(delay);
                    let view = if e == RouterError::SessionTakenOver {
                        SessionView::TakenOver
                    } else {
                        reconnecting(&e, delay)
                    };
                    self.set_session(view).await;
                }
                self.step(Input::SessionUnavailable).await;
                self.apn_step(Input::SessionUnavailable).await;
                self.ip_step(IpInput::SessionUnavailable).await;
            }
        }
    }

    /// At login: a switch that reads off with no run of ours to explain it gets a notice.
    async fn check_data_switch(&mut self) {
        if self.ipr.active()
            || self.refused.contains(DATA_SWITCH)
            || !self.capabilities.as_ref().is_some_and(|c| c.data_switch)
        {
            return;
        }
        match self.driver.data_switch().await {
            Ok(on) => self.data_switch_off = !on,
            Err(e) => self.note_optional_error(DATA_SWITCH, &e).await,
        }
    }

    /// The router dropped our session: log in again on the re-login schedule.
    async fn session_lost(&mut self, e: &RouterError) {
        if !self.logged_in {
            return;
        }
        self.logged_in = false;
        let stable = self
            .logged_in_at
            .is_some_and(|at| at.elapsed() >= STABLE_SESSION);
        if stable {
            self.relogin_failures = 0;
        } else {
            self.relogin_failures = self.relogin_failures.saturating_add(1);
        }
        let delay = self.relogin_delay();
        self.schedule_attempt(delay);
        let view = if *e == RouterError::SessionTakenOver {
            SessionView::TakenOver
        } else {
            reconnecting(e, delay)
        };
        self.set_session(view).await;
        self.log_event(kinds::SESSION_LOST, serde_json::json!({ "error": e }))
            .await;
    }

    /// A failed optional read (`what` is APN, WAN or data switch). "No rights" or a token
    /// error there means the router won't serve that page to us, not that the session is gone:
    /// the signal poll would see a real loss within seconds. So the feature goes off until the
    /// next Connect, logged once, and the session stays. A takeover is still a lost session.
    async fn note_optional_error(&mut self, what: &'static str, e: &RouterError) {
        if is_session_loss(e) && *e != RouterError::SessionTakenOver {
            if self.refused.insert(what) {
                let detail = serde_json::json!({ "what": what, "error": e });
                self.log_event(kinds::FEATURE_REFUSED, detail).await;
            }
        } else {
            self.note_call_error(e).await;
        }
    }

    /// A refused action of an optional feature (`what` is `data_switch_write` or
    /// `network_search`): a session-type refusal as in `note_optional_error`, and also any API
    /// refusal but "busy", turns the feature off until the next Connect. Anything else is an
    /// ordinary failed call.
    async fn note_refused_action(&mut self, what: &'static str, e: &RouterError) {
        let api_refusal = matches!(
            e,
            RouterError::Api { kind, .. } if *kind != ApiErrorKind::Busy
        );
        if api_refusal {
            if self.refused.insert(what) {
                let detail = serde_json::json!({ "what": what, "error": e });
                self.log_event(kinds::FEATURE_REFUSED, detail).await;
            }
        } else {
            self.note_optional_error(what, e).await;
        }
    }

    /// Bookkeeping for a failed driver call outside the signal/status poll.
    async fn note_call_error(&mut self, e: &RouterError) {
        if is_session_loss(e) {
            self.session_lost(e).await;
        } else if is_reachability(e) {
            self.poll_failures += 1;
        }
    }

    async fn set_session(&mut self, view: SessionView) {
        let changed_kind = std::mem::discriminant(&view) != std::mem::discriminant(&self.session);
        self.session = view.clone();
        if changed_kind {
            let detail = serde_json::to_value(&view).unwrap_or_default();
            self.log_event(kinds::SESSION, detail).await;
            let _ = self.alerts.send(Alert::Session { view });
        }
    }

    // --- polling -----------------------------------------------------------------------------

    async fn poll(&mut self) {
        self.poll_with(None).await;
    }

    /// Runs a read-only router call. With `commands`, an incoming command wins: the read is
    /// dropped (it changes nothing on the router) and the command is kept in `preempted` for
    /// the main loop, so Keep/Revert never wait behind a slow router. Returns `None` then.
    /// Writes, logins and safe-apply effects never go through here.
    async fn read<T, F, Fut>(
        &mut self,
        commands: Commands<'_>,
        f: F,
    ) -> Option<Result<T, RouterError>>
    where
        F: FnOnce(Arc<dyn RouterDriver>) -> Fut,
        Fut: Future<Output = Result<T, RouterError>>,
    {
        if self.preempted.is_some() {
            return None;
        }
        let fut = f(self.driver.clone());
        let Some(commands) = commands else {
            return Some(fut.await);
        };
        tokio::select! {
            biased;
            r = fut => Some(r),
            cmd = commands.recv() => {
                self.preempted = Some(cmd.unwrap_or(Command::Shutdown));
                None
            }
        }
    }

    async fn poll_with(&mut self, mut commands: Commands<'_>) {
        let polled_at = Instant::now();
        let read = self
            .read(commands.as_deref_mut(), |d| async move {
                let sig = d.signal().await?;
                let st = d.status().await?;
                Ok((sig, st))
            })
            .await;
        let Some(result) = read else {
            // Interrupted: poll again right after the command.
            self.next_attempt = Some(Instant::now());
            return;
        };
        match result {
            Ok((sig, st)) => {
                self.poll_failures = 0;
                if self.session != SessionView::LoggedIn {
                    self.set_session(SessionView::LoggedIn).await;
                }
                let up = in_service(&st, &sig);
                if !up && self.ipr.searching() {
                    self.search_service_dropped = true;
                }
                let change = self.cells.observe(cell_key(&sig));
                self.signal = Some(sig);
                self.status = Some(st);
                self.in_service = up;
                if let Some((from, to)) = change {
                    self.cell_changed(from, to).await;
                }
                let lte = self
                    .signal
                    .as_ref()
                    .and_then(|s| s.lte.as_ref())
                    .and_then(|l| l.band.map(|b| (b, l.pci)));
                // The last WAN read; the band change ignores it.
                let wan_ip_present = self.wan_evidence(Instant::now());
                self.step(Input::Observation {
                    in_service: up,
                    polled_at: Some(polled_at.into_std()),
                    lte,
                    wan_ip_present,
                })
                .await;
                self.apn_step(Input::Observation {
                    in_service: up,
                    polled_at: Some(polled_at.into_std()),
                    lte,
                    wan_ip_present,
                })
                .await;
                self.track_primary().await;
                self.poll_extras(commands).await;
            }
            Err(e) => {
                if is_session_loss(&e) {
                    self.session_lost(&e).await;
                } else if is_reachability(&e) && self.ipr.searching() {
                    // The router is busy searching: no offline notice, no back-off.
                } else {
                    self.poll_failures += 1;
                    let delay = self.backoff_interval();
                    self.set_session(reconnecting(&e, delay)).await;
                }
                // Any failed poll (API, parse, anything) is "no evidence of service".
                self.step(Input::SessionUnavailable).await;
                self.apn_step(Input::SessionUnavailable).await;
                self.ip_step(IpInput::SessionUnavailable).await;
            }
        }
        if self.logged_in {
            let delay = self.backoff_interval();
            self.schedule_attempt(delay);
        }
    }

    /// One capabilities fetch; a failure leaves them unknown for a later retry.
    async fn fetch_capabilities(&mut self) {
        self.last_capabilities_try = Some(Instant::now());
        match self.driver.capabilities().await {
            Ok(c) => self.capabilities = Some(c),
            Err(e) => self.note_call_error(&e).await,
        }
    }

    /// `net-mode` every 10 s and `heartbeat` every 5 s after a successful poll, plus a
    /// capabilities retry (every 10 s) while they are still unknown.
    /// Reads stop at the first interruption; the next poll catches up.
    async fn poll_extras(&mut self, mut commands: Commands<'_>) {
        let now = Instant::now();
        if self.logged_in
            && self.capabilities.is_none()
            && due(self.last_capabilities_try, CAPABILITIES_RETRY, now)
        {
            self.last_capabilities_try = Some(now);
            let Some(r) = self
                .read(commands.as_deref_mut(), |d| async move {
                    d.capabilities().await
                })
                .await
            else {
                return;
            };
            match r {
                Ok(c) => self.capabilities = Some(c),
                Err(e) => self.note_call_error(&e).await,
            }
        }
        // A search keeps the router busy: only the heartbeat and the session refresh go on.
        let searching = self.ipr.searching();
        if self.logged_in && !searching && due(self.last_net_config, NET_CONFIG_EVERY, now) {
            self.last_net_config = Some(now);
            let Some(r) = self
                .read(
                    commands.as_deref_mut(),
                    |d| async move { d.net_config().await },
                )
                .await
            else {
                return;
            };
            match r {
                Ok(c) => self.config = Some(c),
                Err(e) => self.note_call_error(&e).await,
            }
        }
        let wan_every = if self.reattach_wan_window() {
            REATTACH_WAN_EVERY
        } else if self.apn_active() {
            APN_WAN_EVERY
        } else {
            WAN_EVERY
        };
        if self.logged_in
            && !searching
            && !self.refused.contains(WAN)
            && due(self.last_wan, wan_every, now)
        {
            self.last_wan = Some(now);
            let Some(r) = self
                .read(
                    commands.as_deref_mut(),
                    |d| async move { d.wan_address().await },
                )
                .await
            else {
                return;
            };
            self.note_wan(r).await;
        }
        if self.logged_in
            && !searching
            && self.visible
            && self.apn_readable()
            && due(self.last_apn, APN_EVERY, now)
        {
            self.last_apn = Some(now);
            let Some(r) = self
                .read(commands.as_deref_mut(), |d| async move {
                    d.apn_profiles().await
                })
                .await
            else {
                return;
            };
            self.note_apn(r).await;
        }
        if self.logged_in && due(self.last_heartbeat, HEARTBEAT_EVERY, now) {
            self.last_heartbeat = Some(now);
            let Some(r) = self
                .read(commands, |d| async move { d.heartbeat().await })
                .await
            else {
                return;
            };
            if let Err(e) = r {
                if !self.heartbeat_failure_logged {
                    self.heartbeat_failure_logged = true;
                    self.log_event(kinds::HEARTBEAT_FAILED, serde_json::json!({ "error": e }))
                        .await;
                }
                self.note_call_error(&e).await;
            }
        }
        if self.logged_in && due(self.session_refreshed, SESSION_REFRESH_AFTER, now) {
            self.refresh_session().await;
        }
    }

    /// The driver says it can read APN profiles.
    fn apn_readable(&self) -> bool {
        !self.refused.contains(APN) && self.capabilities.as_ref().is_some_and(|c| c.apn.read)
    }

    /// One APN read now, when the driver has one. Whether it read.
    async fn read_apn(&mut self) -> bool {
        if !self.apn_readable() {
            return false;
        }
        self.last_apn = Some(Instant::now());
        let r = self.driver.apn_profiles().await;
        self.note_apn(r).await
    }

    /// Takes in one APN read. A failure keeps the last list; nothing of it is logged.
    async fn note_apn(&mut self, r: Result<ApnProfiles, RouterError>) -> bool {
        match r {
            Ok(p) => {
                self.apn_profiles = Some(p);
                true
            }
            Err(e) => {
                self.note_optional_error(APN, &e).await;
                false
            }
        }
    }

    /// Takes in one WAN read. A driver without the read leaves the address unknown. A new range
    /// is logged with both ranges only, never an address.
    async fn note_wan(&mut self, r: Result<WanAddress, RouterError>) {
        let addr = match r {
            Ok(a) => a,
            Err(e) => {
                self.note_optional_error(WAN, &e).await;
                return;
            }
        };
        let view = WanView::new(addr.ipv4, addr.ipv6_present);
        if let Some(to) = view.range.clone() {
            if let Some(from) = self.last_wan_range.replace(to.clone()) {
                if from != to {
                    let detail = serde_json::json!({ "from": from, "to": to });
                    self.log_event(kinds::WAN_RANGE_CHANGED, detail).await;
                }
            }
        }
        let range = view.range.clone();
        self.wan = Some(view);
        self.wan_read_at = Some(Instant::now());
        self.wan_read_since_apn_post = true;
        if range.is_some() && self.reattach.finishing.is_some() {
            self.finish_reattach(range).await;
        }
    }

    /// Whether the router has an internet address, as evidence for a change. While an APN
    /// switch is open, only a read after its POST and no older than a few APN reads counts:
    /// an older or failed read is "no address", so a profile that gives no data can't settle
    /// on the address from before the switch.
    fn wan_evidence(&self, now: Instant) -> Option<bool> {
        let present = self.wan.as_ref().map(|w| w.ipv4.is_some());
        if !self.apn_active() {
            return present;
        }
        let fresh = self.wan_read_since_apn_post
            && self
                .wan_read_at
                .is_some_and(|at| now.saturating_duration_since(at) <= APN_WAN_EVERY * 3);
        if fresh {
            present
        } else {
            Some(false)
        }
    }

    /// Repeats the connect step's reads (login state, device info, capabilities) without
    /// touching the session view. No password is sent while the router still reports us
    /// logged in; otherwise this is an ordinary login with the saved password.
    async fn refresh_session(&mut self) {
        self.session_refreshed = Some(Instant::now());
        let Some(creds) = self.creds.clone() else {
            return;
        };
        let gate = self.login_gate(&creds);
        let mut result = match self.driver.login_gated(&creds, &gate).await {
            Ok(seen) => {
                self.note_verified(&creds, seen).await;
                Ok(())
            }
            Err(e) => Err(e),
        };
        if result.is_ok() {
            result = match self.driver.device_info().await {
                Ok(d) => {
                    self.device = Some(d);
                    Ok(())
                }
                Err(e) => Err(e),
            };
        }
        if result.is_ok() {
            result = match self.driver.capabilities().await {
                Ok(c) => {
                    self.capabilities = Some(c);
                    Ok(())
                }
                Err(e) => Err(e),
            };
        }
        let detail = match &result {
            Ok(()) => serde_json::json!({ "ok": true }),
            Err(e) => serde_json::json!({ "ok": false, "error": e }),
        };
        self.log_event(kinds::SESSION_REFRESHED, detail).await;
        if let Err(e) = result {
            if let Some(view) = login_stop_view(&e) {
                self.logged_in = false;
                self.creds = None;
                self.next_attempt = None;
                self.set_session(view).await;
                self.step(Input::SessionUnavailable).await;
                self.apn_step(Input::SessionUnavailable).await;
            } else {
                self.note_call_error(&e).await;
            }
        }
    }

    async fn cell_changed(&mut self, from: CellKey, to: CellKey) {
        let detail = serde_json::json!({ "from": from.for_log(), "to": to.for_log() });
        self.log_event(kinds::CELL_CHANGED, detail).await;
        if self.cfg.cell_alerts {
            let _ = self.alerts.send(Alert::CellChanged { from, to });
        }
    }

    // --- safe-apply effects ------------------------------------------------------------------

    async fn step(&mut self, input: Input) {
        let effects = self.sa.step(input, now_std());
        self.run_effects(effects).await;
    }

    async fn run_effects(&mut self, effects: Vec<Effect>) {
        let mut queue: VecDeque<Effect> = effects.into();
        while let Some(effect) = queue.pop_front() {
            let follow = match effect {
                Effect::ReadConfig => {
                    let input = self.read_config().await;
                    self.sa.step(input, now_std())
                }
                Effect::Post(cfg) => {
                    let input = self.post(&cfg).await;
                    self.sa.step(input, now_std())
                }
                Effect::WriteJournal(phase) => {
                    if self.write_journal(phase).await || phase != JournalPhase::Journaled {
                        vec![]
                    } else {
                        // No journal, no POST: drop it and close the change as not applied.
                        if matches!(queue.front(), Some(Effect::Post(_))) {
                            queue.pop_front();
                        }
                        let err = validation("could not write the pending-change journal");
                        self.sa.step(Input::PostResult(Err(err)), now_std())
                    }
                }
                Effect::ArchiveJournal => {
                    self.archive_journal().await;
                    vec![]
                }
                Effect::Event(event) => {
                    self.emit_safe_event(event).await;
                    vec![]
                }
                Effect::RetryAfter(d) => {
                    self.retry_ticks.push(Instant::now() + d);
                    vec![]
                }
                Effect::LossIgnored(reason) => {
                    let detail = serde_json::json!({ "reason": reason, "revert_on_loss": false });
                    self.log_event(kinds::SAFE_APPLY_LOSS_IGNORED, detail).await;
                    self.emit_automation(AutomationEvent::LossIgnored { reason })
                        .await;
                    vec![]
                }
            };
            for e in follow.into_iter().rev() {
                queue.push_front(e);
            }
        }
    }

    async fn read_config(&mut self) -> Input {
        if !self.online() {
            return Input::SessionUnavailable;
        }
        let snapshotting = *self.sa.phase() == Phase::Snapshotting;
        let result = self.driver.net_config().await.map_err(normalize);
        let detail = match &result {
            Ok(c) => serde_json::json!({ "phase": self.sa.phase(), "config": c }),
            Err(e) => serde_json::json!({ "phase": self.sa.phase(), "error": e }),
        };
        self.log_event(kinds::SAFE_APPLY_READ, detail).await;
        match &result {
            Ok(c) => {
                self.config = Some(c.clone());
                self.last_net_config = Some(Instant::now());
            }
            Err(e) => self.note_call_error(e).await,
        }
        match result {
            // A missing session isn't a mismatch: retry the read once the session is back.
            Err(e) if !snapshotting && (is_session_loss(&e) || is_reachability(&e)) => {
                Input::SessionUnavailable
            }
            r => Input::ConfigRead(r),
        }
    }

    async fn post(&mut self, cfg: &NetConfig) -> Input {
        if !self.online() {
            return Input::SessionUnavailable;
        }
        let result = self.driver.apply_net_config(cfg).await.map_err(normalize);
        let detail = match &result {
            Ok(()) => serde_json::json!({ "phase": self.sa.phase(), "config": cfg, "ok": true }),
            Err(e) => serde_json::json!({ "phase": self.sa.phase(), "config": cfg, "error": e }),
        };
        self.log_event(kinds::SAFE_APPLY_POST, detail).await;
        if let Err(e) = &result {
            self.note_call_error(e).await;
        }
        Input::PostResult(result)
    }

    async fn write_journal(&mut self, phase: JournalPhase) -> bool {
        let Some(mut rec) = self.sa.record() else {
            return false;
        };
        let now_ms = wall_ms();
        let created_ms = match &self.stamps {
            Some(s) if s.id == rec.id => s.created_ms,
            _ => now_ms,
        };
        rec.phase = phase;
        rec.created_at_ms = created_ms;
        rec.phase_changed_at_ms = now_ms;
        self.stamps = Some(Stamps {
            id: rec.id.clone(),
            created_ms,
            changed_ms: now_ms,
        });
        let dir = self.cfg.data_dir.clone();
        if let Err(e) = blocking(move || Journal::new(dir).write(&rec)).await {
            let detail = serde_json::json!({ "phase": phase, "error": e.to_string() });
            self.log_event(kinds::JOURNAL_WRITE_FAILED, detail).await;
            return false;
        }
        let event = match phase {
            JournalPhase::Journaled => Some(SafeApplyEvent::Started),
            JournalPhase::Applied => Some(SafeApplyEvent::Applied),
            JournalPhase::Verified => Some(SafeApplyEvent::Verified),
            _ => None,
        };
        if phase == JournalPhase::Journaled {
            // A starting step of a sequence: its change id goes on file before the POST.
            let adopted = match self.seq.as_mut() {
                Some(q) if q.change.is_none() => {
                    q.change = self.stamps.as_ref().map(|s| s.id.clone());
                    if q.step == 1 && q.original.is_none() {
                        q.original = self.sa.record().map(|r| r.previous);
                    }
                    true
                }
                _ => false,
            };
            if adopted {
                self.persist_sequence().await;
            }
        }
        if let Some(event) = event {
            self.emit_safe_event(event).await;
        }
        true
    }

    async fn archive_journal(&mut self) {
        let Some(mut rec) = self.sa.record() else {
            return;
        };
        let now_ms = wall_ms();
        let (created, changed) = match &self.stamps {
            Some(s) if s.id == rec.id => (s.created_ms, s.changed_ms),
            _ => (now_ms, now_ms),
        };
        rec.created_at_ms = created;
        rec.phase_changed_at_ms = changed;
        let dir = self.cfg.data_dir.clone();
        if let Err(e) = blocking(move || Journal::new(dir).archive(&rec)).await {
            let detail = serde_json::json!({ "error": e.to_string() });
            self.log_event(kinds::JOURNAL_ARCHIVE_FAILED, detail).await;
        }
    }

    // --- APN switch effects --------------------------------------------------------------------

    async fn apn_step(&mut self, input: Input<ApnTarget>) {
        let effects = self.apn_sa.step(input, now_std());
        self.run_apn_effects(effects).await;
    }

    /// `run_effects` for the APN switch.
    async fn run_apn_effects(&mut self, effects: Vec<Effect<ApnTarget>>) {
        let mut queue: VecDeque<Effect<ApnTarget>> = effects.into();
        while let Some(effect) = queue.pop_front() {
            let follow = match effect {
                Effect::ReadConfig => {
                    let input = self.read_apn_target().await;
                    self.apn_sa.step(input, now_std())
                }
                Effect::Post(target) => {
                    let input = self.post_apn(&target).await;
                    self.apn_sa.step(input, now_std())
                }
                Effect::WriteJournal(phase) => {
                    if self.write_apn_journal(phase).await || phase != JournalPhase::Journaled {
                        vec![]
                    } else {
                        // No journal, no POST.
                        if matches!(queue.front(), Some(Effect::Post(_))) {
                            queue.pop_front();
                        }
                        let err = validation("could not write the pending-change journal");
                        self.apn_sa.step(Input::PostResult(Err(err)), now_std())
                    }
                }
                Effect::ArchiveJournal => {
                    self.archive_apn_journal().await;
                    vec![]
                }
                Effect::Event(event) => {
                    self.emit_apn_event(event).await;
                    vec![]
                }
                Effect::RetryAfter(d) => {
                    self.retry_ticks.push(Instant::now() + d);
                    vec![]
                }
                // The loss policy never applies to an APN switch.
                Effect::LossIgnored(_) => vec![],
            };
            for e in follow.into_iter().rev() {
                queue.push_front(e);
            }
        }
    }

    /// The profile in use, as the APN switch's "config read". Also refreshes the list.
    async fn read_apn_target(&mut self) -> Input<ApnTarget> {
        if !self.online() {
            return Input::SessionUnavailable;
        }
        let snapshotting = *self.apn_sa.phase() == Phase::Snapshotting;
        self.last_apn = Some(Instant::now());
        let result = match self.driver.apn_profiles().await.map_err(normalize) {
            Ok(p) => {
                let current = p.current_target();
                self.apn_profiles = Some(p);
                current.ok_or_else(|| RouterError::Parse {
                    message: "the router names no APN profile in use".to_string(),
                })
            }
            Err(e) => {
                self.note_call_error(&e).await;
                Err(e)
            }
        };
        match result {
            Err(e) if !snapshotting && (is_session_loss(&e) || is_reachability(&e)) => {
                Input::SessionUnavailable
            }
            r => Input::ConfigRead(r),
        }
    }

    async fn post_apn(&mut self, target: &ApnTarget) -> Input<ApnTarget> {
        if !self.online() {
            return Input::SessionUnavailable;
        }
        self.wan_read_since_apn_post = false;
        let result = self.driver.set_default_apn(target).await.map_err(normalize);
        if let Err(e) = &result {
            self.note_call_error(e).await;
        }
        Input::PostResult(result)
    }

    async fn write_apn_journal(&mut self, phase: JournalPhase) -> bool {
        let Some(mut rec) = self.apn_sa.record() else {
            return false;
        };
        let now_ms = wall_ms();
        let created_ms = match &self.apn_stamps {
            Some(s) if s.id == rec.id => s.created_ms,
            _ => now_ms,
        };
        rec.phase = phase;
        rec.created_at_ms = created_ms;
        rec.phase_changed_at_ms = now_ms;
        self.apn_stamps = Some(Stamps {
            id: rec.id.clone(),
            created_ms,
            changed_ms: now_ms,
        });
        let dir = self.cfg.data_dir.clone();
        if let Err(e) = blocking(move || apn_journal(dir).write_record(&rec)).await {
            let detail =
                serde_json::json!({ "file": "apn", "phase": phase, "error": e.to_string() });
            self.log_event(kinds::JOURNAL_WRITE_FAILED, detail).await;
            return false;
        }
        let event = match phase {
            JournalPhase::Journaled => Some(SafeApplyEvent::Started),
            JournalPhase::Applied => Some(SafeApplyEvent::Applied),
            JournalPhase::Verified => Some(SafeApplyEvent::Verified),
            _ => None,
        };
        if let Some(event) = event {
            self.emit_apn_event(event).await;
        }
        true
    }

    async fn archive_apn_journal(&mut self) {
        let Some(mut rec) = self.apn_sa.record() else {
            return;
        };
        let now_ms = wall_ms();
        let (created, changed) = match &self.apn_stamps {
            Some(s) if s.id == rec.id => (s.created_ms, s.changed_ms),
            _ => (now_ms, now_ms),
        };
        rec.created_at_ms = created;
        rec.phase_changed_at_ms = changed;
        let dir = self.cfg.data_dir.clone();
        if let Err(e) = blocking(move || apn_journal(dir).archive_record(&rec)).await {
            let detail = serde_json::json!({ "file": "apn", "error": e.to_string() });
            self.log_event(kinds::JOURNAL_ARCHIVE_FAILED, detail).await;
        }
    }

    /// Logs an APN event with the index, phase and reason only. A refusal goes back as the
    /// command's reply, not as an alert.
    async fn emit_apn_event(&mut self, event: SafeApplyEvent) {
        let index = self
            .apn_sa
            .record()
            .map(|r| r.requested.index)
            .or(self.apn_index);
        let mut detail = serde_json::json!({ "index": index, "phase": self.apn_sa.phase() });
        match &event {
            SafeApplyEvent::Reverted { reason, .. } => {
                detail["reason"] = serde_json::to_value(reason).unwrap_or_default();
            }
            SafeApplyEvent::Rejected { message } => {
                detail["message"] = message.clone().into();
                self.apn_rejected = Some(message.clone());
            }
            _ => {}
        }
        self.log_event(apn_event_kind(&event), detail).await;
        if !matches!(event, SafeApplyEvent::Rejected { .. }) {
            let _ = self.alerts.send(Alert::Apn { event });
        }
    }

    async fn emit_safe_event(&mut self, event: SafeApplyEvent) {
        if let SafeApplyEvent::Rejected { message } = &event {
            self.last_rejected = Some(message.clone());
        }
        let reattach_step1 = self
            .seq
            .as_ref()
            .is_some_and(|q| q.origin == SequenceOrigin::IpReattach && q.step == 1);
        if event == SafeApplyEvent::Reattached && reattach_step1 {
            self.reattach.reattached_at = Some(Instant::now());
        }
        let detail = serde_json::to_value(&event).unwrap_or_default();
        self.log_event(event_kind(&event), detail).await;
        let _ = self.alerts.send(Alert::SafeApply { event });
    }

    /// Appends to `events.jsonl`. A failed append is dropped: the log is an audit aid and must
    /// never stop the watchdog.
    async fn log_event(&self, kind: &str, detail: serde_json::Value) {
        let record = EventRecord {
            at_ms: wall_ms(),
            kind: kind.to_string(),
            detail,
        };
        let dir = self.cfg.data_dir.clone();
        let _ = blocking(move || EventLog::new(dir).append(&record)).await;
    }
}

fn event_kind(e: &SafeApplyEvent) -> &'static str {
    match e {
        SafeApplyEvent::Started => kinds::SAFE_APPLY_STARTED,
        SafeApplyEvent::Applied => kinds::SAFE_APPLY_APPLIED,
        SafeApplyEvent::Verified => kinds::SAFE_APPLY_VERIFIED,
        SafeApplyEvent::Reattached => kinds::SAFE_APPLY_REATTACHED,
        SafeApplyEvent::Committed => kinds::SAFE_APPLY_COMMITTED,
        SafeApplyEvent::Reverted { .. } => kinds::SAFE_APPLY_REVERTED,
        SafeApplyEvent::FellBackToAuto => kinds::SAFE_APPLY_FELL_BACK_TO_AUTO,
        SafeApplyEvent::RevertFailed => kinds::SAFE_APPLY_REVERT_FAILED,
        SafeApplyEvent::Rejected { .. } => kinds::SAFE_APPLY_REJECTED,
    }
}

fn apn_event_kind(e: &SafeApplyEvent) -> &'static str {
    match e {
        SafeApplyEvent::Started => kinds::APN_STARTED,
        SafeApplyEvent::Applied => kinds::APN_APPLIED,
        SafeApplyEvent::Verified => kinds::APN_VERIFIED,
        SafeApplyEvent::Reattached => kinds::APN_REATTACHED,
        SafeApplyEvent::Committed => kinds::APN_COMMITTED,
        SafeApplyEvent::Reverted { .. } => kinds::APN_REVERTED,
        // An APN has no fallback, so this never comes; a failed revert says the most.
        SafeApplyEvent::FellBackToAuto | SafeApplyEvent::RevertFailed => kinds::APN_REVERT_FAILED,
        SafeApplyEvent::Rejected { .. } => kinds::APN_REJECTED,
    }
}

/// The APN switch's journal and archive.
fn apn_journal(dir: PathBuf) -> Journal {
    Journal::with_files(dir, APN_JOURNAL_FILE, APN_ARCHIVE_DIR)
}

fn cell_key(sig: &Signal) -> Option<CellKey> {
    match sig.serving? {
        Rat::Lte => {
            let l = sig.lte.as_ref()?;
            Some(CellKey {
                rat: Rat::Lte,
                pci: l.pci,
                cell_id: l.cell_id,
                band: l.band.map(|b| format!("B{b}")).unwrap_or_default(),
            })
        }
        Rat::Nr => {
            let n = sig.nr.as_ref()?;
            Some(CellKey {
                rat: Rat::Nr,
                pci: n.pci,
                cell_id: None,
                band: n
                    .band
                    .as_ref()
                    .map(|b| format!("n{}", b.primary))
                    .unwrap_or_default(),
            })
        }
        rat => Some(CellKey {
            rat,
            pci: None,
            cell_id: None,
            band: String::new(),
        }),
    }
}

/// Step 1 of a band switch for a new IP: a config the router can move to without losing the band
/// in use, so it re-attaches (a band change gives a new address; seen on the H122 when B3 became
/// B1+B3). Two or more allowed bands, or Auto: the serving band alone (the lowest allowed band
/// when the serving one isn't known or isn't allowed). One band: every supported band. The net
/// mode and the 5G option stay as they are. Refused on a mode without LTE, or when nothing else
/// is possible.
pub fn reattach_step1(
    config: &NetConfig,
    caps: &Capabilities,
    serving_lte_band: Option<u16>,
) -> Result<NetConfig, RouterError> {
    if !matches!(
        config.network_mode,
        NetworkMode::Auto | NetworkMode::LteOnly
    ) {
        return Err(validation(IP_REATTACH_NO_LTE));
    }
    let allowed = if config.lte_bands == LteBandSet::AUTO {
        caps.supported_lte
    } else {
        config.lte_bands
    };
    let bands = allowed.bands();
    let serving = serving_lte_band
        .and_then(|b| u8::try_from(b).ok())
        .filter(|b| allowed.contains(*b));
    let lte_bands = if bands.len() >= 2 {
        let band = serving.unwrap_or(bands[0]);
        LteBandSet::from_bands(&[band])?
    } else {
        caps.supported_lte
    };
    if lte_bands.is_empty() || lte_bands == config.lte_bands || lte_bands == allowed {
        return Err(validation(IP_REATTACH_NO_OTHER_BANDS));
    }
    Ok(NetConfig {
        lte_bands,
        ..config.clone()
    })
}

/// `cfg` with only `primary` allowed on LTE.
fn primary_only(cfg: &NetConfig, primary: u8) -> NetConfig {
    NetConfig {
        lte_bands: LteBandSet::from_bands(&[primary]).unwrap_or(cfg.lte_bands),
        ..cfg.clone()
    }
}

/// An operation checked by `busy_refusal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Reboot,
    FactoryReset,
    Logout,
    IpRefresh,
    /// A band switch for a new IP.
    IpReattach,
    ApnSwitch,
    /// An APN add, edit or delete.
    ApnEdit,
    /// The user's "Turn mobile data on"; a speed test or the page-script read doesn't block it.
    DataOn,
    /// Reading the router's page scripts (`BeginPageScripts`).
    PageScripts,
}

/// One APN profile write; the draft's password ends with it.
enum ApnWrite {
    Add(ApnDraft),
    Edit(ApnTarget, ApnDraft),
    Delete(ApnTarget),
}

/// The router's login state, as `session.logout` records it.
fn router_state_name(s: SessionState) -> &'static str {
    match s {
        SessionState::LoggedIn => "loggedIn",
        SessionState::LoggedOut => "loggedOut",
        SessionState::Locked { .. } => "locked",
    }
}

fn change_closed(p: &Phase) -> bool {
    matches!(p, Phase::Idle | Phase::Committed | Phase::Reverted { .. })
}

fn reboot_allowed(p: &Phase) -> bool {
    change_closed(p)
}

fn is_session_loss(e: &RouterError) -> bool {
    matches!(
        e,
        RouterError::SessionLost { .. }
            | RouterError::SessionTakenOver
            | RouterError::Api {
                kind: ApiErrorKind::NoRights,
                ..
            }
    )
}

/// A driver that reports "no rights" as a raw API error still means the session is gone;
/// `SafeApply` sees it as `SessionLost`, so a revert POST isn't spent on it.
fn normalize(e: RouterError) -> RouterError {
    match e {
        RouterError::Api {
            kind: ApiErrorKind::NoRights,
            code,
        } => RouterError::SessionLost { code: Some(code) },
        other => other,
    }
}

/// Login failures that end the session for good: the actor never retries these by itself.
fn login_stop_view(e: &RouterError) -> Option<SessionView> {
    match e {
        RouterError::BadCredentials => Some(SessionView::BadCredentials),
        RouterError::LoginRejected { code } => Some(SessionView::LoginRejected { code: *code }),
        RouterError::LockedOut { wait_s } => Some(SessionView::LockedOut { wait_s: *wait_s }),
        RouterError::LoginAlgorithmMismatch => Some(SessionView::AlgorithmMismatch),
        RouterError::UnrecognizedRouter { first_time } => Some(SessionView::ConfirmRouter {
            first_time: *first_time,
        }),
        _ => None,
    }
}

fn is_reachability(e: &RouterError) -> bool {
    matches!(e, RouterError::Unreachable | RouterError::Timeout)
}

fn reconnecting(e: &RouterError, retry: Duration) -> SessionView {
    SessionView::Reconnecting {
        reason: e.to_string(),
        retry_in_s: retry.as_secs().try_into().unwrap_or(u32::MAX),
    }
}

fn validation(message: &str) -> RouterError {
    RouterError::Validation {
        message: message.to_string(),
    }
}

fn due(last: Option<Instant>, every: Duration, now: Instant) -> bool {
    last.is_none_or(|t| now.saturating_duration_since(t) >= every)
}

/// `SafeApply` takes `std::time::Instant`; tokio's follows the paused test clock (P5).
fn now_std() -> std::time::Instant {
    Instant::now().into_std()
}

fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

async fn sleep_until_opt(at: Option<Instant>) {
    match at {
        Some(t) => sleep_until(t).await,
        None => std::future::pending().await,
    }
}

/// Journal and event-log I/O may sleep in a bounded Windows retry: keep it off the async thread.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .unwrap_or_else(|e| Err(io::Error::other(e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(supported: &[u8]) -> Capabilities {
        Capabilities {
            supported_lte: LteBandSet::from_bands(supported).unwrap(),
            modes: vec![NetworkMode::Auto, NetworkMode::LteOnly],
            nr_options: Vec::new(),
            band_edit_allowed: true,
            factory_reset_verified: false,
            data_switch: false,
            ip_methods: Vec::new(),
            apn: Default::default(),
        }
    }

    fn cfg(mode: NetworkMode, bands: LteBandSet) -> NetConfig {
        NetConfig {
            network_mode: mode,
            lte_bands: bands,
            legacy_band_raw: "0".into(),
            nr_option: Some(crate::NrOption::Nsa),
        }
    }

    fn set(bands: &[u8]) -> LteBandSet {
        LteBandSet::from_bands(bands).unwrap()
    }

    #[test]
    fn two_or_more_bands_go_to_the_serving_band_alone() {
        let c = caps(&[1, 3, 20, 28]);
        let from = cfg(NetworkMode::Auto, set(&[1, 3]));
        let step1 = reattach_step1(&from, &c, Some(3)).unwrap();
        assert_eq!(step1, cfg(NetworkMode::Auto, set(&[3])));
        // Mode and 5G option are kept.
        let lte_only = cfg(NetworkMode::LteOnly, set(&[1, 3, 20]));
        let step1 = reattach_step1(&lte_only, &c, Some(20)).unwrap();
        assert_eq!(step1, cfg(NetworkMode::LteOnly, set(&[20])));
    }

    #[test]
    fn an_unknown_or_outside_serving_band_takes_the_lowest_band() {
        let c = caps(&[1, 3, 20, 28]);
        let from = cfg(NetworkMode::Auto, set(&[3, 20]));
        assert_eq!(
            reattach_step1(&from, &c, None).unwrap().lte_bands,
            set(&[3])
        );
        assert_eq!(
            reattach_step1(&from, &c, Some(1)).unwrap().lte_bands,
            set(&[3])
        );
        assert_eq!(
            reattach_step1(&from, &c, Some(300)).unwrap().lte_bands,
            set(&[3])
        );
    }

    #[test]
    fn one_band_widens_to_every_supported_band() {
        let c = caps(&[1, 3, 20, 28]);
        let from = cfg(NetworkMode::Auto, set(&[3]));
        let step1 = reattach_step1(&from, &c, Some(3)).unwrap();
        assert_eq!(step1, cfg(NetworkMode::Auto, set(&[1, 3, 20, 28])));
        // Nothing else supported: nothing to switch to.
        let only = caps(&[3]);
        assert_eq!(
            reattach_step1(&from, &only, Some(3)),
            Err(validation(IP_REATTACH_NO_OTHER_BANDS))
        );
    }

    #[test]
    fn auto_bands_go_to_the_serving_band_alone() {
        let c = caps(&[1, 3, 20, 28]);
        let from = cfg(NetworkMode::Auto, LteBandSet::AUTO);
        let step1 = reattach_step1(&from, &c, Some(20)).unwrap();
        assert_eq!(step1.lte_bands, set(&[20]));
        let only = caps(&[3]);
        assert_eq!(
            reattach_step1(&from, &only, Some(3)),
            Err(validation(IP_REATTACH_NO_OTHER_BANDS))
        );
    }

    #[test]
    fn a_mode_without_lte_is_refused() {
        let c = caps(&[1, 3]);
        for mode in [NetworkMode::NrOnly, NetworkMode::Raw { value: "02".into() }] {
            let from = cfg(mode, set(&[1, 3]));
            assert_eq!(
                reattach_step1(&from, &c, Some(3)),
                Err(validation(IP_REATTACH_NO_LTE))
            );
        }
    }
}
