//! Refresh IP: a new internet address, by turning mobile data off and back on
//! (`IpMethod::DataSwitch`) or by a manual network search (`IpMethod::NetworkSearch`).
//!
//! A pure state machine, like `safe_apply`: [`IpRefresh::step`] takes an [`IpInput`] and the
//! current time and returns the [`IpEffect`]s for the actor to run, in order. Every driver effect
//! is answered with the matching input before the next tick.
//!
//! A run uses the first method of the list it is started with (the driver's, minus what the
//! router refused this session).
//!
//! Data-switch try: write the data-switch journal (nothing is sent when that fails), post "off",
//! hold data off for 5, 10, 20, 30 or 30 s (by try), post "on", read the switch until it says 1,
//! then read the WAN address every 2 s until the router is online again.
//!
//! Network-search try: send the search (`StartSearch`, answered with `SearchDone` whenever the
//! router answers or the call times out). The router drops its data session while it searches and
//! reconnects by itself; nothing is switched off, so no journal is written. Then the WAN address
//! is read every 2 s, from `SEARCH_SETTLE` after the answer, until the router is online again.
//!
//! With a target ("10" or "10.45") the run tries again, at least `BETWEEN_TRIES` (data switch) or
//! `SEARCH_BETWEEN` (search) later, until the address's range starts with it or `max_tries` (1 to
//! 5) is used up. Without one it is a single try.
//!
//! Rules:
//! - "Off" is posted only from Idle (a start) or Between (the next try), never after an "on"
//!   POST of this run failed: that ends the run once data is back.
//! - Stop, a lost session, or the 12-minute hard cap while data is off turns data on at once.
//! - Every way out of a try that posted "off" passes a read of the switch saying 1. A failed
//!   "on" is retried every 5 s (30 s after 5 failures) for as long as it takes.
//! - The router gets 90 s to come back online after the switch reads 1 (`ONLINE_TIMEOUT`), and
//!   150 s after a search answered (`SEARCH_ONLINE_TIMEOUT`).
//! - A search can't be called back: Stop while searching only marks the run stopped, and it ends
//!   once the router is online again (or the online wait runs out). A lost session while
//!   searching changes nothing, since the router keeps searching.
//! - A search the router refuses within `QUICK_REFUSAL` of being sent (`SearchSent`) ends the run
//!   (`SearchRefused`), with the router's error kept. "Busy" and a lapsed session or token
//!   (`is_token_loss`, which the actor answers with a fresh login) aren't refusals. A later
//!   answer, an error or a timeout goes to the online wait.
//! - When the router refuses "off" (an API error or a lost session with its code) and the run may
//!   also search, the switch is still read: at 1 the journal is archived and the same try goes on
//!   as a search (`MethodFallback`); at 0 data is turned on and the run ends `OffRefused`.
//! - The hard cap (12 minutes, 20 for a run that searches) only stops new tries; it never cuts a
//!   search short.
//! - At most 15 off/on cycles or searches per rolling hour, across runs.
//!
//! Only the range ("a.b") of an address is ever kept here.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{ApiErrorKind, IpMethod, NetConfig, NetworkSearch, RouterError};

/// The most tries one run may make.
pub const MAX_TRIES: u8 = 5;
/// How long data stays off on try 1, 2, 3, 4 and 5.
pub const OFF_HOLDS: [Duration; MAX_TRIES as usize] = [
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(20),
    Duration::from_secs(30),
    Duration::from_secs(30),
];
/// At least this long between the router coming back and the next "off".
pub const BETWEEN_TRIES: Duration = Duration::from_secs(10);
/// The router gets this long to come back online after the switch reads 1.
pub const ONLINE_TIMEOUT: Duration = Duration::from_secs(90);
/// No new try starts, and data off is cut short, after this long.
pub const HARD_CAP: Duration = Duration::from_secs(12 * 60);
/// The router gets this long to come back online after a search answered.
pub const SEARCH_ONLINE_TIMEOUT: Duration = Duration::from_secs(150);
/// After a search answered, the WAN address is first read this much later: a router may only
/// drop the link after it answers.
pub const SEARCH_SETTLE: Duration = Duration::from_secs(10);
/// At least this long between the router coming back from a search and the next one.
pub const SEARCH_BETWEEN: Duration = Duration::from_secs(20);
/// The hard cap of a run that searches.
pub const SEARCH_HARD_CAP: Duration = Duration::from_secs(20 * 60);
/// A search refused faster than this was refused by the router, not run.
pub const QUICK_REFUSAL: Duration = Duration::from_secs(5);
/// The actor gives the driver's search call this long before it answers `Timeout` itself. The
/// Huawei driver times out at 180 s.
pub const SEARCH_CALL_LIMIT: Duration = Duration::from_secs(200);
/// Off/on cycles allowed per rolling hour.
pub const CYCLES_PER_HOUR: usize = 15;
const HOUR: Duration = Duration::from_secs(60 * 60);
/// The WAN address is read this often while waiting for the router to come back.
pub const WAN_READ_EVERY: Duration = Duration::from_secs(2);
/// A failed "on" (or a failed read of the switch) is retried after this long.
pub const ON_RETRY: Duration = Duration::from_secs(5);
/// After `ON_RETRY_SLOW_AFTER` failures in a row, "on" is retried this often.
pub const ON_RETRY_SLOW: Duration = Duration::from_secs(30);
const ON_RETRY_SLOW_AFTER: u32 = 5;
/// A driver effect the actor couldn't send is tried again after this long.
const DEFERRED_RETRY: Duration = Duration::from_secs(1);

