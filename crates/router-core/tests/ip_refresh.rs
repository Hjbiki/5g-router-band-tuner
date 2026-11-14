//! The Refresh IP state machine, driven by hand: every driver effect is answered by the test.

use std::time::{Duration, Instant};

use router_core::ip_refresh::{
    IpEffect, IpInput, IpRefresh, IpRefreshEvent, IpRefreshOutcome, IpRefreshPhase, IpTarget,
    CYCLES_PER_HOUR, HARD_CAP, IP_ALREADY_MATCHES, IP_REFRESH_COOLDOWN, IP_TARGET_INVALID,
    IP_TRIES_INVALID, OFF_HOLDS, ONLINE_TIMEOUT, QUICK_REFUSAL, SEARCH_BETWEEN,
    SEARCH_ONLINE_TIMEOUT, SEARCH_SETTLE,
};
use router_core::{IpMethod, NetworkSearch, RouterError};

const DS: &[IpMethod] = &[IpMethod::DataSwitch];

/// A run under test, with a clock the test moves.
struct Rig {
    m: IpRefresh,
    now: Instant,
    /// Every effect so far, in order.
    log: Vec<IpEffect>,
}

impl Rig {
    fn new() -> Self {
        Self {
            m: IpRefresh::new(),
            now: Instant::now(),
            log: Vec::new(),
        }
    }

    fn start(&mut self, target: Option<&str>, tries: u8, range: &str) -> Vec<IpEffect> {
        let target = target.map(|t| IpTarget::parse(t).unwrap());
        let fx = self
            .m
            .start(target, tries, Some(range.to_string()), DS, self.now)
            .expect("start");
        self.log.extend(fx.clone());
        fx
    }

    fn step(&mut self, input: IpInput) -> Vec<IpEffect> {
        let fx = self.m.step(input, self.now);
        self.log.extend(fx.clone());
        fx
    }

    fn advance(&mut self, d: Duration) -> Vec<IpEffect> {
        let mut out = Vec::new();
        let end = self.now + d;
        while self.now < end {
            self.now += Duration::from_secs(1).min(end - self.now);
            out.extend(self.step(IpInput::Tick));
        }
        out
    }

    fn phase(&self) -> IpRefreshPhase {
        self.m.view(self.now, false).phase
    }

    fn posts(&self) -> Vec<bool> {
        self.log
            .iter()
            .filter_map(|e| match e {
                IpEffect::PostSwitch(on) => Some(*on),
                _ => None,
            })
            .collect()
    }

    fn ended(&self) -> Option<IpRefreshOutcome> {
        self.log.iter().find_map(|e| match e {
            IpEffect::Event(IpRefreshEvent::Ended(r)) => Some(r.outcome),
            _ => None,
        })
    }

    /// Answers the journal write and the "off" POST of a try that just started.
    fn turn_off(&mut self) {
        assert_eq!(self.phase(), IpRefreshPhase::TurningOff);
        let fx = self.step(IpInput::JournalWritten(true));
        assert_eq!(fx, vec![IpEffect::PostSwitch(false)]);
        self.step(IpInput::PostResult(Ok(())));
        assert_eq!(self.phase(), IpRefreshPhase::Off);
    }

    /// Waits out the hold and answers "on" and the read of the switch.
    fn turn_on(&mut self) {
        let fx = self.until_effect(Duration::from_secs(60), |e| {
            *e == IpEffect::PostSwitch(true)
        });
        assert!(fx, "no \"on\" within 60 s");
        let fx = self.step(IpInput::PostResult(Ok(())));
        assert_eq!(fx, vec![IpEffect::ReadSwitch]);
        let fx = self.step(IpInput::SwitchRead(Ok(true)));
        assert!(fx.contains(&IpEffect::ArchiveJournal));
    }

    /// Ticks until an effect matching `pred` comes out; false after `within`.
    fn until_effect(&mut self, within: Duration, pred: impl Fn(&IpEffect) -> bool) -> bool {
        let end = self.now + within;
        while self.now < end {
            self.now += Duration::from_secs(1);
            if self.step(IpInput::Tick).iter().any(&pred) {
                return true;
            }
        }
        false
    }

    /// Answers WAN reads until the router reports `range`.
    fn come_back(&mut self, range: &str) {
        assert!(self.until_effect(Duration::from_secs(10), |e| *e == IpEffect::ReadWan));
        self.step(IpInput::WanRead(None));
        assert!(self.until_effect(Duration::from_secs(10), |e| *e == IpEffect::ReadWan));
        self.step(IpInput::WanRead(Some(range.to_string())));
    }

