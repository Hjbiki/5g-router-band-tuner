//! Safe-apply state machine (§7.1). Pure: no I/O and no clock reads.
//!
//! The machine is generic over what it changes (`ChangeTarget`); the band config (`NetConfig`)
//! is the default and the only target today. The target decides what it accepts, what counts
//! as evidence of service, which cell the settle window follows, whether a revert may fall back
//! to a wider config, and whether the loss policy applies to it.
//!
//! The caller passes `now` with every input and performs the returned effects in order:
//! - `ReadConfig` → read `net-mode` from the router, answer with `Input::ConfigRead`.
//! - `Post(cfg)` → POST it, answer with `Input::PostResult`, or `Input::SessionUnavailable` when
//!   there is no session to post with.
//! - `WriteJournal(phase)` → `Journal::write` of `record()` with `phase` and stamped wall-clock
//!   times. `ArchiveJournal` → `Journal::archive` of `record()`.
//! - `RetryAfter(d)` → send `Input::Tick` after `d`.
//!
//! Caller contract (the timers depend on it):
//! - Send `Tick` at least once per poll interval.
//! - Send `Observation { in_service, polled_at, lte, wan_ip_present }` for each successful poll:
//!   `polled_at` is when the poll *started*, `lte` the serving LTE cell (band, PCI) if any, and
//!   `wan_ip_present` whether the router has an internet address (`None` when unknown).
//! - Send `SessionUnavailable` for each poll that fails, whatever the error (API, parse, session,
//!   reachability): a failed poll is never evidence of service.
//! - Call `set_poll_interval` whenever the cadence changes (1 s visible / 5 s hidden).
//!
//! Re-attach (ReattachGrace -> Probation) needs settled evidence on the new config: every
//! observation for `settle` must come from a poll started after the verify, be in service, and
//! show a serving LTE band the requested mask allows, on one cell. A drop, a disallowed band
//! or a cell change starts the window again. (Device test 2026-10-29: the router kept
//! reporting the old cell in service for about 4 s after the verify.)
//!
//! After a successful POST the router may still report the old config, or fail a read while it
//! re-attaches: the verify read is repeated every 5 s for up to 20 s before it counts as a
//! mismatch. Any other config reverts at once.
//!
//! After an ambiguous POST failure (timeout, unreachable, unparseable reply) a read-back equal to
//! the snapshot doesn't close the change at once: it is read again after 10 s (`RetryAfter`), and
//! only a second snapshot read closes it as not applied. `requested` on either read is watched as
//! a normal change; anything else reverts.
//!
//! Every input checks the deadlines (verify/grace window, loss threshold). A revert that falls due
//! on `SessionUnavailable` becomes `RevertPending`. A pending revert resumes (re-issues its POST,
//! or its read-back) on the next successful poll, i.e. the next `Observation`, or on `RevertNow`;
//! routine Ticks don't, so the phase doesn't flicker while the session is down. Ticks and session
//! failures never change the loss timer: "unknown" keeps the last known state, so an outage that
//! started without service still ends in a revert.
//!
//! Sleep rule: if no input of any kind arrived for more than `max(3 x poll_interval, 45 s)`, the
//! process was suspended, and the suspended time is never counted. Before the input is processed:
//! - Probation: the loss timer restarts at `now` if the last observation was out of service, and
//!   is cleared if it was in service.
//! - ReattachGrace: the grace window restarts at `now`.
//!
//! Either way a fresh `Observation` is then required before Probation may commit. Revert
//! deadlines (bad news) still fire without one.
//!
//! Loss policy (`set_revert_on_loss`, on by default). When it's off, the deadlines that mean "no
//! service on the new config" don't revert: a re-attach grace that ends without settled service
//! (`NoReattach`, `ResumedWithoutService`) moves on to Probation, and a probation loss past the
//! threshold (`LossOfService`) is only reported. Each emits `Effect::LossIgnored` (once per
//! change), and Probation then commits when its timer ends, with or without service. Everything
//! that shows the router didn't take the change still reverts: a rejected apply, a verify
//! mismatch and a verify that never completes. `RevertNow`, the journal and a revert already
//! under way are unaffected.
//!
//! Revert target: `Start { revert_to: Some(cfg) }` reverts to `cfg` instead of the snapshot (the
//! second step of a primary-band apply goes back to the config from before the first step). The
//! snapshot still decides the verify ("the router still shows the old config"); the journal
//! records the revert target as `previous`, so a resumed change reverts to it as well.

use std::fmt::Debug;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::journal::{JournalPhase, JournalRecord, JOURNAL_VERSION};
use crate::{Capabilities, LteBandSet, NetConfig, NetworkMode, RouterError};

/// Waits after the 1st, 2nd and 3rd failed attempt of one revert target.
const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(30),
];
const MAX_ATTEMPTS: u8 = 3;
/// After an uncertain POST whose first read-back still shows the snapshot, the config is read
/// once more after this delay: the router may apply a POST late.
const UNCERTAIN_REREAD: Duration = Duration::from_secs(10);
const VERIFY_REREAD: Duration = Duration::from_secs(5);
/// How long after a successful POST the old config (or a failed read) is still tolerated.
const VERIFY_WINDOW: Duration = Duration::from_secs(20);
/// After `RevertFailed`, the whole revert is retried this often while the app runs.
const REVERT_FAILED_RETRY: Duration = Duration::from_secs(30);
/// Lower bound of the sleep-gap threshold. Re-login back-off tops out at 30 s, so a live process
/// always sends some input more often than this.
const MIN_GAP: Duration = Duration::from_secs(45);

/// What one successful poll showed, as far as a change target cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Evidence {
    pub in_service: bool,
    /// Serving LTE cell as (band, PCI), if any.
    pub lte: Option<ServingLte>,
    /// The router has an internet (WAN) address; `None` when unknown.
    pub wan_ip_present: Option<bool>,
}