/// The refusal for a target that isn't one or two numbers from 0 to 255.
pub const IP_TARGET_INVALID: &str = "the address start must be one or two numbers from 0 to 255";
/// The refusal for a try count outside 1 to 5.
pub const IP_TRIES_INVALID: &str = "the number of tries must be from 1 to 5";
/// The refusal when the driver has no way to refresh the IP, or the router refused them all.
pub const IP_NO_METHOD: &str = "this router has no way to refresh the IP";
/// The refusal when 15 off/on cycles already ran in the last hour.
pub const IP_REFRESH_COOLDOWN: &str = "too many IP refreshes in the last hour";
/// The refusal when the address already starts with the target.
pub const IP_ALREADY_MATCHES: &str = "the address already starts with that";

/// What the address's range should start with: one or two numbers ("10" or "10.45").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpTarget(Vec<u8>);

impl IpTarget {
    /// One or two numbers from 0 to 255, separated by a dot. A trailing dot is allowed; a
    /// third number is refused, since only the first two are ever kept.
    pub fn parse(raw: &str) -> Result<Self, RouterError> {
        let invalid = || RouterError::Validation {
            message: IP_TARGET_INVALID.to_string(),
        };
        let s = raw.trim();
        let s = s.strip_suffix('.').unwrap_or(s);
        let parts: Vec<&str> = s.split('.').collect();
        if parts.is_empty() || parts.len() > 2 {
            return Err(invalid());
        }
        let mut octets = Vec::with_capacity(parts.len());
        for p in parts {
            if p.is_empty() || p.len() > 3 || !p.bytes().all(|b| b.is_ascii_digit()) {
                return Err(invalid());
            }
            octets.push(p.parse::<u8>().map_err(|_| invalid())?);
        }
        Ok(Self(octets))
    }

    /// Whether a range ("a.b") starts with this target.
    pub fn matches(&self, range: &str) -> bool {
        let got: Vec<Option<u8>> = range.split('.').map(|p| p.parse().ok()).collect();
        self.0
            .iter()
            .enumerate()
            .all(|(i, want)| got.get(i).copied().flatten() == Some(*want))
    }
}

impl std::fmt::Display for IpTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let parts: Vec<String> = self.0.iter().map(u8::to_string).collect();
        f.write_str(&parts.join("."))
    }
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum IpRefreshOutcome {
    /// The range starts with the target.
    Matched,
    /// Every try ran and none matched.
    NotMatched,
    /// No target: the range changed.
    Changed,
    /// No target: the same range came back.
    Unchanged,
    /// The user stopped it; data is back on.
    Stopped,
    /// Data is on (or the search ended), but the router didn't come back online in time: 90 s
    /// after data was back on, 150 s after a search.
    OnlineTimeout,
    /// The run reached 12 minutes.
    HardCap,
    /// The session was lost while data was off; data is back on.
    SessionLost,
    /// The router refused "off"; data is on.
    OffRefused,
    /// The data-switch journal couldn't be written, so nothing was sent.
    JournalFailed,
    /// Turning data back on needed more than one try, so no more tries were made.
    DataOnFailed,
    /// 15 cycles ran in the last hour, so no more tries were made.
    RateLimited,
    /// The app was closed with data off; data is back on.
    Resumed,
    /// The router refused the network search at once; nothing changed.
    SearchRefused,
}

/// The phase of a run, for the banner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum IpRefreshPhase {
    Idle,
    TurningOff,
    Off,
    TurningOn,
    Confirming,
    WaitingOnline,
    Between,
    /// A network search is running; it can't be stopped midway.
    Searching,
}

/// The last run's result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IpRefreshResult {
    pub outcome: IpRefreshOutcome,
    pub tries: u8,
    pub target: Option<String>,
    /// The range at the start, and at the end ("a.b").
    pub from: Option<String>,
    pub to: Option<String>,
    /// The method the run ended with (a refused "off" can hand over to a search).
    pub method: IpMethod,
    /// The router's refusal that ended the run (`OffRefused`, `SearchRefused`), with its code.
    #[serde(default)]
    pub refusal: Option<RouterError>,
    /// The band config read after a search differs from the one before the run (set by the
    /// actor). Nothing is changed back by itself; the UI offers it.
    #[serde(default)]
    pub bands_changed: bool,
    /// The config from before the run, when `bands_changed`.
    #[serde(default)]
    pub bands_before: Option<NetConfig>,
}