    /// One whole try: off, hold, on, back online at `range`.
    fn cycle(&mut self, range: &str) {
        self.turn_off();
        self.turn_on();
        self.come_back(range);
    }

    /// Ticks through the wait between tries until the next try starts.
    fn next_try(&mut self) {
        assert_eq!(self.phase(), IpRefreshPhase::Between);
        let fx = self.until_effect(Duration::from_secs(20), |e| {
            matches!(e, IpEffect::WriteJournal { .. })
        });
        assert!(fx, "no next try");
    }

    fn holds(&self) -> Vec<u32> {
        self.log
            .iter()
            .filter_map(|e| match e {
                IpEffect::Event(IpRefreshEvent::Try { off_s, .. }) => Some(*off_s),
                _ => None,
            })
            .collect()
    }
}

fn refusal(r: Result<Vec<IpEffect>, RouterError>) -> String {
    match r {
        Err(RouterError::Validation { message }) => message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn happy_path_without_a_target_is_one_try() {
    let mut r = Rig::new();
    let fx = r.start(None, 5, "10.45");
    // The journal comes before anything is sent.
    assert!(matches!(fx.last(), Some(IpEffect::WriteJournal { .. })));
    assert!(r.posts().is_empty());
    r.turn_off();
    // Data stays off for the first hold, 5 s.
    assert!(!r
        .advance(OFF_HOLDS[0] - Duration::from_secs(1))
        .contains(&IpEffect::PostSwitch(true)));
    r.turn_on();
    assert_eq!(r.phase(), IpRefreshPhase::WaitingOnline);
    r.come_back("10.62");
    assert_eq!(r.ended(), Some(IpRefreshOutcome::Changed));
    assert_eq!(r.posts(), vec![false, true]);
    assert!(!r.m.active());
    let last = r.m.last().unwrap();
    assert_eq!(last.from.as_deref(), Some("10.45"));
    assert_eq!(last.to.as_deref(), Some("10.62"));
}

#[test]
fn the_same_range_without_a_target_reads_unchanged() {
    let mut r = Rig::new();
    r.start(None, 1, "10.45");
    r.cycle("10.45");
    assert_eq!(r.ended(), Some(IpRefreshOutcome::Unchanged));
}

#[test]
fn a_failed_journal_write_sends_nothing() {
    let mut r = Rig::new();
    r.start(Some("10.62"), 3, "10.45");
    let fx = r.step(IpInput::JournalWritten(false));
    assert!(!fx.iter().any(|e| matches!(e, IpEffect::PostSwitch(_))));
    assert_eq!(r.ended(), Some(IpRefreshOutcome::JournalFailed));
    assert!(r.posts().is_empty());
}

#[test]
fn on_failing_three_times_keeps_trying_and_makes_no_more_tries() {
    let mut r = Rig::new();
    r.start(Some("100"), 5, "10.45");
    r.turn_off();
    for attempt in 1..=3 {
        assert!(r.until_effect(Duration::from_secs(40), |e| *e
            == IpEffect::PostSwitch(true)));
        let fx = r.step(IpInput::PostResult(Err(RouterError::Timeout)));
        assert!(fx.iter().any(|e| matches!(
            e,
            IpEffect::Event(IpRefreshEvent::DataOnFailed { attempt: a, .. }) if *a == attempt
        )));
        assert_eq!(r.phase(), IpRefreshPhase::TurningOn);
    }
    assert!(r.until_effect(Duration::from_secs(10), |e| *e
        == IpEffect::PostSwitch(true)));
    r.step(IpInput::PostResult(Ok(())));
    r.step(IpInput::SwitchRead(Ok(true)));
    // Data is confirmed on, and an "on" failed: no new "off" after that.
    assert_eq!(r.ended(), Some(IpRefreshOutcome::DataOnFailed));
    r.advance(Duration::from_secs(120));
    assert_eq!(r.posts(), vec![false, true, true, true, true]);
}

#[test]
fn never_off_after_an_on_failure() {
    // Any order of failures: the run never posts "off" once an "on" failed.
    for fail_at_try in 1..=3u8 {
        let mut r = Rig::new();
        r.start(Some("100"), 5, "10.45");
        for t in 1..=fail_at_try {
            r.turn_off();
            if t == fail_at_try {
                assert!(r.until_effect(Duration::from_secs(40), |e| *e
                    == IpEffect::PostSwitch(true)));
                r.step(IpInput::PostResult(Err(RouterError::Timeout)));
                assert!(r.until_effect(Duration::from_secs(10), |e| *e
                    == IpEffect::PostSwitch(true)));
                r.step(IpInput::PostResult(Ok(())));
                r.step(IpInput::SwitchRead(Ok(true)));
            } else {
                r.turn_on();
                r.come_back("10.45");
                r.next_try();
            }
        }
        r.advance(Duration::from_secs(300));
        let posts = r.posts();
        let first_failed_on = posts.iter().rposition(|on| !*on).unwrap();
        assert!(
            posts[first_failed_on + 1..].iter().all(|on| *on),
            "try {fail_at_try}: {posts:?}"
        );
        assert_eq!(r.ended(), Some(IpRefreshOutcome::DataOnFailed));
    }
}

#[test]
fn a_crash_after_off_resumes_by_turning_data_on() {
    let mut r = Rig::new();
    r.m.resume("journal-id".into(), r.now);
    assert!(r.m.active());
    assert!(r.m.data_may_be_off());
    // Not logged in yet: the actor answers Deferred, which is not a failure.
    let fx = r.step(IpInput::Tick);
    assert_eq!(fx, vec![IpEffect::PostSwitch(true)]);
    let fx = r.step(IpInput::Deferred);
    assert!(fx.is_empty());
    assert_eq!(r.m.view(r.now, false).data_on_failures, 0);
    assert!(r.until_effect(Duration::from_secs(2), |e| *e == IpEffect::PostSwitch(true)));
    r.step(IpInput::PostResult(Ok(())));
    let fx = r.step(IpInput::SwitchRead(Ok(true)));
    assert!(fx.contains(&IpEffect::ArchiveJournal));
    assert_eq!(r.ended(), Some(IpRefreshOutcome::Resumed));
    assert_eq!(r.posts(), vec![true, true]);
}

#[test]
fn stop_while_off_turns_data_on_first() {
    let mut r = Rig::new();
    r.start(Some("100"), 5, "10.45");
    r.turn_off();
    let fx = r.step(IpInput::Stop);
    assert_eq!(fx, vec![IpEffect::PostSwitch(true)]);
    assert!(r.m.view(r.now, false).stopping);
    r.step(IpInput::PostResult(Ok(())));
    // Not over until the switch reads 1.
    let fx = r.step(IpInput::SwitchRead(Ok(false)));
    assert_eq!(fx, vec![IpEffect::PostSwitch(true)]);
    r.step(IpInput::PostResult(Ok(())));
    r.step(IpInput::SwitchRead(Ok(true)));
    assert_eq!(r.ended(), Some(IpRefreshOutcome::Stopped));
}

#[test]
fn stop_while_data_is_on_ends_at_once() {
    let mut r = Rig::new();
    r.start(Some("100"), 5, "10.45");
    r.cycle("10.45");
    assert_eq!(r.phase(), IpRefreshPhase::Between);
    let fx = r.step(IpInput::Stop);
    assert!(!fx.iter().any(|e| matches!(e, IpEffect::PostSwitch(_))));
    assert_eq!(r.ended(), Some(IpRefreshOutcome::Stopped));
}

#[test]
fn a_match_on_try_3_ends_the_run() {
    let mut r = Rig::new();
    r.start(Some("100.70"), 5, "10.45");
    r.cycle("10.45");
    r.next_try();
    r.cycle("10.62");
    r.next_try();
    r.cycle("100.70");
    assert_eq!(r.ended(), Some(IpRefreshOutcome::Matched));
    assert_eq!(r.m.last().unwrap().tries, 3);
    assert_eq!(r.posts(), vec![false, true, false, true, false, true]);
}

#[test]
fn one_number_targets_match_the_first_number() {
    let mut r = Rig::new();
    r.start(Some("100"), 2, "10.45");
    r.cycle("100.70");
    assert_eq!(r.ended(), Some(IpRefreshOutcome::Matched));
}

#[test]
fn tries_are_capped_at_5_and_6_is_refused() {
    let mut m = IpRefresh::new();
    let now = Instant::now();
    let t = || Some(IpTarget::parse("100").unwrap());
    for bad in [0u8, 6, 255] {
        assert_eq!(
            refusal(m.start(t(), bad, Some("10.45".into()), DS, now)),
            IP_TRIES_INVALID
        );
    }
    let mut r = Rig::new();
    r.start(Some("100"), 5, "10.45");
    for i in 0..5 {
        r.cycle("10.45");
        if i < 4 {
            r.next_try();
        }
    }
    assert_eq!(r.ended(), Some(IpRefreshOutcome::NotMatched));
    assert_eq!(r.posts().iter().filter(|on| !**on).count(), 5);
}

#[test]
fn the_same_range_makes_the_hold_grow() {
    let mut r = Rig::new();
    r.start(Some("100"), 5, "10.45");
    for i in 0..5 {
        r.cycle("10.45");
        if i < 4 {
            r.next_try();
        }
    }
    assert_eq!(r.holds(), vec![5, 10, 20, 30, 30]);
}

#[test]
fn a_session_loss_while_off_turns_data_on() {
    let mut r = Rig::new();
    r.start(Some("100"), 5, "10.45");
    r.turn_off();
    let fx = r.step(IpInput::SessionUnavailable);
    assert_eq!(fx, vec![IpEffect::PostSwitch(true)]);
    // The actor can't send it until the relogin; then it goes out.
    r.step(IpInput::Deferred);
    assert!(r.until_effect(Duration::from_secs(2), |e| *e == IpEffect::PostSwitch(true)));
    r.step(IpInput::PostResult(Ok(())));
    r.step(IpInput::SwitchRead(Ok(true)));
    assert_eq!(r.ended(), Some(IpRefreshOutcome::SessionLost));
}

#[test]
fn online_timeout_after_90_s() {
    let mut r = Rig::new();
    r.start(None, 1, "10.45");
    r.turn_off();
    r.turn_on();
    let mut reads = 0;
    let end = r.now + ONLINE_TIMEOUT + Duration::from_secs(2);
    while r.now < end && r.ended().is_none() {
        r.now += Duration::from_secs(1);
        for e in r.step(IpInput::Tick) {
            if e == IpEffect::ReadWan {
                reads += 1;
                r.step(IpInput::WanRead(None));
            }
        }
    }
    assert_eq!(r.ended(), Some(IpRefreshOutcome::OnlineTimeout));
    assert!((40..=46).contains(&reads), "a read every 2 s: {reads}");
}

#[test]
fn the_hard_cap_turns_data_on_and_ends() {
    let mut r = Rig::new();
    r.start(Some("100"), 5, "10.45");
    r.turn_off();
    // The router never answers the first "on" read of the switch.
    assert!(r.until_effect(Duration::from_secs(10), |e| *e
        == IpEffect::PostSwitch(true)));
    assert_eq!(
        r.step(IpInput::PostResult(Ok(()))),
        vec![IpEffect::ReadSwitch]
    );
    r.step(IpInput::SwitchRead(Err(RouterError::Timeout)));
    // Reads keep failing until past the cap.
    let end = r.now + HARD_CAP;
    while r.now < end {
        r.now += Duration::from_secs(1);
        for e in r.step(IpInput::Tick) {
            if e == IpEffect::ReadSwitch {
                r.step(IpInput::SwitchRead(Err(RouterError::Timeout)));
            }
        }
    }
    assert!(r.until_effect(Duration::from_secs(10), |e| *e == IpEffect::ReadSwitch));
    r.step(IpInput::SwitchRead(Ok(true)));
    // Data is on: the run ends without another try.
    r.advance(Duration::from_secs(20));
    assert_eq!(r.ended(), Some(IpRefreshOutcome::HardCap));
    assert_eq!(r.posts(), vec![false, true], "no new try past the cap");
}

#[test]
fn the_hourly_budget_is_15_cycles() {
    let mut r = Rig::new();
    for _ in 0..3 {
        r.start(Some("100"), 5, "10.45");
        for i in 0..5 {
            r.cycle("10.45");
            if i < 4 {
                r.next_try();
            }
        }
        r.log.clear();
        r.now += Duration::from_secs(1);
    }
    assert_eq!(r.m.view(r.now, false).cycles_left, 0);
    let now = r.now;
    assert_eq!(
        refusal(r.m.start(None, 1, Some("10.45".into()), DS, now)),
        IP_REFRESH_COOLDOWN
    );
    // An hour after the first cycles, some budget is back.
    r.now += Duration::from_secs(60 * 60);
    assert_eq!(r.m.view(r.now, false).cycles_left as usize, CYCLES_PER_HOUR);
    r.start(None, 1, "10.45");
}

#[test]
fn a_run_that_spends_the_budget_stops() {
    let mut r = Rig::new();
    // 14 single tries leave one cycle.
    for _ in 0..14 {
        r.start(None, 1, "10.45");
        r.cycle("10.45");
        r.now += Duration::from_secs(1);
    }
    r.log.clear();
    r.start(Some("100"), 3, "10.45");
    r.cycle("10.45");
    r.advance(Duration::from_secs(20));
    assert_eq!(r.ended(), Some(IpRefreshOutcome::RateLimited));
    assert_eq!(r.posts(), vec![false, true]);
}

#[test]
fn an_off_refusal_still_checks_the_switch() {
    let mut r = Rig::new();
    r.start(None, 1, "10.45");
    r.step(IpInput::JournalWritten(true));
    let fx = r.step(IpInput::PostResult(Err(RouterError::Timeout)));
    assert_eq!(fx, vec![IpEffect::ReadSwitch]);
    // It went off after all: turned back on.
    let fx = r.step(IpInput::SwitchRead(Ok(false)));
    assert_eq!(fx, vec![IpEffect::PostSwitch(true)]);
    r.step(IpInput::PostResult(Ok(())));
    r.step(IpInput::SwitchRead(Ok(true)));
    assert_eq!(r.ended(), Some(IpRefreshOutcome::OffRefused));
}

#[test]
fn off_that_cannot_be_sent_ends_with_data_on() {
    let mut r = Rig::new();
    r.start(None, 1, "10.45");
    r.step(IpInput::JournalWritten(true));
    let fx = r.step(IpInput::Deferred);
    assert!(fx.contains(&IpEffect::ArchiveJournal));
    assert_eq!(r.ended(), Some(IpRefreshOutcome::SessionLost));
    assert_eq!(r.posts(), vec![false], "only the deferred one, never sent");
}

#[test]
fn starts_are_refused_when_already_matched_or_running() {
    let mut m = IpRefresh::new();
    let now = Instant::now();
    let t = Some(IpTarget::parse("10.45").unwrap());
    assert_eq!(
        refusal(m.start(t, 3, Some("10.45".into()), DS, now)),
        IP_ALREADY_MATCHES
    );
    m.start(None, 1, Some("10.45".into()), DS, now).unwrap();
    assert_eq!(
        refusal(m.start(None, 1, Some("10.45".into()), DS, now)),
        router_core::IP_REFRESH_RUNNING
    );
}

#[test]
fn targets_are_one_or_two_numbers() {
    for ok in ["10", "10.45", " 100.70 ", "10.", "0", "255.255"] {
        assert!(IpTarget::parse(ok).is_ok(), "{ok}");
    }
    for bad in [
        "",
        "10.45.12",
        "256",
        "10.256",
        "a",
        "10..",
        ".10",
        "-1",
        "1e2",
        "10.45.12.1",
    ] {
        match IpTarget::parse(bad) {
            Err(RouterError::Validation { message }) => assert_eq!(message, IP_TARGET_INVALID),
            other => panic!("{bad:?}: {other:?}"),
        }
    }
    let t = IpTarget::parse("10.4").unwrap();
    assert!(t.matches("10.4"));
    assert!(!t.matches("10.45"), "whole numbers only");
    assert_eq!(t.to_string(), "10.4");
}

// ---------------------------------------------------------------------------------------------
// Network search (the H122's method) and the fallback from a refused "off"

const SEARCH: &[IpMethod] = &[IpMethod::NetworkSearch];
const BOTH: &[IpMethod] = &[IpMethod::DataSwitch, IpMethod::NetworkSearch];

fn found(networks: u8) -> Result<NetworkSearch, RouterError> {
    Ok(NetworkSearch {
        networks: Some(networks),
    })
}

impl Rig {
    fn start_with(
        &mut self,
        target: Option<&str>,
        tries: u8,
        range: &str,
        methods: &[IpMethod],
    ) -> Vec<IpEffect> {
        let target = target.map(|t| IpTarget::parse(t).unwrap());
        let fx = self
            .m
            .start(target, tries, Some(range.to_string()), methods, self.now)
            .expect("start");
        self.log.extend(fx.clone());
        fx
    }

    /// Lets the search run for `took`, ticking, then answers it.
    fn search_answers(
        &mut self,
        took: Duration,
        result: Result<NetworkSearch, RouterError>,
    ) -> Vec<IpEffect> {
        assert_eq!(self.phase(), IpRefreshPhase::Searching);
        let fx = self.advance(took);
        assert!(fx.is_empty(), "nothing while searching: {fx:?}");
        self.step(IpInput::SearchDone {
            result,
            service_dropped: true,
        })
    }

    fn searches(&self) -> usize {
        self.log
            .iter()
            .filter(|e| **e == IpEffect::StartSearch)
            .count()
    }

    fn search_events(&self) -> Vec<IpRefreshEvent> {
        self.log
            .iter()
            .filter_map(|e| match e {
                IpEffect::Event(ev @ IpRefreshEvent::Search { .. }) => Some(ev.clone()),
                _ => None,
            })
            .collect()
    }
}

#[test]
fn a_search_needs_no_journal_and_waits_for_the_router() {
    let mut r = Rig::new();
    let fx = r.start_with(None, 1, "10.45", SEARCH);
    assert_eq!(fx.last(), Some(&IpEffect::StartSearch));
    assert!(!fx
        .iter()
        .any(|e| matches!(e, IpEffect::WriteJournal { .. })));
    assert_eq!(r.m.view(r.now, false).method, Some(IpMethod::NetworkSearch));
    // Searching: no timer of its own, and a lost session changes nothing.
    assert_eq!(r.m.view(r.now, false).seconds_left, None);
    assert!(r.step(IpInput::SessionUnavailable).is_empty());
    assert!(!r.m.data_may_be_off());
    r.search_answers(Duration::from_secs(40), found(3));
    assert_eq!(
        r.search_events(),
        vec![IpRefreshEvent::Search {
            n: 1,
            seconds: 40,
            networks: Some(3),
            timed_out: false,
            service_dropped: true,
            error: None,
        }]
    );
    assert_eq!(r.phase(), IpRefreshPhase::WaitingOnline);
    // The first read waits out the settle time.
    let early = r.advance(SEARCH_SETTLE - Duration::from_secs(1));
    assert!(!early.contains(&IpEffect::ReadWan), "{early:?}");
    r.come_back("10.62");
    assert_eq!(r.ended(), Some(IpRefreshOutcome::Changed));
    let last = r.m.last().unwrap();
    assert_eq!(last.method, IpMethod::NetworkSearch);
    assert_eq!(last.refusal, None);
    assert!(r.posts().is_empty(), "the data switch is never touched");
    let tried = r.log.iter().any(|e| {
        matches!(
            e,
            IpEffect::Event(IpRefreshEvent::Try {
                off_s: 0,
                method: IpMethod::NetworkSearch,
                ..
            })
        )
    });
    assert!(tried);
}

#[test]
fn searches_repeat_until_the_target_matches() {
    let mut r = Rig::new();
    r.start_with(Some("100.70"), 3, "10.45", SEARCH);
    r.search_answers(Duration::from_secs(30), found(2));
    r.come_back("10.45");
    assert_eq!(r.phase(), IpRefreshPhase::Between);
    // At least 20 s between searches.
    let soon = r.advance(SEARCH_BETWEEN - Duration::from_secs(1));
    assert!(!soon.contains(&IpEffect::StartSearch));
    assert!(r.until_effect(Duration::from_secs(2), |e| *e == IpEffect::StartSearch));
    assert_eq!(r.m.view(r.now, false).try_n, 2);
    r.search_answers(Duration::from_secs(30), found(2));
    r.come_back("100.70");
    assert_eq!(r.ended(), Some(IpRefreshOutcome::Matched));
    assert_eq!(r.m.last().unwrap().tries, 2);
    assert_eq!(r.searches(), 2);
}

#[test]
fn a_quick_refusal_ends_the_run_with_the_code() {
    let mut r = Rig::new();
    r.start_with(Some("100"), 5, "10.45", SEARCH);
    let refusal = RouterError::SessionLost { code: Some(100003) };
    r.search_answers(Duration::from_secs(1), Err(refusal.clone()));
    assert_eq!(r.ended(), Some(IpRefreshOutcome::SearchRefused));
    assert_eq!(r.m.last().unwrap().refusal, Some(refusal.clone()));
    assert!(matches!(
        &r.search_events()[0],
        IpRefreshEvent::Search { error: Some(e), timed_out: false, .. } if *e == refusal
    ));
    assert_eq!(r.searches(), 1, "no second try after a refusal");
}

/// A lapsed session or token, or a busy router, isn't the search being refused: the run goes on
/// to the online wait (the actor logs in again), and the feature isn't turned off.
#[test]
fn a_quick_lapsed_session_or_busy_answer_is_not_a_refusal() {
    for e in [
        RouterError::SessionLost { code: Some(125002) },
        RouterError::SessionLost { code: Some(125003) },
        RouterError::SessionLost { code: None },
        RouterError::Api {
            code: 100004,
            kind: router_core::ApiErrorKind::Busy,
        },
    ] {
        let mut r = Rig::new();
        r.start_with(None, 1, "10.45", SEARCH);
        r.search_answers(Duration::from_secs(1), Err(e.clone()));
        assert_eq!(r.ended(), None, "{e:?}");
        assert_eq!(r.phase(), IpRefreshPhase::WaitingOnline, "{e:?}");
    }
}

/// The quick-refusal window starts when the search is sent, not when the try began: a slow
/// session refresh before it doesn't turn a refusal into a search that ran.
#[test]
fn a_quick_refusal_counts_from_when_the_search_was_sent() {
    let mut r = Rig::new();
    r.start_with(None, 3, "10.45", SEARCH);
    r.advance(Duration::from_secs(8));
    r.step(IpInput::SearchSent);
    let refusal = RouterError::SessionLost { code: Some(100003) };
    r.search_answers(Duration::from_secs(1), Err(refusal));
    assert_eq!(r.ended(), Some(IpRefreshOutcome::SearchRefused));
}

#[test]
fn a_timeout_or_a_late_error_goes_to_the_online_wait() {
    let mut r = Rig::new();
    r.start_with(None, 1, "10.45", SEARCH);
    r.search_answers(Duration::from_secs(180), Err(RouterError::Timeout));
    assert!(matches!(
        &r.search_events()[0],
        IpRefreshEvent::Search {
            timed_out: true,
            seconds: 180,
            ..
        }
    ));
    assert_eq!(r.phase(), IpRefreshPhase::WaitingOnline);
    r.come_back("10.45");
    assert_eq!(r.ended(), Some(IpRefreshOutcome::Unchanged));

    // A refusal after the router searched for a while isn't a refusal to search.
    let mut r = Rig::new();
    r.start_with(None, 1, "10.45", SEARCH);
    let late = RouterError::Api {
        code: 100003,
        kind: router_core::ApiErrorKind::NoRights,
    };
    r.search_answers(QUICK_REFUSAL + Duration::from_secs(25), Err(late));
    assert_eq!(r.phase(), IpRefreshPhase::WaitingOnline);
}

#[test]
fn the_router_gets_150_s_after_a_search() {
    let mut r = Rig::new();
    r.start_with(None, 1, "10.45", SEARCH);
    r.search_answers(Duration::from_secs(60), found(3));
    let waiting_from = r.now;
    let end = r.now + SEARCH_ONLINE_TIMEOUT + Duration::from_secs(2);
    let mut ended_at = None;
    while r.now < end && ended_at.is_none() {
        r.now += Duration::from_secs(1);
        for e in r.step(IpInput::Tick) {
            if e == IpEffect::ReadWan {
                r.step(IpInput::WanRead(None));
            }
        }
        if r.ended().is_some() {
            ended_at = Some(r.now - waiting_from);
        }
    }
    assert_eq!(r.ended(), Some(IpRefreshOutcome::OnlineTimeout));
    let after = ended_at.unwrap();
    assert!(
        after >= SEARCH_ONLINE_TIMEOUT && after > ONLINE_TIMEOUT,
        "{after:?}"
    );
}

#[test]
fn stop_during_a_search_waits_for_it() {
    let mut r = Rig::new();
    r.start_with(Some("100"), 5, "10.45", SEARCH);
    r.advance(Duration::from_secs(10));
    let fx = r.step(IpInput::Stop);
    assert!(fx.is_empty(), "nothing to call back: {fx:?}");
    let view = r.m.view(r.now, false);
    assert!(view.active && view.stopping);
    assert_eq!(view.phase, IpRefreshPhase::Searching);
    r.search_answers(Duration::from_secs(20), found(3));
    assert_eq!(r.phase(), IpRefreshPhase::WaitingOnline);
    assert!(r.ended().is_none());
    r.come_back("10.45");
    assert_eq!(r.ended(), Some(IpRefreshOutcome::Stopped));
    assert_eq!(r.searches(), 1);
}

#[test]
fn searches_count_against_the_hourly_budget() {
    let mut r = Rig::new();
    for _ in 0..CYCLES_PER_HOUR {
        r.start_with(None, 1, "10.45", SEARCH);
        r.search_answers(Duration::from_secs(1), Err(RouterError::Timeout));
        r.come_back("10.45");
        assert!(!r.m.active());
    }
    assert_eq!(r.m.view(r.now, false).cycles_left, 0);
    let now = r.now;
    assert_eq!(
        refusal(r.m.start(None, 1, Some("10.45".into()), SEARCH, now)),
        IP_REFRESH_COOLDOWN
    );
}

#[test]
fn a_search_that_cannot_be_sent_gives_its_cycle_back() {
    let mut r = Rig::new();
    let before = r.m.view(r.now, false).cycles_left;
    r.start_with(None, 1, "10.45", SEARCH);
    assert_eq!(r.m.view(r.now, false).cycles_left, before - 1);
    r.step(IpInput::Deferred);
    assert_eq!(r.ended(), Some(IpRefreshOutcome::SessionLost));
    assert_eq!(r.m.view(r.now, false).cycles_left, before);
}

#[test]
fn no_method_is_refused() {
    let mut m = IpRefresh::new();
    assert_eq!(
        refusal(m.start(None, 1, Some("10.45".into()), &[], Instant::now())),
        router_core::IP_NO_METHOD
    );
}

#[test]
fn a_refused_off_falls_back_to_a_search_when_the_switch_reads_1() {
    let mut r = Rig::new();
    r.start_with(None, 1, "10.45", BOTH);
    assert_eq!(r.m.view(r.now, false).method, Some(IpMethod::DataSwitch));
    let refusal = RouterError::SessionLost { code: Some(125003) };
    r.step(IpInput::JournalWritten(true));
    let fx = r.step(IpInput::PostResult(Err(refusal.clone())));
    assert_eq!(
        fx,
        vec![IpEffect::ReadSwitch],
        "the switch is still checked"
    );
    let fx = r.step(IpInput::SwitchRead(Ok(true)));
    assert_eq!(
        fx,
        vec![
            IpEffect::ArchiveJournal,
            IpEffect::Event(IpRefreshEvent::MethodFallback {
                from: IpMethod::DataSwitch,
                to: IpMethod::NetworkSearch,
                error: refusal,
            }),
            IpEffect::StartSearch,
        ]
    );
    let view = r.m.view(r.now, false);
    assert_eq!(
        (view.method, view.try_n),
        (Some(IpMethod::NetworkSearch), 1)
    );
    r.search_answers(Duration::from_secs(30), found(3));
    r.come_back("10.62");
    assert_eq!(r.ended(), Some(IpRefreshOutcome::Changed));
    let last = r.m.last().unwrap();
    assert_eq!(last.method, IpMethod::NetworkSearch);
    assert_eq!(last.refusal, None);
    assert_eq!(r.posts(), vec![false], "no \"on\" for data that stayed on");
}

#[test]
fn a_refused_off_that_went_off_anyway_turns_data_on_and_ends() {
    let mut r = Rig::new();
    r.start_with(None, 1, "10.45", BOTH);
    let refusal = RouterError::Api {
        code: 100003,
        kind: router_core::ApiErrorKind::NoRights,
    };
    r.step(IpInput::JournalWritten(true));
    r.step(IpInput::PostResult(Err(refusal.clone())));
    let fx = r.step(IpInput::SwitchRead(Ok(false)));
    assert_eq!(fx, vec![IpEffect::PostSwitch(true)]);
    r.step(IpInput::PostResult(Ok(())));
    r.step(IpInput::SwitchRead(Ok(true)));
    assert_eq!(r.ended(), Some(IpRefreshOutcome::OffRefused));
    assert_eq!(r.m.last().unwrap().refusal, Some(refusal));
    assert_eq!(r.searches(), 0);
}

#[test]
fn a_timeout_on_off_is_no_refusal_and_does_not_search() {
    let mut r = Rig::new();
    r.start_with(None, 1, "10.45", BOTH);
    r.step(IpInput::JournalWritten(true));
    r.step(IpInput::PostResult(Err(RouterError::Timeout)));
    let fx = r.step(IpInput::SwitchRead(Ok(true)));
    assert!(fx.contains(&IpEffect::ArchiveJournal));
    assert_eq!(r.ended(), Some(IpRefreshOutcome::OffRefused));
    assert_eq!(r.m.last().unwrap().refusal, Some(RouterError::Timeout));
    assert_eq!(r.searches(), 0);
}