/// Something safe-apply can change, verify and revert.
pub trait ChangeTarget: Clone + PartialEq + Debug + Serialize + DeserializeOwned {
    /// Whether `set_revert_on_loss(false)` may keep a change that lost service. When false,
    /// every loss reverts whatever the setting says.
    const LOSS_POLICY_APPLIES: bool;

    /// Whether this router can take `self` now. Also checks a revert target read from disk.
    fn validate(&self, caps: &Capabilities) -> Result<(), String>;

    /// Whether an in-service poll counts as evidence for this (requested) target.
    fn evidence_ok(&self, obs: &Evidence) -> bool;

    /// The settle window needs one value of this for its whole run: a change of it starts the
    /// window again.
    fn settle_key(obs: &Evidence) -> Option<ServingLte>;

    /// What to revert to when `previous` can't be restored; `None` means there's no fallback,
    /// and the revert ends in `RevertFailed`.
    fn fallback(previous: &Self) -> Option<Self>;
}

impl ChangeTarget for NetConfig {
    const LOSS_POLICY_APPLIES: bool = true;

    fn validate(&self, caps: &Capabilities) -> Result<(), String> {
        if !caps.band_edit_allowed {
            return Err("band editing isn't allowed on this router right now".to_string());
        }
        check_config(self, caps)
    }

    /// The serving LTE band, if one is reported, must be one the requested mask allows.
    fn evidence_ok(&self, obs: &Evidence) -> bool {
        obs.lte
            .is_none_or(|(band, _)| u8::try_from(band).is_ok_and(|b| self.lte_bands.contains(b)))
    }

    /// One cell: a cell change restarts the settle window.
    fn settle_key(obs: &Evidence) -> Option<ServingLte> {
        obs.lte
    }