/// The live view of Refresh IP, for the snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IpRefreshView {
    /// A run is open: from the start until data is confirmed back on and the run ended.
    pub active: bool,
    pub phase: IpRefreshPhase,
    /// The open run's method; `None` when idle.
    pub method: Option<IpMethod>,
    /// The current try, from 1; 0 when idle.
    pub try_n: u8,
    pub max_tries: u8,
    pub target: Option<String>,
    pub from_range: Option<String>,
    pub current_range: Option<String>,
    pub seconds_in_phase: u32,
    /// Seconds left in a timed phase (the hold, the wait between tries, the online wait).
    pub seconds_left: Option<u32>,
    /// Failed "on" POSTs in this run.
    pub data_on_failures: u32,
    /// Stop was pressed: data is being turned back on.
    pub stopping: bool,
    /// This run turns data back on after the app was closed with it off.
    pub resumed: bool,
    pub last: Option<IpRefreshResult>,
    /// A data-switch journal for another router, or one that can't be read: shown, never
    /// acted on. Set by the actor.
    pub journal_blocked: bool,
    /// Off/on cycles (or searches) left in the rolling hour.
    pub cycles_left: u8,
}

/// Inputs to [`IpRefresh::step`].
#[derive(Debug, Clone, PartialEq)]
pub enum IpInput {
    Stop,
    Tick,
    /// The answer to `WriteJournal`.
    JournalWritten(bool),
    /// The answer to `PostSwitch`.
    PostResult(Result<(), RouterError>),
    /// The answer to `ReadSwitch`: `true` is on.
    SwitchRead(Result<bool, RouterError>),
    /// The answer to `ReadWan`: the range, when the router is online with an address.
    WanRead(Option<String>),
    /// The search was sent (after the actor refreshed the session, which can take a while):
    /// its time counts from here, so a slow refresh doesn't hide a quick refusal.
    SearchSent,
    /// The answer to `StartSearch`. `service_dropped`: a poll during the search saw no service
    /// or no internet address.
    SearchDone {
        result: Result<NetworkSearch, RouterError>,
        service_dropped: bool,
    },
    /// The session or the router is gone (a failed poll or login).
    SessionUnavailable,
    /// The answer to a driver effect the actor couldn't send (not logged in). Not a failure:
    /// it is sent again a second later.
    Deferred,
}

/// Effects for the actor, run in order.
#[derive(Debug, Clone, PartialEq)]
pub enum IpEffect {
    /// Write `pending-data-switch.json` (fsynced); answered with `JournalWritten`.
    WriteJournal {
        id: String,
    },
    /// Post the switch; answered with `PostResult`. Never preempted.
    PostSwitch(bool),
    /// Read the switch; answered with `SwitchRead`.
    ReadSwitch,
    /// Read the WAN address; answered with `WanRead`.
    ReadWan,
    /// Start a network search without waiting for it; answered with `SearchDone` when it ends,
    /// or with `Deferred` when it couldn't be sent.
    StartSearch,
    /// The switch read 1: move the journal to the archive.
    ArchiveJournal,
    Event(IpRefreshEvent),
}

/// What happened, for `events.jsonl` and alerts. Ranges only.
#[derive(Debug, Clone, PartialEq)]
pub enum IpRefreshEvent {
    Started {
        target: Option<String>,
        max_tries: u8,
        from: Option<String>,
        method: IpMethod,
    },
    Try {
        n: u8,
        from: Option<String>,
        to: Option<String>,
        /// 0 for a search.
        off_s: u32,
        method: IpMethod,
    },
    DataOnFailed {
        attempt: u32,
        error: RouterError,
    },
    /// A network search ended: an answer (with how many networks it listed), an error or a
    /// timeout.
    Search {
        n: u8,
        seconds: u32,
        networks: Option<u8>,
        timed_out: bool,
        service_dropped: bool,
        error: Option<RouterError>,
    },
    /// The router refused "off", so this try goes on as a network search.
    MethodFallback {
        from: IpMethod,
        to: IpMethod,
        error: RouterError,
    },
    Ended(IpRefreshResult),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum P {
    Idle,
    TurningOff,
    Off { until: Instant },
    TurningOn { next_at: Instant },
    Confirming { next_at: Instant },
    WaitingOnline { since: Instant, next_read: Instant },
    Between { until: Instant },
    Searching { since: Instant },
}

#[derive(Debug, Clone)]
struct Run {
    id: String,
    target: Option<IpTarget>,
    max_tries: u8,
    try_n: u8,
    started: Instant,
    phase_since: Instant,
    from_range: Option<String>,
    /// The range before the current try.
    try_from: Option<String>,
    current_range: Option<String>,
    off_for: Duration,
    /// Set when the run must end once data reads back on.
    end: Option<IpRefreshOutcome>,
    stopping: bool,
    on_failures: u32,
    /// A driver effect was sent and not answered yet.
    pending: bool,
    resumed: bool,
    method: IpMethod,
    /// The methods this run may use, in order.
    methods: Vec<IpMethod>,
    /// The router's refusal that ends the run.
    refusal: Option<RouterError>,
    /// "Off" was refused and the run may search: the refusal, until the switch reads 1.
    fallback: Option<RouterError>,
    /// When the last search answered.
    search_ended_at: Option<Instant>,
}

/// Refresh IP's state, kept by the actor across runs (for the hourly cycle budget).
#[derive(Debug, Default)]
pub struct IpRefresh {
    phase: Option<P>,
    run: Option<Run>,
    cycles: VecDeque<Instant>,
    last: Option<IpRefreshResult>,
}

fn secs(d: Duration) -> u32 {
    u32::try_from(d.as_secs()).unwrap_or(u32::MAX)
}

impl IpRefresh {
    pub fn new() -> Self {
        Self::default()
    }

    fn p(&self) -> P {
        self.phase.unwrap_or(P::Idle)
    }

    /// A run is open.
    pub fn active(&self) -> bool {
        self.run.is_some()
    }

    /// Data may be off right now: from the "off" POST until the switch reads 1.
    pub fn data_may_be_off(&self) -> bool {
        matches!(
            self.p(),
            P::Off { .. } | P::TurningOn { .. } | P::Confirming { .. }
        )
    }

    /// The id of the run's journal, when one may be on file.
    pub fn journal_id(&self) -> Option<&str> {
        self.run.as_ref().map(|r| r.id.as_str())
    }

    /// Off/on cycles left in the rolling hour.
    pub fn cycles_left(&mut self, now: Instant) -> usize {
        while self
            .cycles
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= HOUR)
        {
            self.cycles.pop_front();
        }
        self.cycles_left_at(now)
    }

    fn cycles_left_at(&self, now: Instant) -> usize {
        let recent = self
            .cycles
            .iter()
            .filter(|t| now.saturating_duration_since(**t) < HOUR)
            .count();
        CYCLES_PER_HOUR.saturating_sub(recent)
    }

    /// Starts a run from the current range with the first of `methods`. Refuses no method, a
    /// bad try count, a target already met, a run that is open, and an empty hourly budget;
    /// sends nothing then.
    pub fn start(
        &mut self,
        target: Option<IpTarget>,
        max_tries: u8,
        range: Option<String>,
        methods: &[IpMethod],
        now: Instant,
    ) -> Result<Vec<IpEffect>, RouterError> {
        let refuse = |m: &str| {
            Err(RouterError::Validation {
                message: m.to_string(),
            })
        };
        if self.active() {
            return refuse(crate::actor::IP_REFRESH_RUNNING);
        }
        let Some(&method) = methods.first() else {
            return refuse(IP_NO_METHOD);
        };
        if !(1..=MAX_TRIES).contains(&max_tries) {
            return refuse(IP_TRIES_INVALID);
        }
        if let (Some(t), Some(r)) = (&target, &range) {
            if t.matches(r) {
                return refuse(IP_ALREADY_MATCHES);
            }
        }
        if self.cycles_left(now) == 0 {
            return refuse(IP_REFRESH_COOLDOWN);
        }
        let max_tries = if target.is_some() { max_tries } else { 1 };
        let id = uuid::Uuid::new_v4().to_string();
        let started = IpRefreshEvent::Started {
            target: target.as_ref().map(ToString::to_string),
            max_tries,
            from: range.clone(),
            method,
        };
        self.run = Some(Run {
            id: id.clone(),
            target,
            max_tries,
            try_n: 1,
            started: now,
            phase_since: now,
            from_range: range.clone(),
            try_from: range.clone(),
            current_range: range,
            off_for: Duration::ZERO,
            end: None,
            stopping: false,
            on_failures: 0,
            pending: true,
            resumed: false,
            method,
            methods: methods.to_vec(),
            refusal: None,
            fallback: None,
            search_ended_at: None,
        });
        let mut out = vec![IpEffect::Event(started)];
        match method {
            IpMethod::DataSwitch => {
                self.phase = Some(P::TurningOff);
                out.push(IpEffect::WriteJournal { id });
            }
            IpMethod::NetworkSearch => self.begin_search(now, &mut out),
        }
        Ok(out)
    }

    /// The app was closed with data off (journal `id` on file): turn it back on. The "on" is
    /// sent on the first tick; the actor answers it with a failure until it is logged in.
    pub fn resume(&mut self, id: String, now: Instant) {
        self.run = Some(Run {
            id,
            target: None,
            max_tries: 1,
            try_n: 1,
            started: now,
            phase_since: now,
            from_range: None,
            try_from: None,
            current_range: None,
            off_for: Duration::ZERO,
            end: Some(IpRefreshOutcome::Resumed),
            stopping: false,
            on_failures: 0,
            pending: false,
            resumed: true,
            method: IpMethod::DataSwitch,
            methods: vec![IpMethod::DataSwitch],
            refusal: None,
            fallback: None,
            search_ended_at: None,
        });
        self.phase = Some(P::TurningOn { next_at: now });
    }