    /// Auto (all bands), keeping the router's legacy band value and 5G option.
    fn fallback(previous: &Self) -> Option<Self> {
        Some(NetConfig {
            network_mode: NetworkMode::Auto,
            lte_bands: LteBandSet::AUTO,
            legacy_band_raw: previous.legacy_band_raw.clone(),
            nr_option: previous.nr_option,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SafeApplyTimers {
    pub grace: Duration,
    pub loss: Duration,
    pub probation: Duration,
    pub poll_interval: Duration,
    /// Continuous in-service evidence needed before re-attach counts.
    pub settle: Duration,
}

impl Default for SafeApplyTimers {
    fn default() -> Self {
        Self {
            grace: Duration::from_secs(90),
            loss: Duration::from_secs(60),
            probation: Duration::from_secs(180),
            poll_interval: Duration::from_secs(1),
            settle: Duration::from_secs(10),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Phase {
    Idle,
    Snapshotting,
    Applying,
    Verifying,
    ReattachGrace,
    Probation,
    Reverting {
        attempt: u8,
        fallback: bool,
    },
    RevertPending {
        reason: RevertReason,
    },
    Committed,
    /// `fallback`: the previous bands couldn't be restored, so the router was switched to Auto
    /// (all bands) instead.
    Reverted {
        reason: RevertReason,
        fallback: bool,
    },
    RevertFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RevertReason {
    ApplyRejected,
    VerifyMismatch,
    NoReattach,
    LossOfService,
    UserRequested,
    ResumedWithoutService,
}

// An input is built, stepped and dropped at once; `Start` (two configs) is rare, so boxing it
// would only add noise.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum Input<T = NetConfig> {
    Start {
        requested: T,
        caps: Capabilities,
        current: Option<T>,
        /// Revert to this instead of the snapshot. Ignored when equal to `requested`.
        revert_to: Option<T>,
    },
    ConfigRead(Result<T, RouterError>),
    PostResult(Result<(), RouterError>),
    Observation {
        in_service: bool,
        /// When the poll started; `None` means "just now".
        polled_at: Option<Instant>,
        /// Serving LTE cell as (band, PCI), if any.
        lte: Option<ServingLte>,
        /// The router has an internet address; `None` when unknown. The band target ignores it.
        wan_ip_present: Option<bool>,
    },
    SessionUnavailable,
    Tick,
    KeepNow,
    RevertNow,
    /// The auto-scan stops waiting for this change: a revert like `RevertNow`, except that the
    /// reason says the band gave no service, so the band history counts it.
    GiveUp,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect<T = NetConfig> {
    ReadConfig,
    WriteJournal(JournalPhase),
    Post(T),
    ArchiveJournal,
    Event(SafeApplyEvent),
    RetryAfter(Duration),
    /// Only with the loss policy off: service was lost (or never came back) on the new config
    /// and no revert was started. `reason` is the revert that would have run.
    LossIgnored(RevertReason),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum SafeApplyEvent {
    Started,
    Applied,
    Verified,
    Reattached,
    Committed,
    /// `fallback`: the revert ended on Auto (all bands), not on the previous bands.
    Reverted {
        reason: RevertReason,
        fallback: bool,
    },
    FellBackToAuto,
    RevertFailed,
    Rejected {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, concrete(T = NetConfig))]
pub struct SafeApplyView<T = NetConfig> {
    pub phase: Phase,
    pub seconds_left: Option<u32>,
    /// What a revert goes back to.
    pub previous: Option<T>,
    pub requested: Option<T>,
    /// The loss policy in force. Always set by the backend (optional only for old fixtures).
    #[serde(default)]
    #[ts(optional = nullable)]
    pub policy: Option<SafeApplyPolicyView>,
    /// A primary-band apply in progress (filled in by the actor).
    #[serde(default)]
    #[ts(optional = nullable)]
    pub sequence: Option<ApplySequenceView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SafeApplyPolicyView {
    /// Auto-revert on loss of service is on (the default).
    pub revert_on_loss: bool,
    /// The open change lost service (or never re-attached) and wasn't reverted because the
    /// policy is off.
    pub loss_ignored: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SequenceOrigin {
    /// The user pressed Apply with a primary band set.
    User,
    /// The re-lock watchdog.
    Relock,
    /// The user asked for a new IP address: step 1 is a short stay on other bands (`step1`),
    /// step 2 puts the bands from before back.
    IpReattach,
}

/// "Step 1 of 2": step 1 locks the primary band alone (or `step1`), step 2 applies
/// `final_config`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ApplySequenceView {
    pub step: u8,
    pub total: u8,
    /// The primary band; for `IpReattach`, the first band of step 1 (display only).
    pub primary: u8,
    pub final_config: NetConfig,
    pub origin: SequenceOrigin,
    /// Step 1's config when it isn't the primary band alone (`IpReattach`).
    #[serde(default)]
    #[ts(optional = nullable)]
    pub step1: Option<NetConfig>,
}

/// A journal written for another host or driver. It's kept and surfaced, never applied.
#[derive(Debug, Clone, PartialEq)]
pub struct ForeignJournal<T = NetConfig>(pub JournalRecord<T>);

/// Serving LTE cell as reported by a poll: (band, PCI).
pub type ServingLte = (u16, Option<u16>);

/// Why a config read is outstanding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadKind {
    /// The POST succeeded: the read-back must equal `requested`.
    AfterApply,
    /// The POST failed ambiguously (timeout, unreachable, unparseable reply): it may or may not
    /// have been applied.
    AfterUncertainApply,
    /// The first read after an uncertain POST showed the snapshot; this is the second (and
    /// last) read before the change is closed as not applied.
    UncertainRecheck,
    /// Resumed from a `journaled`/`applied` journal.
    Resumed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RevertStage {
    /// `Post(target)` issued, waiting for `PostResult`.
    Posting,
    /// A POST is due but there is no session (`RevertPending`).
    PostDue,
    /// The revert POST succeeded but its read-back had no session (`RevertPending`).
    ReadDue,
    /// A POST failed; the next one is due at this instant.
    RetryAt(Instant),
    /// The POST succeeded, waiting for the read-back.
    Confirming,
    /// Read-back matched; waiting for service since this instant.
    AwaitReattach(Instant),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Revert {
    reason: RevertReason,
    /// Failed attempts of the current target.
    attempt: u8,
    fallback: bool,
    stage: RevertStage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Snapshotting,
    Applying,
    Verifying {
        kind: ReadKind,
        /// The read couldn't be issued (no session); re-issue on the next Tick/Observation.
        read_due: bool,
        /// A re-read is scheduled for this instant (uncertain POST re-check).
        read_at: Option<Instant>,
        /// Start of the grace clock (the POST result, or the resume): verify must finish within
        /// `grace` of it.
        since: Instant,
    },
    ReattachGrace {
        since: Instant,
        resumed: bool,
        /// When grace began (the verify): earlier polls aren't evidence.
        entered: Instant,
        /// Start of the current run of good observations, and its cell.
        settle: Option<(Instant, Option<ServingLte>)>,
    },
    Probation {
        reattach_at: Instant,
        loss_start: Option<Instant>,
    },
    Reverting(Revert),
    Committed,
    Reverted {
        reason: RevertReason,
        fallback: bool,
    },
    RevertFailed {
        reason: RevertReason,
        retry_at: Instant,
    },
}

impl State {
    fn phase(&self) -> Phase {
        match *self {
            State::Idle => Phase::Idle,
            State::Snapshotting => Phase::Snapshotting,
            State::Applying => Phase::Applying,
            State::Verifying { .. } => Phase::Verifying,
            State::ReattachGrace { .. } => Phase::ReattachGrace,
            State::Probation { .. } => Phase::Probation,
            State::Reverting(Revert {
                stage: RevertStage::PostDue | RevertStage::ReadDue,
                reason,
                ..
            }) => Phase::RevertPending { reason },
            State::Reverting(Revert {
                attempt, fallback, ..
            }) => Phase::Reverting { attempt, fallback },
            State::Committed => Phase::Committed,
            State::Reverted { reason, fallback } => Phase::Reverted { reason, fallback },
            State::RevertFailed { .. } => Phase::RevertFailed,
        }
    }
}

pub struct SafeApply<T: ChangeTarget = NetConfig> {
    timers: SafeApplyTimers,
    host: String,
    driver: String,
    state: State,
    phase: Phase,
    id: Option<String>,
    /// The revert target (journaled as `previous`).
    previous: Option<T>,
    /// What the router had when the change started; the verify compares against it.
    snapshot: Option<T>,
    /// `Start { revert_to }`, applied once the snapshot is read.
    revert_to: Option<T>,
    requested: Option<T>,
    journal_phase: Option<JournalPhase>,
    /// Revert when the new config loses service (the default).
    revert_on_loss: bool,
    /// A loss was ignored in this change because `revert_on_loss` is off.
    loss_ignored: bool,
    /// Time of the previous `step`, of any input. Drives the sleep-gap rule.
    last_input_at: Option<Instant>,
    /// The most recent observation was in service.
    last_in_service: bool,
    /// A `SessionUnavailable` arrived after the most recent observation.
    session_down: bool,
    /// A sleep gap happened and no observation has arrived since: no good-news decisions.
    needs_fresh_observation: bool,
}

impl<T: ChangeTarget> SafeApply<T> {
    pub fn new(timers: SafeApplyTimers, host: String, driver: String) -> Self {
        Self {
            timers,
            host,
            driver,
            state: State::Idle,
            phase: Phase::Idle,
            id: None,
            previous: None,
            snapshot: None,
            revert_to: None,
            requested: None,
            journal_phase: None,
            revert_on_loss: true,
            loss_ignored: false,
            last_input_at: None,
            last_in_service: false,
            session_down: false,
            needs_fresh_observation: false,
        }
    }

    /// The poll cadence currently in use; it scales the sleep-gap threshold.
    pub fn set_poll_interval(&mut self, d: Duration) {
        self.timers.poll_interval = d;
    }

    /// The loss policy; applies to the open change at once. Turning it back on during a
    /// probation outage restarts the loss timer at `now`, so the full threshold applies.
    pub fn set_revert_on_loss(&mut self, on: bool, now: Instant) {
        if on && !self.revert_on_loss {
            if let State::Probation {
                loss_start: Some(start),
                ..
            } = &mut self.state
            {
                *start = now;
            }
        }
        self.revert_on_loss = on;
    }

    /// The loss policy in force: always on for a target it doesn't apply to.
    pub fn revert_on_loss(&self) -> bool {
        self.revert_on_loss || !T::LOSS_POLICY_APPLIES
    }

    /// Id of the current (or last) change; every accepted `Start` draws a new one.
    pub fn change_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// Restarts from a journal found on launch (§7.2). Wall-clock times in the record are never
    /// trusted: every timer starts fresh at `now`.
    // The Err carries the whole record back to the caller by design (it's kept and shown);
    // this runs once at startup, so the size doesn't matter.
    #[allow(clippy::result_large_err)]
    pub fn resume(
        record: JournalRecord<T>,
        timers: SafeApplyTimers,
        host: &str,
        driver: &str,
        now: Instant,
    ) -> Result<(Self, Vec<Effect<T>>), ForeignJournal<T>> {
        if record.host != host || record.driver != driver || record.version != JOURNAL_VERSION {
            return Err(ForeignJournal(record));
        }
        let mut sa = Self::new(timers, host.to_string(), driver.to_string());
        sa.id = Some(record.id);
        sa.snapshot = Some(record.previous.clone());
        sa.previous = Some(record.previous);
        sa.requested = Some(record.requested);
        sa.journal_phase = Some(record.phase);
        sa.last_input_at = Some(now);
        let effects = match record.phase {
            JournalPhase::Journaled | JournalPhase::Applied => {
                sa.state = State::Verifying {
                    kind: ReadKind::Resumed,
                    read_due: false,
                    read_at: None,
                    since: now,
                };
                vec![Effect::ReadConfig]
            }
            JournalPhase::Verified | JournalPhase::ReattachGrace | JournalPhase::Probation => {
                sa.enter_grace(now, true, now)
            }
            JournalPhase::Reverting | JournalPhase::RevertFailed => {
                sa.begin_revert(RevertReason::ResumedWithoutService)
            }
            // Crashed between the final journal write and the archive: just finish the archive.
            JournalPhase::Committed | JournalPhase::Reverted => {
                sa.state = State::Idle;
                vec![Effect::ArchiveJournal]
            }
        };
        sa.sync_phase();
        Ok((sa, effects))
    }

    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    /// The journal record for the current change, with timestamps left at 0 for the caller to
    /// stamp. `None` until the snapshot is known. Still available after the change closes, so
    /// the caller can archive it.
    pub fn record(&self) -> Option<JournalRecord<T>> {
        Some(JournalRecord {
            version: JOURNAL_VERSION,
            id: self.id.clone()?,
            driver: self.driver.clone(),
            host: self.host.clone(),
            previous: self.previous.clone()?,
            requested: self.requested.clone()?,
            phase: self.journal_phase?,
            phase_changed_at_ms: 0,
            created_at_ms: 0,
        })
    }

    pub fn view(&self, now: Instant) -> SafeApplyView<T> {
        let deadline = match self.state {
            State::Verifying { since, .. } | State::ReattachGrace { since, .. } => {
                Some(since + self.timers.grace)
            }
            State::Probation { reattach_at, .. } => Some(reattach_at + self.timers.probation),
            State::Reverting(Revert {
                stage: RevertStage::RetryAt(at),
                ..
            }) => Some(at),
            State::Reverting(Revert {
                stage: RevertStage::AwaitReattach(since),
                ..
            }) => Some(since + self.timers.grace),
            State::RevertFailed { retry_at, .. } => Some(retry_at),
            _ => None,
        };
        let idle = self.state == State::Idle;
        SafeApplyView {
            phase: self.phase.clone(),
            seconds_left: deadline.map(|d| ceil_secs(d.saturating_duration_since(now))),
            previous: self.previous.clone().filter(|_| !idle),
            requested: self.requested.clone().filter(|_| !idle),
            policy: Some(SafeApplyPolicyView {
                revert_on_loss: self.revert_on_loss(),
                loss_ignored: self.loss_ignored && !idle,
            }),
            sequence: None,
        }
    }

    pub fn step(&mut self, input: Input<T>, now: Instant) -> Vec<Effect<T>> {
        if self.is_gap(now) {
            self.reset_after_gap(now);
        }
        self.last_input_at = Some(now);

        let session_ok = !matches!(input, Input::SessionUnavailable);
        // Commit needs evidence of service: an in-service observation, or a Tick with no
        // session failure since the last (in-service) observation.
        let may_commit = match input {
            Input::Observation { in_service, .. } => in_service,
            Input::Tick => !self.session_down,
            _ => false,
        };
        match input {
            Input::Observation { in_service, .. } => {
                self.last_in_service = in_service;
                self.session_down = false;
                self.needs_fresh_observation = false;
            }
            Input::SessionUnavailable => self.session_down = true,
            _ => {}
        }

        let mut effects = Vec::new();
        // RevertPending resumes only on evidence the session is back (a successful poll) or on
        // an explicit RevertNow. Routine Ticks don't, so the phase doesn't flicker.
        if let State::Reverting(r) = self.state {
            if matches!(
                input,
                Input::Observation { .. } | Input::RevertNow | Input::GiveUp
            ) {
                effects.extend(self.resume_pending(r));
            }
        }

        effects.extend(match input {
            Input::Start {
                requested,
                caps,
                current,
                revert_to,
            } => self.on_start(requested, &caps, current.as_ref(), revert_to),
            Input::ConfigRead(result) => self.on_config_read(result, now),
            Input::PostResult(result) => self.on_post_result(result, now),
            Input::Observation {
                in_service,
                polled_at,
                lte,
                wan_ip_present,
            } => {
                let obs = Evidence {
                    in_service,
                    lte,
                    wan_ip_present,
                };
                self.on_observation(obs, polled_at, now)
            }
            Input::SessionUnavailable => self.on_session_unavailable(),
            Input::Tick => self.on_tick(now),
            Input::KeepNow => match self.state {
                State::Probation { .. } => self.commit(),
                _ => vec![],
            },
            Input::RevertNow => match self.state {
                State::ReattachGrace { .. }
                | State::Probation { .. }
                | State::RevertFailed { .. } => self.begin_revert(RevertReason::UserRequested),
                _ => vec![],
            },
            Input::GiveUp => match self.state {
                State::ReattachGrace { .. } => self.begin_revert(RevertReason::NoReattach),
                State::Probation { .. } => self.begin_revert(RevertReason::LossOfService),
                State::RevertFailed { reason, .. } => self.begin_revert(reason),
                _ => vec![],
            },
        });
        effects.extend(self.check_deadlines(now, session_ok, may_commit));
        self.sync_phase();
        effects
    }

    fn on_start(
        &mut self,
        requested: T,
        caps: &Capabilities,
        current: Option<&T>,
        revert_to: Option<T>,
    ) -> Vec<Effect<T>> {
        if let Err(message) = validate(&self.state, &requested, caps, current) {
            return vec![rejected(message)];
        }
        self.id = Some(uuid::Uuid::new_v4().to_string());
        self.previous = None;
        self.snapshot = None;
        // It may come from a file on disk, so it's checked like a new request. The snapshot of
        // what the router has now is the revert target otherwise.
        self.revert_to = revert_to.filter(|c| *c != requested && c.validate(caps).is_ok());
        self.requested = Some(requested);
        self.journal_phase = None;
        self.loss_ignored = false;
        self.last_in_service = false;
        self.session_down = false;
        self.needs_fresh_observation = false;
        self.state = State::Snapshotting;
        vec![Effect::ReadConfig]
    }

    fn on_config_read(&mut self, result: Result<T, RouterError>, now: Instant) -> Vec<Effect<T>> {
        match self.state {
            State::Snapshotting => match result {
                Ok(cfg) if Some(&cfg) == self.requested.as_ref() => {
                    self.state = State::Idle;
                    vec![rejected(
                        "the router already has the requested configuration".to_string(),
                    )]
                }
                Ok(cfg) => {
                    self.previous = Some(self.revert_to.take().unwrap_or_else(|| cfg.clone()));
                    self.snapshot = Some(cfg);
                    self.state = State::Applying;
                    vec![
                        self.journal(JournalPhase::Journaled),
                        Effect::Post(self.requested_cfg()),
                    ]
                }
                Err(e) => {
                    self.state = State::Idle;
                    vec![rejected(format!(
                        "could not read the current configuration: {e}"
                    ))]
                }
            },
            State::Verifying { kind, since, .. } => {
                let is_requested = matches!(&result, Ok(c) if Some(c) == self.requested.as_ref());
                let is_previous = matches!(&result, Ok(c) if Some(c) == self.snapshot.as_ref());
                match kind {
                    ReadKind::AfterApply
                    | ReadKind::AfterUncertainApply
                    | ReadKind::UncertainRecheck
                        if is_requested =>
                    {
                        let verified = self.journal(JournalPhase::Verified);
                        let mut effects = vec![verified];
                        // Same grace clock as Verifying: no fresh window after the verify.
                        effects.extend(self.enter_grace(since, false, now));
                        effects
                    }
                    // One read showing the snapshot isn't proof: the router may apply late.
                    ReadKind::AfterUncertainApply if is_previous => {
                        self.state = State::Verifying {
                            kind: ReadKind::UncertainRecheck,
                            read_due: false,
                            read_at: Some(now + UNCERTAIN_REREAD),
                            since,
                        };
                        vec![Effect::RetryAfter(UNCERTAIN_REREAD)]
                    }
                    ReadKind::UncertainRecheck if is_previous => self.close_not_applied(),
                    ReadKind::Resumed if is_previous => {
                        self.state = State::Idle;
                        vec![Effect::ArchiveJournal]
                    }
                    ReadKind::Resumed if is_requested => self.enter_grace(now, true, now),
                    // The router may report the old config, or fail a read, for a while after
                    // accepting the POST: read again before calling it a mismatch.
                    ReadKind::AfterApply
                        if (is_previous || result.is_err())
                            && now + VERIFY_REREAD < since + VERIFY_WINDOW =>
                    {
                        self.state = State::Verifying {
                            kind: ReadKind::AfterApply,
                            read_due: false,
                            read_at: Some(now + VERIFY_REREAD),
                            since,
                        };
                        vec![Effect::RetryAfter(VERIFY_REREAD)]
                    }
                    _ => self.begin_revert(RevertReason::VerifyMismatch),
                }
            }
            State::Reverting(mut r) if r.stage == RevertStage::Confirming => {
                let target = self.revert_target(r.fallback);
                if matches!(&result, Ok(c) if *c == target) {
                    r.stage = RevertStage::AwaitReattach(now);
                    self.state = State::Reverting(r);
                    vec![]
                } else if matches!(&result, Err(e) if is_session_loss(e)) {
                    // The read-back failed for lack of a session: re-read later, no attempt spent.
                    r.stage = RevertStage::ReadDue;
                    self.state = State::Reverting(r);
                    vec![]
                } else {
                    self.fail_attempt(r, now)
                }
            }
            _ => vec![],
        }
    }

    fn on_post_result(&mut self, result: Result<(), RouterError>, now: Instant) -> Vec<Effect<T>> {
        match self.state {
            State::Applying => match result {
                Ok(()) => {
                    self.state = State::Verifying {
                        kind: ReadKind::AfterApply,
                        read_due: false,
                        read_at: None,
                        since: now,
                    };
                    vec![self.journal(JournalPhase::Applied), Effect::ReadConfig]
                }
                Err(e) if may_have_applied(&e) => {
                    self.state = State::Verifying {
                        kind: ReadKind::AfterUncertainApply,
                        read_due: false,
                        read_at: None,
                        since: now,
                    };
                    vec![Effect::ReadConfig]
                }
                Err(_) => self.close_not_applied(),
            },
            State::Reverting(mut r) if r.stage == RevertStage::Posting => match result {
                Ok(()) => {
                    r.stage = RevertStage::Confirming;
                    self.state = State::Reverting(r);
                    vec![Effect::ReadConfig]
                }
                // No session to post with: RevertPending, the attempt isn't spent.
                Err(e) if is_session_loss(&e) => {
                    r.stage = RevertStage::PostDue;
                    self.state = State::Reverting(r);
                    vec![]
                }
                Err(_) => self.fail_attempt(r, now),
            },
            _ => vec![],
        }
    }

    fn on_observation(
        &mut self,
        obs: Evidence,
        polled_at: Option<Instant>,
        now: Instant,
    ) -> Vec<Effect<T>> {
        let in_service = obs.in_service;
        match self.state {
            State::ReattachGrace {
                since,
                resumed,
                entered,
                settle,
            } => {
                // A poll already under way at the verify may show the old cell: not evidence.
                if polled_at.is_some_and(|t| t < entered) {
                    return vec![];
                }
                let target_ok = self.requested.as_ref().is_none_or(|r| r.evidence_ok(&obs));
                let key = T::settle_key(&obs);
                let settle = if in_service && target_ok {
                    match settle {
                        Some((start, cell)) if cell == key => Some((start, cell)),
                        _ => Some((now, key)),
                    }
                } else {
                    None
                };
                if let Some((start, _)) = settle {
                    if now.saturating_duration_since(start) >= self.timers.settle {
                        self.state = State::Probation {
                            reattach_at: now,
                            loss_start: None,
                        };
                        return vec![
                            self.journal(JournalPhase::Probation),
                            Effect::Event(SafeApplyEvent::Reattached),
                        ];
                    }
                }
                self.state = State::ReattachGrace {
                    since,
                    resumed,
                    entered,
                    settle,
                };
                vec![]
            }
            State::Probation {
                reattach_at,
                loss_start,
            } => {
                let loss_start = if in_service {
                    None
                } else {
                    Some(loss_start.unwrap_or(now))
                };
                self.state = State::Probation {
                    reattach_at,
                    loss_start,
                };
                vec![]
            }
            State::Reverting(r) => match r.stage {
                RevertStage::AwaitReattach(_) if in_service => {
                    self.finish_revert(r.reason, r.fallback)
                }
                _ => self.revert_tick(r, now),
            },
            State::Verifying { .. } | State::RevertFailed { .. } => self.on_tick(now),
            _ => vec![],
        }
    }

    fn on_tick(&mut self, now: Instant) -> Vec<Effect<T>> {
        match self.state {
            State::Verifying {
                kind,
                read_due,
                read_at,
                since,
            } if now < since + self.timers.grace
                && (read_due || read_at.is_some())
                && read_at.is_none_or(|t| now >= t) =>
            {
                self.state = State::Verifying {
                    kind,
                    read_due: false,
                    read_at: None,
                    since,
                };
                vec![Effect::ReadConfig]
            }
            State::Reverting(r) => self.revert_tick(r, now),
            State::RevertFailed { reason, retry_at } if now >= retry_at => {
                self.begin_revert(reason)
            }
            _ => vec![],
        }
    }

    fn on_session_unavailable(&mut self) -> Vec<Effect<T>> {
        match self.state {
            // The POST couldn't be sent: nothing changed.
            State::Applying => self.close_not_applied(),
            State::Snapshotting => {
                self.state = State::Idle;
                vec![rejected("no router session".to_string())]
            }
            State::Verifying {
                kind,
                since,
                read_at,
                ..
            } => {
                self.state = State::Verifying {
                    kind,
                    read_due: true,
                    read_at,
                    since,
                };
                vec![]
            }
            State::Reverting(mut r)
                if matches!(r.stage, RevertStage::Posting | RevertStage::Confirming) =>
            {
                r.stage = if r.stage == RevertStage::Confirming {
                    RevertStage::ReadDue
                } else {
                    RevertStage::PostDue
                };
                self.state = State::Reverting(r);
                vec![]
            }
            // Grace/probation timers keep running (checked in `check_deadlines`).
            _ => vec![],
        }
    }

    /// Runs after every input: the verify/grace deadline, the loss threshold and the probation
    /// end. A revert that falls due without a session becomes `RevertPending`.
    fn check_deadlines(
        &mut self,
        now: Instant,
        session_ok: bool,
        may_commit: bool,
    ) -> Vec<Effect<T>> {
        match self.state {
            State::Verifying { kind, since, .. } if now >= since + self.timers.grace => {
                self.revert_due(no_reattach_reason(kind == ReadKind::Resumed), session_ok)
            }
            State::ReattachGrace { since, resumed, .. } if now >= since + self.timers.grace => {
                if self.revert_on_loss() {
                    self.revert_due(no_reattach_reason(resumed), session_ok)
                } else {
                    self.grace_ended_without_revert(no_reattach_reason(resumed), now)
                }
            }
            State::Probation {
                reattach_at,
                loss_start,
            } if !self.revert_on_loss() => {
                let mut effects = Vec::new();
                if loss_start.is_some_and(|s| now.saturating_duration_since(s) >= self.timers.loss)
                {
                    effects.extend(self.ignore_loss(RevertReason::LossOfService));
                }
                if now >= reattach_at + self.timers.probation {
                    effects.extend(self.commit());
                }
                effects
            }
            State::Probation {
                loss_start: Some(start),
                ..
            } if now.saturating_duration_since(start) >= self.timers.loss => {
                self.revert_due(RevertReason::LossOfService, session_ok)
            }
            State::Probation {
                reattach_at,
                loss_start: None,
            } if may_commit
                && self.last_in_service
                && !self.needs_fresh_observation
                && now >= reattach_at + self.timers.probation =>
            {
                self.commit()
            }
            _ => vec![],
        }
    }

    /// Loss policy off: the grace window ended without settled service. The change moves on to
    /// Probation (its full timer from `now`) instead of reverting.
    fn grace_ended_without_revert(&mut self, reason: RevertReason, now: Instant) -> Vec<Effect<T>> {
        self.state = State::Probation {
            reattach_at: now,
            loss_start: (!self.last_in_service).then_some(now),
        };
        let mut effects = vec![self.journal(JournalPhase::Probation)];
        effects.extend(self.ignore_loss(reason));
        effects
    }

    /// Reports an ignored loss once per change.
    fn ignore_loss(&mut self, reason: RevertReason) -> Vec<Effect<T>> {
        if std::mem::replace(&mut self.loss_ignored, true) {
            vec![]
        } else {
            vec![Effect::LossIgnored(reason)]
        }
    }

    /// The process was suspended: don't count the suspended time, but don't forget a pre-sleep
    /// outage either, and demand a fresh observation before any good-news decision.
    fn reset_after_gap(&mut self, now: Instant) {
        let last_in_service = self.last_in_service;
        match &mut self.state {
            State::Probation { loss_start, .. } => {
                *loss_start = if last_in_service { None } else { Some(now) };
                self.needs_fresh_observation = true;
            }
            State::ReattachGrace { since, settle, .. } => {
                *since = now;
                *settle = None;
                self.needs_fresh_observation = true;
            }
            _ => {}
        }
    }

    /// Leaves `RevertPending`: re-issues the POST, or the read-back if the POST already succeeded.
    fn resume_pending(&mut self, mut r: Revert) -> Vec<Effect<T>> {
        match r.stage {
            RevertStage::PostDue => {
                r.stage = RevertStage::Posting;
                self.state = State::Reverting(r);
                vec![Effect::Post(self.revert_target(r.fallback))]
            }
            RevertStage::ReadDue => {
                r.stage = RevertStage::Confirming;
                self.state = State::Reverting(r);
                vec![Effect::ReadConfig]
            }
            _ => vec![],
        }
    }

    /// Deadlines while reverting: a due retry, or a reverted config that never re-attached.
    /// (A pending POST/read is resumed by `resume_pending`, not by deadlines.)
    fn revert_tick(&mut self, mut r: Revert, now: Instant) -> Vec<Effect<T>> {
        match r.stage {
            RevertStage::RetryAt(at) if now >= at => {
                if r.attempt >= MAX_ATTEMPTS {
                    self.fall_back(r, now)
                } else {
                    r.stage = RevertStage::Posting;
                    self.state = State::Reverting(r);
                    vec![Effect::Post(self.revert_target(r.fallback))]
                }
            }
            RevertStage::AwaitReattach(since) if now >= since + self.timers.grace => {
                if r.fallback {
                    self.revert_failed(r.reason, now)
                } else {
                    self.fall_back(r, now)
                }
            }
            _ => vec![],
        }
    }

    fn fail_attempt(&mut self, mut r: Revert, now: Instant) -> Vec<Effect<T>> {
        r.attempt = r.attempt.saturating_add(1);
        if r.fallback && r.attempt >= MAX_ATTEMPTS {
            return self.revert_failed(r.reason, now);
        }
        let delay = RETRY_DELAYS[usize::from(r.attempt.min(MAX_ATTEMPTS)) - 1];
        r.stage = RevertStage::RetryAt(now + delay);
        self.state = State::Reverting(r);
        vec![Effect::RetryAfter(delay)]
    }

    /// The previous config couldn't be restored: revert to the target's fallback, or, if it has
    /// none, end in `RevertFailed` (retried every 30 s).
    fn fall_back(&mut self, r: Revert, now: Instant) -> Vec<Effect<T>> {
        if self.fallback_target().is_none() {
            return self.revert_failed(r.reason, now);
        }
        self.state = State::Reverting(Revert {
            reason: r.reason,
            attempt: 0,
            fallback: true,
            stage: RevertStage::Posting,
        });
        vec![
            Effect::Post(self.revert_target(true)),
            Effect::Event(SafeApplyEvent::FellBackToAuto),
        ]
    }

    /// A revert is due now; without a session it waits as `RevertPending` (journal written).
    fn revert_due(&mut self, reason: RevertReason, session_ok: bool) -> Vec<Effect<T>> {
        if session_ok {
            return self.begin_revert(reason);
        }
        self.state = State::Reverting(Revert {
            reason,
            attempt: 0,
            fallback: false,
            stage: RevertStage::PostDue,
        });
        vec![self.journal(JournalPhase::Reverting)]
    }

    fn begin_revert(&mut self, reason: RevertReason) -> Vec<Effect<T>> {
        self.state = State::Reverting(Revert {
            reason,
            attempt: 0,
            fallback: false,
            stage: RevertStage::Posting,
        });
        vec![
            self.journal(JournalPhase::Reverting),
            Effect::Post(self.revert_target(false)),
        ]
    }

    fn finish_revert(&mut self, reason: RevertReason, fallback: bool) -> Vec<Effect<T>> {
        self.state = State::Reverted { reason, fallback };
        vec![
            self.journal(JournalPhase::Reverted),
            Effect::ArchiveJournal,
            Effect::Event(SafeApplyEvent::Reverted { reason, fallback }),
        ]
    }

    /// The journal is kept; the whole revert is retried every 30 s while the app runs.
    fn revert_failed(&mut self, reason: RevertReason, now: Instant) -> Vec<Effect<T>> {
        self.state = State::RevertFailed {
            reason,
            retry_at: now + REVERT_FAILED_RETRY,
        };
        vec![
            self.journal(JournalPhase::RevertFailed),
            Effect::Event(SafeApplyEvent::RevertFailed),
            Effect::RetryAfter(REVERT_FAILED_RETRY),
        ]
    }

    fn commit(&mut self) -> Vec<Effect<T>> {
        self.state = State::Committed;
        vec![
            self.journal(JournalPhase::Committed),
            Effect::ArchiveJournal,
            Effect::Event(SafeApplyEvent::Committed),
        ]
    }

    /// The change was never applied: archive the journal, no revert POST.
    fn close_not_applied(&mut self) -> Vec<Effect<T>> {
        self.state = State::Idle;
        vec![
            Effect::ArchiveJournal,
            Effect::Event(SafeApplyEvent::Reverted {
                reason: RevertReason::ApplyRejected,
                fallback: false,
            }),
        ]
    }

    fn enter_grace(&mut self, since: Instant, resumed: bool, entered: Instant) -> Vec<Effect<T>> {
        self.state = State::ReattachGrace {
            since,
            resumed,
            entered,
            settle: None,
        };
        vec![self.journal(JournalPhase::ReattachGrace)]
    }

    fn journal(&mut self, phase: JournalPhase) -> Effect<T> {
        self.journal_phase = Some(phase);
        Effect::WriteJournal(phase)
    }

    fn is_gap(&self, now: Instant) -> bool {
        let threshold = (self.timers.poll_interval * 3).max(MIN_GAP);
        self.last_input_at
            .is_some_and(|last| now.saturating_duration_since(last) > threshold)
    }

    fn requested_cfg(&self) -> T {
        self.requested.clone().expect("requested is set on Start")
    }

    fn fallback_target(&self) -> Option<T> {
        self.previous.as_ref().and_then(T::fallback)
    }

    fn revert_target(&self, fallback: bool) -> T {
        if fallback {
            self.fallback_target()
                .expect("a fallback revert only starts when the target has one")
        } else {
            self.previous
                .clone()
                .expect("previous is set before any POST")
        }
    }

    fn sync_phase(&mut self) {
        self.phase = self.state.phase();
    }
}

fn validate<T: ChangeTarget>(
    state: &State,
    requested: &T,
    caps: &Capabilities,
    current: Option<&T>,
) -> Result<(), String> {
    if !matches!(
        state,
        State::Idle | State::Committed | State::Reverted { .. }
    ) {
        return Err("another change is in progress".to_string());
    }
    requested.validate(caps)?;
    if current == Some(requested) {
        return Err("the requested configuration is already active".to_string());
    }
    Ok(())
}

/// What this router can take at all.
fn check_config(requested: &NetConfig, caps: &Capabilities) -> Result<(), String> {
    let mask = requested.lte_bands;
    if mask.is_empty() {
        return Err("select at least one LTE band".to_string());
    }
    if mask != LteBandSet::AUTO && !mask.is_subset_of(&caps.supported_lte) {
        return Err(format!(
            "unsupported LTE bands: {}",
            mask.intersection(&LteBandSet::from_bits(!caps.supported_lte.bits()))
                .label()
        ));
    }
    if !caps.modes.contains(&requested.network_mode) {
        return Err("network mode isn't supported by this router".to_string());
    }
    if let Some(option) = requested.nr_option {
        if !caps.nr_options.contains(&option) {
            return Err("5G option isn't supported by this router".to_string());
        }
    }
    let raw = &requested.legacy_band_raw;
    if raw.len() > 32 || !raw.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err("the router's legacy band value isn't valid".to_string());
    }
    Ok(())
}

/// Errors after which the router may or may not have applied the POST. API-level refusals mean
/// nothing changed; a timeout, an unreachable router or an unparseable reply don't.
fn may_have_applied(e: &RouterError) -> bool {
    matches!(
        e,
        RouterError::Timeout | RouterError::Unreachable | RouterError::Parse { .. }
    )
}

fn is_session_loss(e: &RouterError) -> bool {
    matches!(
        e,
        RouterError::SessionLost { .. } | RouterError::SessionTakenOver
    )
}

fn no_reattach_reason(resumed: bool) -> RevertReason {
    if resumed {
        RevertReason::ResumedWithoutService
    } else {
        RevertReason::NoReattach
    }
}

fn rejected<T>(message: String) -> Effect<T> {
    Effect::Event(SafeApplyEvent::Rejected { message })
}

fn ceil_secs(d: Duration) -> u32 {
    let secs = d.as_secs() + u64::from(d.subsec_nanos() > 0);
    u32::try_from(secs).unwrap_or(u32::MAX)
}