    pub fn step(&mut self, input: IpInput, now: Instant) -> Vec<IpEffect> {
        let mut out = Vec::new();
        if self.run.is_none() {
            return out;
        }
        match input {
            IpInput::Tick => self.tick(now, &mut out),
            IpInput::Stop => self.stop(now, &mut out),
            // While searching the router keeps searching whatever happens to the session.
            IpInput::SessionUnavailable => {
                if matches!(self.p(), P::Off { .. }) {
                    self.mark_end(IpRefreshOutcome::SessionLost);
                    self.turn_on(now, &mut out);
                }
            }
            IpInput::JournalWritten(ok) => {
                if self.p() != P::TurningOff {
                    return out;
                }
                if ok {
                    self.set_pending(true);
                    out.push(IpEffect::PostSwitch(false));
                } else {
                    // Nothing was sent in this try, and data was on before it.
                    self.finish(IpRefreshOutcome::JournalFailed, &mut out);
                }
            }
            IpInput::PostResult(r) => self.post_result(r, now, &mut out),
            IpInput::SwitchRead(r) => self.switch_read(r, now, &mut out),
            IpInput::WanRead(range) => self.wan_read(range, now, &mut out),
            IpInput::SearchSent => {
                if matches!(self.p(), P::Searching { .. }) {
                    self.set_phase(P::Searching { since: now }, now);
                }
            }
            IpInput::SearchDone {
                result,
                service_dropped,
            } => self.search_done(result, service_dropped, now, &mut out),
            IpInput::Deferred => self.deferred(now, &mut out),
        }
        out
    }

    fn run_mut(&mut self) -> &mut Run {
        self.run.as_mut().expect("a run is open")
    }

    fn set_pending(&mut self, on: bool) {
        if let Some(r) = self.run.as_mut() {
            r.pending = on;
        }
    }

    fn set_phase(&mut self, p: P, now: Instant) {
        self.phase = Some(p);
        if let Some(r) = self.run.as_mut() {
            r.phase_since = now;
        }
    }

    /// Records why the run ends; the first reason wins.
    fn mark_end(&mut self, why: IpRefreshOutcome) {
        let r = self.run_mut();
        r.end.get_or_insert(why);
    }

    fn turn_on(&mut self, now: Instant, out: &mut Vec<IpEffect>) {
        self.set_phase(P::TurningOn { next_at: now }, now);
        self.set_pending(true);
        out.push(IpEffect::PostSwitch(true));
    }

    fn confirm(&mut self, now: Instant, out: &mut Vec<IpEffect>) {
        self.set_phase(P::Confirming { next_at: now }, now);
        self.set_pending(true);
        out.push(IpEffect::ReadSwitch);
    }

    /// The open run's method.
    pub fn method(&self) -> Option<IpMethod> {
        self.run.as_ref().map(|r| r.method)
    }

    /// A network search is running (sent, not answered yet).
    pub fn searching(&self) -> bool {
        matches!(self.p(), P::Searching { .. }) && self.run.is_some()
    }

    /// The actor found the band config changed after a run that searched: recorded in the last
    /// result, with the config from before.
    pub fn note_bands_changed(&mut self, before: NetConfig) {
        if let Some(last) = self.last.as_mut() {
            last.bands_changed = true;
            last.bands_before = Some(before);
        }
    }

    fn over_cap(&self, now: Instant) -> bool {
        self.run.as_ref().is_some_and(|r| {
            let cap = match r.method {
                IpMethod::DataSwitch => HARD_CAP,
                IpMethod::NetworkSearch => SEARCH_HARD_CAP,
            };
            now.saturating_duration_since(r.started) >= cap
        })
    }

    fn online_timeout(&self) -> Duration {
        match self.method() {
            Some(IpMethod::NetworkSearch) => SEARCH_ONLINE_TIMEOUT,
            _ => ONLINE_TIMEOUT,
        }
    }

    /// Sends a search for the current try: one cycle of the hourly budget.
    fn begin_search(&mut self, now: Instant, out: &mut Vec<IpEffect>) {
        self.cycles.push_back(now);
        self.set_phase(P::Searching { since: now }, now);
        self.set_pending(true);
        out.push(IpEffect::StartSearch);
    }

    fn tick(&mut self, now: Instant, out: &mut Vec<IpEffect>) {
        if self.run.as_ref().is_some_and(|r| r.pending) {
            return;
        }
        match self.p() {
            P::Idle | P::TurningOff | P::Searching { .. } => {}
            P::Off { until } => {
                if self.over_cap(now) {
                    self.mark_end(IpRefreshOutcome::HardCap);
                    self.turn_on(now, out);
                } else if now >= until {
                    self.turn_on(now, out);
                }
            }
            P::TurningOn { next_at } => {
                if now >= next_at {
                    self.set_pending(true);
                    out.push(IpEffect::PostSwitch(true));
                }
            }
            P::Confirming { next_at } => {
                if now >= next_at {
                    self.set_pending(true);
                    out.push(IpEffect::ReadSwitch);
                }
            }
            P::WaitingOnline { since, next_read } => {
                let settling = self.run.as_ref().is_some_and(|r| {
                    r.method == IpMethod::NetworkSearch
                        && r.search_ended_at
                            .is_some_and(|t| now.saturating_duration_since(t) < SEARCH_SETTLE)
                });
                if now.saturating_duration_since(since) >= self.online_timeout() {
                    self.finish(IpRefreshOutcome::OnlineTimeout, out);
                } else if self.over_cap(now) {
                    self.finish(IpRefreshOutcome::HardCap, out);
                } else if now >= next_read && !settling {
                    // The same phase: its start time stays.
                    self.phase = Some(P::WaitingOnline {
                        since,
                        next_read: now + WAN_READ_EVERY,
                    });
                    self.set_pending(true);
                    out.push(IpEffect::ReadWan);
                }
            }
            P::Between { until } => {
                if self.over_cap(now) {
                    self.finish(IpRefreshOutcome::HardCap, out);
                } else if now >= until {
                    if self.cycles_left(now) == 0 {
                        self.finish(IpRefreshOutcome::RateLimited, out);
                        return;
                    }
                    if self.method() == Some(IpMethod::NetworkSearch) {
                        self.run_mut().try_n += 1;
                        self.begin_search(now, out);
                        return;
                    }
                    let id = {
                        let r = self.run_mut();
                        r.try_n += 1;
                        r.id = uuid::Uuid::new_v4().to_string();
                        r.id.clone()
                    };
                    self.set_phase(P::TurningOff, now);
                    self.set_pending(true);
                    out.push(IpEffect::WriteJournal { id });
                }
            }
        }
    }

    fn stop(&mut self, now: Instant, out: &mut Vec<IpEffect>) {
        self.run_mut().stopping = true;
        match self.p() {
            P::Off { .. } => {
                self.mark_end(IpRefreshOutcome::Stopped);
                self.turn_on(now, out);
            }
            // A search can't be called back: the run ends once the router is back.
            P::Searching { .. } => self.mark_end(IpRefreshOutcome::Stopped),
            P::WaitingOnline { .. } if self.method() == Some(IpMethod::NetworkSearch) => {
                self.mark_end(IpRefreshOutcome::Stopped);
            }
            // Data is on: nothing to undo.
            P::WaitingOnline { .. } | P::Between { .. } => {
                self.finish(IpRefreshOutcome::Stopped, out);
            }
            // Turning on already; it ends once the switch reads 1.
            P::TurningOff | P::TurningOn { .. } | P::Confirming { .. } => {
                self.mark_end(IpRefreshOutcome::Stopped);
            }
            P::Idle => {}
        }
    }

    fn post_result(&mut self, r: Result<(), RouterError>, now: Instant, out: &mut Vec<IpEffect>) {
        self.set_pending(false);
        match self.p() {
            P::TurningOff => match r {
                Ok(()) => {
                    self.cycles.push_back(now);
                    let hold = {
                        let run = self.run_mut();
                        let i = usize::from(run.try_n.clamp(1, MAX_TRIES)) - 1;
                        run.off_for = OFF_HOLDS[i];
                        run.off_for
                    };
                    if self.run.as_ref().is_some_and(|r| r.end.is_some()) || self.over_cap(now) {
                        self.mark_end(IpRefreshOutcome::HardCap);
                        self.turn_on(now, out);
                    } else {
                        self.set_phase(P::Off { until: now + hold }, now);
                    }
                }
                // The router may or may not have turned data off: find out, and turn it on. A
                // refusal hands the try to a search when the run may search.
                Err(e) => {
                    let run = self.run_mut();
                    let fallback = is_refusal(&e)
                        && run.method == IpMethod::DataSwitch
                        && run.methods.contains(&IpMethod::NetworkSearch);
                    if fallback {
                        run.fallback = Some(e);
                    } else {
                        run.refusal = Some(e);
                        self.mark_end(IpRefreshOutcome::OffRefused);
                    }
                    self.confirm(now, out);
                }
            },
            P::TurningOn { .. } => match r {
                Ok(()) => self.confirm(now, out),
                Err(error) => {
                    let attempt = {
                        let run = self.run_mut();
                        run.on_failures += 1;
                        run.on_failures
                    };
                    if !self.run.as_ref().is_some_and(|r| r.resumed) {
                        self.mark_end(IpRefreshOutcome::DataOnFailed);
                    }
                    out.push(IpEffect::Event(IpRefreshEvent::DataOnFailed {
                        attempt,
                        error,
                    }));
                    let wait = if attempt >= ON_RETRY_SLOW_AFTER {
                        ON_RETRY_SLOW
                    } else {
                        ON_RETRY
                    };
                    self.set_phase(
                        P::TurningOn {
                            next_at: now + wait,
                        },
                        now,
                    );
                }
            },
            _ => {}
        }
    }

    fn switch_read(&mut self, r: Result<bool, RouterError>, now: Instant, out: &mut Vec<IpEffect>) {
        self.set_pending(false);
        if !matches!(self.p(), P::Confirming { .. }) {
            return;
        }
        match r {
            Ok(true) => {
                out.push(IpEffect::ArchiveJournal);
                let fallback = self.run_mut().fallback.take();
                match (self.run.as_ref().and_then(|r| r.end), fallback) {
                    (Some(why), _) => self.finish(why, out),
                    (None, Some(error)) => {
                        out.push(IpEffect::Event(IpRefreshEvent::MethodFallback {
                            from: IpMethod::DataSwitch,
                            to: IpMethod::NetworkSearch,
                            error,
                        }));
                        self.run_mut().method = IpMethod::NetworkSearch;
                        self.begin_search(now, out);
                    }
                    (None, None) => self.set_phase(
                        P::WaitingOnline {
                            since: now,
                            next_read: now + WAN_READ_EVERY,
                        },
                        now,
                    ),
                }
            }
            Ok(false) => {
                // "Off" went through after all: no search; data goes back on and the run ends.
                if let Some(e) = self.run_mut().fallback.take() {
                    self.run_mut().refusal = Some(e);
                    self.mark_end(IpRefreshOutcome::OffRefused);
                }
                self.turn_on(now, out);
            }
            Err(_) => self.set_phase(
                P::Confirming {
                    next_at: now + ON_RETRY,
                },
                now,
            ),
        }
    }

    fn wan_read(&mut self, range: Option<String>, now: Instant, out: &mut Vec<IpEffect>) {
        self.set_pending(false);
        if !matches!(self.p(), P::WaitingOnline { .. }) {
            return;
        }
        let Some(range) = range else { return };
        let (n, from, off_s, target, max_tries, start_from, method) = {
            let r = self.run_mut();
            r.current_range = Some(range.clone());
            let from = r.try_from.replace(range.clone());
            let off_s = match r.method {
                IpMethod::DataSwitch => secs(r.off_for),
                IpMethod::NetworkSearch => 0,
            };
            (
                r.try_n,
                from,
                off_s,
                r.target.clone(),
                r.max_tries,
                r.from_range.clone(),
                r.method,
            )
        };
        out.push(IpEffect::Event(IpRefreshEvent::Try {
            n,
            from,
            to: Some(range.clone()),
            off_s,
            method,
        }));
        let stopped = self.run.as_ref().and_then(|r| r.end);
        let outcome = match &target {
            _ if stopped.is_some() => stopped,
            None if start_from.as_deref() == Some(range.as_str()) => {
                Some(IpRefreshOutcome::Unchanged)
            }
            None => Some(IpRefreshOutcome::Changed),
            Some(t) if t.matches(&range) => Some(IpRefreshOutcome::Matched),
            Some(_) if n >= max_tries => Some(IpRefreshOutcome::NotMatched),
            Some(_) => None,
        };
        let between = match self.method() {
            Some(IpMethod::NetworkSearch) => SEARCH_BETWEEN,
            _ => BETWEEN_TRIES,
        };
        match outcome {
            Some(o) => self.finish(o, out),
            None => self.set_phase(
                P::Between {
                    until: now + between,
                },
                now,
            ),
        }
    }

    fn search_done(
        &mut self,
        result: Result<NetworkSearch, RouterError>,
        service_dropped: bool,
        now: Instant,
        out: &mut Vec<IpEffect>,
    ) {
        self.set_pending(false);
        let P::Searching { since } = self.p() else {
            return;
        };
        let took = now.saturating_duration_since(since);
        let (networks, error) = match &result {
            Ok(found) => (found.networks, None),
            Err(e) => (None, Some(e.clone())),
        };
        out.push(IpEffect::Event(IpRefreshEvent::Search {
            n: self.run_mut().try_n,
            seconds: secs(took),
            networks,
            timed_out: error == Some(RouterError::Timeout),
            service_dropped,
            error: error.clone(),
        }));
        if let Some(e) = error {
            if search_refused(&e) && took < QUICK_REFUSAL {
                self.run_mut().refusal = Some(e);
                self.finish(IpRefreshOutcome::SearchRefused, out);
                return;
            }
        }
        self.run_mut().search_ended_at = Some(now);
        self.set_phase(
            P::WaitingOnline {
                since: now,
                next_read: now + SEARCH_SETTLE,
            },
            now,
        );
    }

    fn deferred(&mut self, now: Instant, out: &mut Vec<IpEffect>) {
        self.set_pending(false);
        let again = now + DEFERRED_RETRY;
        let p = match self.p() {
            P::TurningOn { .. } => P::TurningOn { next_at: again },
            P::Confirming { .. } => P::Confirming { next_at: again },
            P::WaitingOnline { since, .. } => P::WaitingOnline {
                since,
                next_read: again,
            },
            // The search wasn't sent: its cycle is given back, and nothing changed.
            P::Searching { .. } => {
                self.cycles.pop_back();
                self.finish(IpRefreshOutcome::SessionLost, out);
                return;
            }
            // "Off" wasn't sent: the run ends with data on, as it was.
            P::TurningOff => {
                out.push(IpEffect::ArchiveJournal);
                self.finish(IpRefreshOutcome::SessionLost, out);
                return;
            }
            other => other,
        };
        self.phase = Some(p);
    }

    fn finish(&mut self, outcome: IpRefreshOutcome, out: &mut Vec<IpEffect>) {
        let Some(r) = self.run.take() else { return };
        self.phase = Some(P::Idle);
        let result = IpRefreshResult {
            outcome,
            tries: r.try_n,
            target: r.target.as_ref().map(ToString::to_string),
            from: r.from_range,
            to: r.current_range,
            method: r.method,
            refusal: r.refusal,
            bands_changed: false,
            bands_before: None,
        };
        self.last = Some(result.clone());
        out.push(IpEffect::Event(IpRefreshEvent::Ended(result)));
    }

    /// The live view. `journal_blocked` is the actor's.
    pub fn view(&self, now: Instant, journal_blocked: bool) -> IpRefreshView {
        let cycles_left = u8::try_from(self.cycles_left_at(now)).unwrap_or(u8::MAX);
        let phase = self.p();
        let name = match phase {
            P::Idle => IpRefreshPhase::Idle,
            P::TurningOff => IpRefreshPhase::TurningOff,
            P::Off { .. } => IpRefreshPhase::Off,
            P::TurningOn { .. } => IpRefreshPhase::TurningOn,
            P::Confirming { .. } => IpRefreshPhase::Confirming,
            P::WaitingOnline { .. } => IpRefreshPhase::WaitingOnline,
            P::Between { .. } => IpRefreshPhase::Between,
            P::Searching { .. } => IpRefreshPhase::Searching,
        };
        let left = |until: Instant| Some(secs(until.saturating_duration_since(now)));
        let seconds_left = match phase {
            P::Off { until } | P::Between { until } => left(until),
            P::WaitingOnline { since, .. } => left(since + self.online_timeout()),
            _ => None,
        };
        let run = self.run.as_ref();
        IpRefreshView {
            active: run.is_some(),
            phase: if run.is_some() {
                name
            } else {
                IpRefreshPhase::Idle
            },
            method: run.map(|r| r.method),
            try_n: run.map_or(0, |r| r.try_n),
            max_tries: run.map_or(0, |r| r.max_tries),
            target: run.and_then(|r| r.target.as_ref().map(ToString::to_string)),
            from_range: run.and_then(|r| r.from_range.clone()),
            current_range: run.and_then(|r| r.current_range.clone()),
            seconds_in_phase: run.map_or(0, |r| secs(now.saturating_duration_since(r.phase_since))),
            seconds_left: run.and(seconds_left),
            data_on_failures: run.map_or(0, |r| r.on_failures),
            stopping: run.is_some_and(|r| r.stopping),
            resumed: run.is_some_and(|r| r.resumed),
            last: self.last.clone(),
            journal_blocked,
            cycles_left,
        }
    }

    /// The last run's result, if any.
    pub fn last(&self) -> Option<&IpRefreshResult> {
        self.last.as_ref()
    }
}

/// The router answered with a refusal of its own: an API error, or a lost session with its code.
/// A session or token that lapsed (no code, or Huawei's 125001 to 125003): the actor logs in
/// again. It says nothing about whether the router allows the call.
pub fn is_token_loss(e: &RouterError) -> bool {
    matches!(
        e,
        RouterError::SessionLost {
            code: None | Some(125001..=125003)
        }
    )
}

/// An answer to the search that means the router won't run it: an API refusal but "busy", a
/// session-type refusal that isn't a lapsed session or token (such as 100003, no rights), or a
/// call the driver can't make.
fn search_refused(e: &RouterError) -> bool {
    match e {
        RouterError::Api { kind, .. } => *kind != ApiErrorKind::Busy,
        RouterError::SessionLost { .. } => !is_token_loss(e),
        RouterError::Validation { .. } | RouterError::Unsupported => true,
        _ => false,
    }
}

fn is_refusal(e: &RouterError) -> bool {
    matches!(
        e,
        RouterError::Api { .. } | RouterError::SessionLost { code: Some(_) }
    )
}
