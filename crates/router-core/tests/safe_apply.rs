use std::time::{Duration, Instant};

use router_core::drivers::huawei::xml::parse_signal;
use router_core::drivers::mock::MockScenario;
use router_core::journal::{JournalPhase, JournalRecord};
use router_core::safe_apply::{
    Effect, ForeignJournal, Input, Phase, RevertReason, SafeApply, SafeApplyEvent, SafeApplyTimers,
};
use router_core::{
    in_service, ApiErrorKind, Capabilities, ConnState, LteBandSet, NetConfig, NetworkMode,
    NrOption, RouterError, Status,
};

const HOST: &str = "192.168.8.1";
const DRIVER: &str = "huawei";

fn timers() -> SafeApplyTimers {
    SafeApplyTimers {
        grace: Duration::from_secs(90),
        loss: Duration::from_secs(60),
        probation: Duration::from_secs(180),
        poll_interval: Duration::from_secs(1),
        // Most tests re-attach on the first good observation; settling has its own tests.
        settle: Duration::ZERO,
    }
}

fn settling_timers() -> SafeApplyTimers {
    SafeApplyTimers {
        settle: Duration::from_secs(10),
        ..timers()
    }
}

fn caps() -> Capabilities {
    Capabilities {
        supported_lte: MockScenario::zain_like().supported,
        modes: vec![NetworkMode::Auto, NetworkMode::LteOnly, NetworkMode::NrOnly],
        nr_options: vec![NrOption::Nsa, NrOption::Sa, NrOption::SaNsa],
        band_edit_allowed: true,
        factory_reset_verified: false,
        data_switch: false,
        ip_methods: Vec::new(),
        apn: Default::default(),
    }
}

/// A non-Auto snapshot so the fallback config is distinguishable from it.
fn previous() -> NetConfig {
    NetConfig {
        network_mode: NetworkMode::LteOnly,
        lte_bands: LteBandSet::from_bands(&[1, 3, 8]).unwrap(),
        legacy_band_raw: "3FFFFFFF".to_string(),
        nr_option: Some(NrOption::Sa),
    }
}

fn requested() -> NetConfig {
    NetConfig {
        network_mode: NetworkMode::LteOnly,
        lte_bands: LteBandSet::from_bands(&[3]).unwrap(),
        legacy_band_raw: "3FFFFFFF".to_string(),
        nr_option: Some(NrOption::Sa),
    }
}

fn fallback() -> NetConfig {
    NetConfig {
        network_mode: NetworkMode::Auto,
        lte_bands: LteBandSet::AUTO,
        legacy_band_raw: "3FFFFFFF".to_string(),
        nr_option: Some(NrOption::Sa),
    }
}

fn api_err() -> RouterError {
    RouterError::Api {
        code: 112003,
        kind: ApiErrorKind::BadParam,
    }
}

fn start_input(requested: NetConfig) -> Input {
    Input::Start {
        requested,
        caps: caps(),
        current: Some(previous()),
        revert_to: None,
    }
}

fn obs(in_service: bool) -> Input {
    Input::Observation {
        in_service,
        polled_at: None,
        lte: None,
        wan_ip_present: None,
    }
}

fn has_post(effects: &[Effect]) -> bool {
    effects.iter().any(|e| matches!(e, Effect::Post(_)))
}

struct H {
    sa: SafeApply,
    t0: Instant,
}

impl H {
    fn new() -> Self {
        Self {
            sa: SafeApply::new(timers(), HOST.to_string(), DRIVER.to_string()),
            t0: Instant::now(),
        }
    }

    fn resumed(record: JournalRecord) -> (Self, Vec<Effect>) {
        let t0 = Instant::now();
        let (sa, effects) = SafeApply::resume(record, timers(), HOST, DRIVER, t0).unwrap();
        (Self { sa, t0 }, effects)
    }

    fn with_timers(t: SafeApplyTimers) -> Self {
        Self {
            sa: SafeApply::new(t, HOST.to_string(), DRIVER.to_string()),
            t0: Instant::now(),
        }
    }

    /// An observation at `s` from a poll that started at `polled`, serving LTE `band`/`pci`.
    fn cell(&mut self, in_service: bool, polled: u64, band: u16, pci: u16, s: u64) -> Vec<Effect> {
        let input = Input::Observation {
            in_service,
            polled_at: Some(self.t0 + Duration::from_secs(polled)),
            lte: Some((band, Some(pci))),
            wan_ip_present: None,
        };
        self.step(input, s)
    }

    fn step(&mut self, input: Input, s: u64) -> Vec<Effect> {
        self.sa.step(input, self.t0 + Duration::from_secs(s))
    }

    fn phase(&self) -> Phase {
        self.sa.phase().clone()
    }

    /// Start → snapshot → POST ok → verified, all at t=0. Grace starts at t=0.
    fn enter_grace(&mut self) {
        self.step(start_input(requested()), 0);
        self.step(Input::ConfigRead(Ok(previous())), 0);
        self.step(Input::PostResult(Ok(())), 0);
        self.step(Input::ConfigRead(Ok(requested())), 0);
        assert_eq!(self.phase(), Phase::ReattachGrace);
    }

    /// Re-attached at t=0: probation ends at t=180.
    fn enter_probation(&mut self) {
        self.enter_grace();
        self.step(obs(true), 0);
        assert_eq!(self.phase(), Phase::Probation);
    }

    /// A revert POST succeeds, verifies and re-attaches at `s`; returns the last effects.
    fn finish_revert(&mut self, s: u64) -> Vec<Effect> {
        assert_eq!(
            self.step(Input::PostResult(Ok(())), s),
            vec![Effect::ReadConfig]
        );
        assert_eq!(self.step(Input::ConfigRead(Ok(previous())), s), vec![]);
        self.step(obs(true), s)
    }

    fn tick_range(&mut self, from: u64, to: u64) -> Vec<Effect> {
        let mut all = Vec::new();
        for s in from..=to {
            all.extend(self.step(Input::Tick, s));
        }
        all
    }

    /// One input every `every` seconds over `from..=to`.
    fn every(&mut self, input: Input, from: u64, to: u64, every: u64) -> Vec<Effect> {
        let mut all = Vec::new();
        let mut s = from;
        while s <= to {
            all.extend(self.step(input.clone(), s));
            s += every;
        }
        all
    }

    fn observe_range(&mut self, in_service: bool, from: u64, to: u64) -> Vec<Effect> {
        let mut all = Vec::new();
        for s in from..=to {
            all.extend(self.step(obs(in_service), s));
        }
        all
    }
}

fn reverted_effects(reason: RevertReason) -> Vec<Effect> {
    vec![
        Effect::WriteJournal(JournalPhase::Reverted),
        Effect::ArchiveJournal,
        Effect::Event(SafeApplyEvent::Reverted {
            reason,
            fallback: false,
        }),
    ]
}

/// The revert finished on the Auto fallback, not on the previous bands.
fn reverted_via_auto_effects(reason: RevertReason) -> Vec<Effect> {
    vec![
        Effect::WriteJournal(JournalPhase::Reverted),
        Effect::ArchiveJournal,
        Effect::Event(SafeApplyEvent::Reverted {
            reason,
            fallback: true,
        }),
    ]
}

fn not_applied_effects() -> Vec<Effect> {
    vec![
        Effect::ArchiveJournal,
        Effect::Event(SafeApplyEvent::Reverted {
            reason: RevertReason::ApplyRejected,
            fallback: false,
        }),
    ]
}

fn revert_start() -> Vec<Effect> {
    vec![
        Effect::WriteJournal(JournalPhase::Reverting),
        Effect::Post(previous()),
    ]
}

fn commit_effects() -> Vec<Effect> {
    vec![
        Effect::WriteJournal(JournalPhase::Committed),
        Effect::ArchiveJournal,
        Effect::Event(SafeApplyEvent::Committed),
    ]
}

fn journal_record(phase: JournalPhase) -> JournalRecord {
    JournalRecord {
        version: 1,
        id: "6f1c3a52-6a43-4f39-9d55-1f0b8c1d2e3f".to_string(),
        driver: DRIVER.to_string(),
        host: HOST.to_string(),
        previous: previous(),
        requested: requested(),
        phase,
        phase_changed_at_ms: 1,
        created_at_ms: 1,
    }
}

#[test]
fn happy_path_effect_sequence() {
    let mut h = H::new();
    assert_eq!(
        h.step(start_input(requested()), 0),
        vec![Effect::ReadConfig]
    );
    assert_eq!(h.phase(), Phase::Snapshotting);
    assert!(
        h.sa.record().is_none(),
        "nothing journaled before the snapshot"
    );

    assert_eq!(
        h.step(Input::ConfigRead(Ok(previous())), 0),
        vec![
            Effect::WriteJournal(JournalPhase::Journaled),
            Effect::Post(requested())
        ]
    );
    assert_eq!(h.phase(), Phase::Applying);
    let r = h.sa.record().unwrap();
    assert_eq!(r.version, 1);
    assert_eq!(r.phase, JournalPhase::Journaled);
    assert_eq!(r.previous, previous());
    assert_eq!(r.requested, requested());
    assert_eq!((r.host.as_str(), r.driver.as_str()), (HOST, DRIVER));
    assert_eq!((r.created_at_ms, r.phase_changed_at_ms), (0, 0));
    let id = uuid::Uuid::parse_str(&r.id).unwrap();
    assert_eq!(id.get_version_num(), 4);

    assert_eq!(
        h.step(Input::PostResult(Ok(())), 1),
        vec![
            Effect::WriteJournal(JournalPhase::Applied),
            Effect::ReadConfig
        ]
    );
    assert_eq!(h.phase(), Phase::Verifying);

    assert_eq!(
        h.step(Input::ConfigRead(Ok(requested())), 2),
        vec![
            Effect::WriteJournal(JournalPhase::Verified),
            Effect::WriteJournal(JournalPhase::ReattachGrace)
        ]
    );
    assert_eq!(h.phase(), Phase::ReattachGrace);
    // R3: the grace clock started at PostResult(Ok) at t=1, not at the verify.
    assert_eq!(
        h.sa.view(h.t0 + Duration::from_secs(32)).seconds_left,
        Some(59)
    );

    assert_eq!(h.observe_range(false, 3, 9), vec![]);
    assert_eq!(
        h.step(obs(true), 10),
        vec![
            Effect::WriteJournal(JournalPhase::Probation),
            Effect::Event(SafeApplyEvent::Reattached)
        ]
    );
    assert_eq!(h.phase(), Phase::Probation);
    assert_eq!(
        h.sa.view(h.t0 + Duration::from_secs(100)).seconds_left,
        Some(90)
    );

    assert_eq!(h.observe_range(true, 11, 189), vec![]);
    assert_eq!(h.step(obs(true), 190), commit_effects());
    assert_eq!(h.phase(), Phase::Committed);
    assert_eq!(h.sa.record().unwrap().phase, JournalPhase::Committed);
    assert_eq!(h.step(Input::Tick, 500), vec![]);
}

#[test]
fn commit_is_also_reached_by_tick() {
    let mut h = H::new();
    h.enter_probation();
    h.observe_range(true, 1, 179);
    assert_eq!(h.step(Input::Tick, 179), vec![]);
    assert_eq!(h.step(Input::Tick, 180), commit_effects());
}

fn assert_rejected(effects: &[Effect]) {
    assert!(
        matches!(effects, [Effect::Event(SafeApplyEvent::Rejected { .. })]),
        "{effects:?}"
    );
}

#[test]
fn rejects_invalid() {
    let mut h = H::new();

    let mut empty = requested();
    empty.lte_bands = LteBandSet::from_bits(0);
    assert_rejected(&h.step(start_input(empty), 0));
    assert_eq!(h.phase(), Phase::Idle);

    let mut b13 = requested();
    b13.lte_bands = LteBandSet::from_bands(&[3, 13]).unwrap();
    assert_rejected(&h.step(start_input(b13), 0));
    assert_eq!(h.phase(), Phase::Idle);

    assert_rejected(&h.step(start_input(previous()), 0));
    assert_eq!(h.phase(), Phase::Idle);

    let mut raw_mode = requested();
    raw_mode.network_mode = NetworkMode::Raw {
        value: "0201".to_string(),
    };
    assert_rejected(&h.step(start_input(raw_mode), 0));

    let mut caps_no_sa = caps();
    caps_no_sa.nr_options = vec![NrOption::Nsa];
    assert_rejected(&h.step(
        Input::Start {
            requested: requested(),
            caps: caps_no_sa,
            current: Some(previous()),
            revert_to: None,
        },
        0,
    ));

    let mut locked = caps();
    locked.band_edit_allowed = false;
    assert_rejected(&h.step(
        Input::Start {
            requested: requested(),
            caps: locked,
            current: Some(previous()),
            revert_to: None,
        },
        0,
    ));
    assert_eq!(h.phase(), Phase::Idle);
    assert!(h.sa.record().is_none());

    h.enter_probation();
    assert_rejected(&h.step(start_input(fallback()), 5));
    assert_eq!(h.phase(), Phase::Probation);
}

#[test]
fn auto_mask_is_accepted_even_though_not_a_subset() {
    let mut h = H::new();
    assert_eq!(h.step(start_input(fallback()), 0), vec![Effect::ReadConfig]);
    assert_eq!(h.phase(), Phase::Snapshotting);
}

#[test]
fn snapshot_equal_to_requested_is_rejected_without_journal() {
    let mut h = H::new();
    h.step(
        Input::Start {
            requested: requested(),
            caps: caps(),
            current: None,
            revert_to: None,
        },
        0,
    );
    assert_rejected(&h.step(Input::ConfigRead(Ok(requested())), 0));
    assert_eq!(h.phase(), Phase::Idle);
    assert!(h.sa.record().is_none());
}

#[test]
fn snapshot_read_failure_is_rejected_without_journal() {
    let mut h = H::new();
    h.step(start_input(requested()), 0);
    assert_rejected(&h.step(Input::ConfigRead(Err(RouterError::Timeout)), 0));
    assert_eq!(h.phase(), Phase::Idle);
    assert!(h.sa.record().is_none());
}

#[test]
fn apply_error_closes_without_revert_post() {
    let mut h = H::new();
    h.step(start_input(requested()), 0);
    h.step(Input::ConfigRead(Ok(previous())), 0);
    let effects = h.step(Input::PostResult(Err(api_err())), 1);
    assert_eq!(effects, not_applied_effects());
    assert!(!has_post(&effects));
    assert_eq!(h.phase(), Phase::Idle);
    // The record is still available for the archive.
    assert_eq!(h.sa.record().unwrap().phase, JournalPhase::Journaled);
    assert_eq!(h.step(Input::Tick, 500), vec![]);
}

#[test]
fn ambiguous_apply_error_reads_back_before_closing() {
    // A timed-out POST may still have been applied: read back instead of assuming.
    let mut h = H::new();
    h.step(start_input(requested()), 0);
    h.step(Input::ConfigRead(Ok(previous())), 0);
    assert_eq!(
        h.step(Input::PostResult(Err(RouterError::Timeout)), 1),
        vec![Effect::ReadConfig]
    );
    assert_eq!(h.phase(), Phase::Verifying);
    assert_eq!(
        h.step(Input::ConfigRead(Ok(requested())), 2),
        vec![
            Effect::WriteJournal(JournalPhase::Verified),
            Effect::WriteJournal(JournalPhase::ReattachGrace)
        ]
    );

    let mut h = H::new();
    h.step(start_input(requested()), 0);
    h.step(Input::ConfigRead(Ok(previous())), 0);
    h.step(Input::PostResult(Err(RouterError::Unreachable)), 1);
    // One read-back equal to the snapshot isn't enough (I3): read again in 10 s.
    assert_eq!(
        h.step(Input::ConfigRead(Ok(previous())), 2),
        vec![Effect::RetryAfter(Duration::from_secs(10))]
    );
    assert_eq!(h.phase(), Phase::Verifying);
}

/// I3: an uncertain POST whose first read-back shows the snapshot is re-read once, 10 s later,
/// before the change is closed as not applied.
#[test]
fn uncertain_post_rereads_before_closing() {
    let mut h = H::new();
    h.step(start_input(requested()), 0);
    h.step(Input::ConfigRead(Ok(previous())), 0);
    assert_eq!(
        h.step(Input::PostResult(Err(RouterError::Timeout)), 1),
        vec![Effect::ReadConfig]
    );
    assert_eq!(
        h.step(Input::ConfigRead(Ok(previous())), 2),
        vec![Effect::RetryAfter(Duration::from_secs(10))]
    );
    assert_eq!(h.phase(), Phase::Verifying);
    // Nothing until the 10 s are up, not even on observations.
    assert_eq!(h.tick_range(3, 11), vec![]);
    assert_eq!(h.step(obs(true), 11), vec![]);
    assert_eq!(h.step(Input::Tick, 12), vec![Effect::ReadConfig]);
    assert_eq!(
        h.step(Input::ConfigRead(Ok(previous())), 12),
        not_applied_effects()
    );
    assert_eq!(h.phase(), Phase::Idle);

    // A second read showing something else entirely: revert.
    let mut h = H::new();
    h.step(start_input(requested()), 0);
    h.step(Input::ConfigRead(Ok(previous())), 0);
    h.step(
        Input::PostResult(Err(RouterError::Parse {
            message: "x".into(),
        })),
        1,
    );
    h.step(Input::ConfigRead(Ok(previous())), 2);
    assert_eq!(h.step(Input::Tick, 12), vec![Effect::ReadConfig]);
    assert_eq!(
        h.step(Input::ConfigRead(Ok(fallback())), 12),
        revert_start()
    );
    assert_eq!(
        h.phase(),
        Phase::Reverting {
            attempt: 0,
            fallback: false
        }
    );
}

/// I3: the router applied the uncertain POST late: the second read shows `requested`, and
/// the change is watched like any other.
#[test]
fn uncertain_post_late_apply_is_watched() {
    let mut h = H::new();
    h.step(start_input(requested()), 0);
    h.step(Input::ConfigRead(Ok(previous())), 0);
    h.step(Input::PostResult(Err(RouterError::Timeout)), 1);
    h.step(Input::ConfigRead(Ok(previous())), 2);
    assert_eq!(h.step(Input::Tick, 12), vec![Effect::ReadConfig]);
    assert_eq!(
        h.step(Input::ConfigRead(Ok(requested())), 12),
        vec![
            Effect::WriteJournal(JournalPhase::Verified),
            Effect::WriteJournal(JournalPhase::ReattachGrace)
        ]
    );
    assert_eq!(h.phase(), Phase::ReattachGrace);
    // Still a bad band: no re-attach within grace (counted from the POST result) reverts.
    assert_eq!(h.tick_range(13, 90), vec![]);
    assert_eq!(h.step(Input::Tick, 91), revert_start());
}

/// I3: the re-read that finds no session waits for the session, then reads.
#[test]
fn uncertain_post_reread_waits_for_session() {
    let mut h = H::new();
    h.step(start_input(requested()), 0);
    h.step(Input::ConfigRead(Ok(previous())), 0);
    h.step(Input::PostResult(Err(RouterError::Timeout)), 1);
    h.step(Input::ConfigRead(Ok(previous())), 2);
    assert_eq!(h.step(Input::Tick, 12), vec![Effect::ReadConfig]);
    assert_eq!(h.step(Input::SessionUnavailable, 12), vec![]);
    assert_eq!(h.step(Input::Tick, 13), vec![Effect::ReadConfig]);
    assert_eq!(
        h.step(Input::ConfigRead(Ok(previous())), 13),
        not_applied_effects()
    );
}

#[test]
fn verify_mismatch_reverts() {
    let mut h = H::new();
    h.step(start_input(requested()), 0);
    h.step(Input::ConfigRead(Ok(previous())), 0);
    h.step(Input::PostResult(Ok(())), 1);
    assert_eq!(h.step(Input::ConfigRead(Ok(fallback())), 2), revert_start());
    assert_eq!(
        h.phase(),
        Phase::Reverting {
            attempt: 0,
            fallback: false
        }
    );
    assert_eq!(
        h.finish_revert(3),
        reverted_effects(RevertReason::VerifyMismatch)
    );
    assert_eq!(
        h.phase(),
        Phase::Reverted {
            reason: RevertReason::VerifyMismatch,
            fallback: false
        }
    );
    // A new change may start after a revert.
    assert_eq!(
        h.step(start_input(requested()), 10),
        vec![Effect::ReadConfig]
    );
}

/// Device test 2026-10-29: the router kept reporting the old cell in service for about 4 s
/// after the verify. Re-attach needs 10 s of good, fresh observations on one cell.
#[test]
fn reattach_needs_a_settled_window() {
    let mut h = H::with_timers(settling_timers());
    h.enter_grace();
    for s in 1..=10 {
        h.cell(true, s, 3, 103, s);
        assert_eq!(h.phase(), Phase::ReattachGrace, "still settling at {s} s");
    }
    let effects = h.cell(true, 11, 3, 103, 11);
    assert!(effects.contains(&Effect::Event(SafeApplyEvent::Reattached)));
    assert_eq!(h.phase(), Phase::Probation);
}

#[test]
fn a_drop_or_cell_change_restarts_settling() {
    let mut h = H::with_timers(settling_timers());
    h.enter_grace();
    for s in 1..=6 {
        h.cell(true, s, 3, 103, s);
    }
    // Cell change at 7: the window restarts there.
    for s in 7..=16 {
        h.cell(true, s, 3, 9, s);
        assert_eq!(
            h.phase(),
            Phase::ReattachGrace,
            "after the cell change, at {s} s"
        );
    }
    // A drop at 17 restarts it again.
    h.cell(false, 17, 3, 9, 17);
    for s in 18..=27 {
        h.cell(true, s, 3, 9, s);
        assert_eq!(h.phase(), Phase::ReattachGrace, "after the drop, at {s} s");
    }
    h.cell(true, 28, 3, 9, 28);
    assert_eq!(h.phase(), Phase::Probation);
}

#[test]
fn a_poll_started_before_the_verify_is_not_evidence() {
    let mut h = H::new(); // settle 0: any counted observation re-attaches at once
    h.step(start_input(requested()), 0);
    h.step(Input::ConfigRead(Ok(previous())), 0);
    h.step(Input::PostResult(Ok(())), 0);
    h.step(Input::ConfigRead(Ok(requested())), 5);
    assert_eq!(h.phase(), Phase::ReattachGrace);
    // In flight since 4 s, answered after the verify: still the old reading.
    assert_eq!(h.cell(true, 4, 3, 103, 6), vec![]);
    assert_eq!(h.phase(), Phase::ReattachGrace);
    h.cell(true, 6, 3, 103, 7);
    assert_eq!(h.phase(), Phase::Probation);
}

#[test]
fn a_band_the_new_mask_excludes_is_not_reattach() {
    let mut h = H::new();
    h.enter_grace(); // requested allows B3 only
    for s in 1..=20 {
        h.cell(true, s, 1, 103, s);
    }
    assert_eq!(h.phase(), Phase::ReattachGrace);
    h.cell(true, 21, 3, 103, 21);
    assert_eq!(h.phase(), Phase::Probation);
}

/// Device test 2026-10-29: right after accepting the POST the router answered reads with the
/// old config or errors while it re-attached, and one such read caused a revert.
#[test]
fn verify_rereads_while_the_router_still_shows_the_old_config() {
    let mut h = H::new();
    h.step(start_input(requested()), 0);
    h.step(Input::ConfigRead(Ok(previous())), 0);
    h.step(Input::PostResult(Ok(())), 1);
    assert_eq!(
        h.step(Input::ConfigRead(Ok(previous())), 2),
        vec![Effect::RetryAfter(Duration::from_secs(5))]
    );
    assert_eq!(h.phase(), Phase::Verifying);
    assert_eq!(h.step(Input::Tick, 7), vec![Effect::ReadConfig]);
    assert_eq!(
        h.step(Input::ConfigRead(Err(api_err())), 7),
        vec![Effect::RetryAfter(Duration::from_secs(5))]
    );
    assert_eq!(h.step(Input::Tick, 12), vec![Effect::ReadConfig]);
    h.step(Input::ConfigRead(Ok(requested())), 12);
    assert_eq!(h.phase(), Phase::ReattachGrace);
}

#[test]
fn verify_gives_up_after_the_window() {
    let mut h = H::new();
    h.step(start_input(requested()), 0);
    h.step(Input::ConfigRead(Ok(previous())), 0);
    h.step(Input::PostResult(Ok(())), 1);
    for s in [2, 7, 12] {
        assert_eq!(
            h.step(Input::ConfigRead(Ok(previous())), s),
            vec![Effect::RetryAfter(Duration::from_secs(5))],
            "re-read at {s} s"
        );
        h.step(Input::Tick, s + 5);
    }
    assert_eq!(
        h.step(Input::ConfigRead(Ok(previous())), 17),
        revert_start()
    );
}

#[test]
fn no_reattach_within_grace_reverts() {
    let mut h = H::new();
    h.enter_grace();
    assert_eq!(h.tick_range(1, 89), vec![]);
    assert_eq!(h.step(Input::Tick, 90), revert_start());
    assert_eq!(
        h.finish_revert(91),
        reverted_effects(RevertReason::NoReattach)
    );
}

#[test]
fn reattach_at_85s_is_not_loss() {
    let mut h = H::new();
    h.enter_grace();
    let mut all = h.observe_range(false, 1, 85);
    all.extend(h.step(obs(true), 86));
    assert!(!has_post(&all), "{all:?}");
    assert_eq!(h.phase(), Phase::Probation);
    // Probation is counted from the re-attach, not from grace start.
    assert_eq!(h.observe_range(true, 87, 265), vec![]);
    assert_eq!(h.step(obs(true), 266), commit_effects());
}

#[test]
fn loss_60s_continuous_reverts() {
    let mut h = H::new();
    h.enter_probation();
    assert_eq!(h.observe_range(true, 1, 19), vec![]);
    let all = h.observe_range(false, 20, 79);
    assert!(!has_post(&all), "{all:?}");
    assert_eq!(h.step(obs(false), 80), revert_start());
    assert_eq!(
        h.finish_revert(81),
        reverted_effects(RevertReason::LossOfService)
    );
}

#[test]
fn loss_is_also_detected_by_tick() {
    let mut h = H::new();
    h.enter_probation();
    h.step(obs(false), 20);
    h.step(obs(false), 21);
    assert_eq!(h.step(Input::Tick, 22), vec![]);
    // The loss threshold is reached while observations are still fresh.
    let mut h2 = H::new();
    h2.enter_probation();
    h2.observe_range(false, 20, 79);
    assert_eq!(h2.step(Input::Tick, 80), revert_start());
}

#[test]
fn intermittent_loss_does_not_revert() {
    let mut h = H::new();
    h.enter_probation();
    let mut all = h.observe_range(false, 1, 50);
    all.extend(h.step(obs(true), 51));
    all.extend(h.observe_range(false, 52, 101));
    assert!(!has_post(&all), "{all:?}");
    assert_eq!(h.phase(), Phase::Probation);
}

#[test]
fn observation_gap_resets_loss_timer() {
    let mut h = H::new();
    h.enter_probation();
    h.observe_range(true, 1, 10);
    assert_eq!(h.step(obs(false), 20), vec![]);
    // The PC slept: 600 s with no input of any kind. The first post-gap observation must not
    // count the gap as loss; loss counts afresh from it.
    assert_eq!(h.step(obs(false), 620), vec![]);
    assert_eq!(h.phase(), Phase::Probation);
    let all = h.observe_range(false, 621, 679);
    assert!(!has_post(&all), "{all:?}");
    assert_eq!(h.step(obs(false), 680), revert_start());
}

#[test]
fn blank_signal_is_not_in_service() {
    let body = std::fs::read_to_string(format!(
        "{}/tests/fixtures/huawei_h122/signal_reattach.xml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let signal = parse_signal(&body).unwrap();
    let status = Status {
        connection: ConnState::Connected,
        network_type: 19,
        operator: None,
        bars: None,
        nr_bars: None,
        endc: None,
        endc_restricted: None,
    };
    assert!(!in_service(&status, &signal));
}

/// RevertNow at t=20, then three failures of the snapshot POST.
fn fail_previous_three_times(h: &mut H) {
    h.enter_probation();
    assert_eq!(h.step(Input::RevertNow, 20), revert_start());

    assert_eq!(
        h.step(Input::PostResult(Err(RouterError::Timeout)), 20),
        vec![Effect::RetryAfter(Duration::from_secs(5))]
    );
    assert_eq!(
        h.phase(),
        Phase::Reverting {
            attempt: 1,
            fallback: false
        }
    );
    assert_eq!(h.step(Input::Tick, 24), vec![]);
    assert_eq!(h.step(Input::Tick, 25), vec![Effect::Post(previous())]);

    assert_eq!(
        h.step(Input::PostResult(Err(api_err())), 25),
        vec![Effect::RetryAfter(Duration::from_secs(15))]
    );
    assert_eq!(h.step(obs(false), 39), vec![]);
    assert_eq!(h.step(Input::Tick, 40), vec![Effect::Post(previous())]);

    assert_eq!(
        h.step(Input::PostResult(Err(RouterError::Unreachable)), 40),
        vec![Effect::RetryAfter(Duration::from_secs(30))]
    );
    assert_eq!(
        h.phase(),
        Phase::Reverting {
            attempt: 3,
            fallback: false
        }
    );
    assert_eq!(h.step(Input::Tick, 69), vec![]);
}

#[test]
fn revert_retries_then_fallback() {
    let mut h = H::new();
    fail_previous_three_times(&mut h);
    assert_eq!(
        h.step(Input::Tick, 70),
        vec![
            Effect::Post(fallback()),
            Effect::Event(SafeApplyEvent::FellBackToAuto)
        ]
    );
    assert_eq!(
        h.phase(),
        Phase::Reverting {
            attempt: 0,
            fallback: true
        }
    );
    // The fallback applies and re-attaches: reverted.
    assert_eq!(
        h.step(Input::PostResult(Ok(())), 70),
        vec![Effect::ReadConfig]
    );
    assert_eq!(h.step(Input::ConfigRead(Ok(fallback())), 71), vec![]);
    assert_eq!(
        h.step(obs(true), 80),
        reverted_via_auto_effects(RevertReason::UserRequested)
    );
    assert_eq!(
        h.phase(),
        Phase::Reverted {
            reason: RevertReason::UserRequested,
            fallback: true
        }
    );
}

#[test]
fn failed_revert_verify_counts_as_failed_attempt() {
    let mut h = H::new();
    h.enter_probation();
    h.step(Input::RevertNow, 20);
    assert_eq!(
        h.step(Input::PostResult(Ok(())), 20),
        vec![Effect::ReadConfig]
    );
    assert_eq!(
        h.step(Input::ConfigRead(Ok(requested())), 21),
        vec![Effect::RetryAfter(Duration::from_secs(5))]
    );
    assert_eq!(h.step(Input::Tick, 26), vec![Effect::Post(previous())]);
}

#[test]
fn reverted_config_without_service_falls_back() {
    let mut h = H::new();
    h.enter_probation();
    h.step(Input::RevertNow, 20);
    h.step(Input::PostResult(Ok(())), 20);
    assert_eq!(h.step(Input::ConfigRead(Ok(previous())), 21), vec![]);
    assert_eq!(
        h.sa.view(h.t0 + Duration::from_secs(51)).seconds_left,
        Some(60)
    );
    assert_eq!(h.observe_range(false, 22, 110), vec![]);
    assert_eq!(
        h.step(Input::Tick, 111),
        vec![
            Effect::Post(fallback()),
            Effect::Event(SafeApplyEvent::FellBackToAuto)
        ]
    );
    // The fallback doesn't re-attach either: revert failed.
    h.step(Input::PostResult(Ok(())), 111);
    h.step(Input::ConfigRead(Ok(fallback())), 111);
    assert_eq!(
        h.step(obs(false), 201),
        vec![
            Effect::WriteJournal(JournalPhase::RevertFailed),
            Effect::Event(SafeApplyEvent::RevertFailed),
            Effect::RetryAfter(Duration::from_secs(30))
        ]
    );
    assert_eq!(h.phase(), Phase::RevertFailed);
}

#[test]
fn fallback_failure_is_revert_failed_and_keeps_journal() {
    let mut h = H::new();
    let mut all = Vec::new();
    fail_previous_three_times(&mut h);
    all.extend(h.step(Input::Tick, 70));
    all.extend(h.step(Input::PostResult(Err(RouterError::Timeout)), 70));
    all.extend(h.step(Input::Tick, 75));
    all.extend(h.step(Input::PostResult(Err(RouterError::Timeout)), 75));
    all.extend(h.step(Input::Tick, 90));
    assert_eq!(all.last(), Some(&Effect::Post(fallback())));
    let last = h.step(Input::PostResult(Err(RouterError::Timeout)), 90);
    assert_eq!(
        last,
        vec![
            Effect::WriteJournal(JournalPhase::RevertFailed),
            Effect::Event(SafeApplyEvent::RevertFailed),
            Effect::RetryAfter(Duration::from_secs(30))
        ]
    );
    all.extend(last);
    assert!(!all.contains(&Effect::ArchiveJournal), "{all:?}");
    assert_eq!(h.phase(), Phase::RevertFailed);
    assert_eq!(h.sa.record().unwrap().phase, JournalPhase::RevertFailed);

    // RevertFailed keeps retrying while the app runs.
    assert_eq!(h.step(Input::Tick, 119), vec![]);
    assert_eq!(h.step(Input::Tick, 120), revert_start());
    assert_eq!(
        h.phase(),
        Phase::Reverting {
            attempt: 0,
            fallback: false
        }
    );
}

#[test]
fn session_loss_during_revert_is_pending() {
    let mut h = H::new();
    h.enter_probation();
    assert_eq!(h.step(Input::RevertNow, 20), revert_start());
    assert_eq!(h.step(Input::SessionUnavailable, 20), vec![]);
    assert_eq!(
        h.phase(),
        Phase::RevertPending {
            reason: RevertReason::UserRequested
        }
    );
    // R4a: a routine Tick doesn't resume the pending revert; a successful poll does.
    assert_eq!(h.step(Input::Tick, 25), vec![]);
    assert_eq!(h.step(obs(false), 26), vec![Effect::Post(previous())]);
    assert_eq!(
        h.phase(),
        Phase::Reverting {
            attempt: 0,
            fallback: false
        }
    );

    // Due by the grace deadline, no session: pending, then re-issued on the next observation.
    let mut h = H::new();
    h.enter_grace();
    h.tick_range(1, 89);
    assert_eq!(h.step(Input::Tick, 90), revert_start());
    h.step(Input::SessionUnavailable, 90);
    assert_eq!(
        h.phase(),
        Phase::RevertPending {
            reason: RevertReason::NoReattach
        }
    );
    h.step(Input::SessionUnavailable, 95);
    assert_eq!(h.step(obs(false), 100), vec![Effect::Post(previous())]);
    assert_eq!(
        h.finish_revert(101),
        reverted_effects(RevertReason::NoReattach)
    );
}

#[test]
fn session_loss_during_probation_keeps_timers_running() {
    let mut h = H::new();
    h.enter_probation();
    h.observe_range(false, 1, 30);
    assert_eq!(h.step(Input::SessionUnavailable, 30), vec![]);
    assert_eq!(h.phase(), Phase::Probation);
    // Loss started at t=1, so the 60 s threshold is reached at t=61.
    let all = h.observe_range(false, 31, 61);
    assert_eq!(all, revert_start());
}

#[test]
fn keep_now_commits() {
    let mut h = H::new();
    h.enter_probation();
    h.observe_range(true, 1, 30);
    assert_eq!(h.step(Input::KeepNow, 30), commit_effects());
    assert_eq!(h.phase(), Phase::Committed);
}

#[test]
fn giving_up_reverts_with_a_reason_that_counts_as_no_service() {
    let mut h = H::new();
    h.enter_probation();
    assert_eq!(h.step(Input::GiveUp, 30), revert_start());
    assert_eq!(
        h.finish_revert(31),
        reverted_effects(RevertReason::LossOfService)
    );
}

#[test]
fn revert_now_reverts_with_user_reason() {
    let mut h = H::new();
    h.enter_probation();
    assert_eq!(h.step(Input::RevertNow, 30), revert_start());
    assert_eq!(
        h.finish_revert(31),
        reverted_effects(RevertReason::UserRequested)
    );
    assert_eq!(
        h.phase(),
        Phase::Reverted {
            reason: RevertReason::UserRequested,
            fallback: false
        }
    );

    let mut h = H::new();
    h.enter_grace();
    assert_eq!(h.step(Input::RevertNow, 5), revert_start());
}

#[test]
fn resume_applied_equals_requested_restarts_grace() {
    let (mut h, effects) = H::resumed(journal_record(JournalPhase::Applied));
    assert_eq!(effects, vec![Effect::ReadConfig]);
    assert_eq!(h.phase(), Phase::Verifying);
    assert_eq!(
        h.step(Input::ConfigRead(Ok(requested())), 10),
        vec![Effect::WriteJournal(JournalPhase::ReattachGrace)]
    );
    assert_eq!(h.phase(), Phase::ReattachGrace);
    // Fresh timers: grace counts from the read-back, not from the journal timestamps.
    assert_eq!(h.tick_range(11, 99), vec![]);
    assert_eq!(h.step(Input::Tick, 100), revert_start());
    assert_eq!(
        h.finish_revert(101),
        reverted_effects(RevertReason::ResumedWithoutService)
    );
}

#[test]
fn resume_applied_equals_previous_archives() {
    let (mut h, effects) = H::resumed(journal_record(JournalPhase::Journaled));
    assert_eq!(effects, vec![Effect::ReadConfig]);
    assert_eq!(
        h.step(Input::ConfigRead(Ok(previous())), 1),
        vec![Effect::ArchiveJournal]
    );
    assert_eq!(h.phase(), Phase::Idle);
    assert_eq!(
        h.sa.record().unwrap().id,
        journal_record(JournalPhase::Journaled).id
    );
}

#[test]
fn resume_applied_other_config_reverts() {
    let (mut h, _) = H::resumed(journal_record(JournalPhase::Applied));
    assert_eq!(h.step(Input::ConfigRead(Ok(fallback())), 1), revert_start());
}

#[test]
fn resume_probation_restarts_grace() {
    for phase in [
        JournalPhase::Verified,
        JournalPhase::ReattachGrace,
        JournalPhase::Probation,
    ] {
        let (mut h, effects) = H::resumed(journal_record(phase));
        assert_eq!(
            effects,
            vec![Effect::WriteJournal(JournalPhase::ReattachGrace)]
        );
        assert_eq!(h.phase(), Phase::ReattachGrace);
        assert_eq!(h.sa.view(h.t0).seconds_left, Some(90));
        assert_eq!(
            h.step(obs(true), 5),
            vec![
                Effect::WriteJournal(JournalPhase::Probation),
                Effect::Event(SafeApplyEvent::Reattached)
            ]
        );
        h.observe_range(true, 6, 184);
        assert_eq!(h.step(obs(true), 185), commit_effects());
    }
}

#[test]
fn resume_reverting_restarts_revert() {
    for phase in [JournalPhase::Reverting, JournalPhase::RevertFailed] {
        let (h, effects) = H::resumed(journal_record(phase));
        assert_eq!(effects, revert_start());
        assert_eq!(
            h.phase(),
            Phase::Reverting {
                attempt: 0,
                fallback: false
            }
        );
    }
}

#[test]
fn resume_closed_journal_just_archives() {
    for phase in [JournalPhase::Committed, JournalPhase::Reverted] {
        let (h, effects) = H::resumed(journal_record(phase));
        assert_eq!(effects, vec![Effect::ArchiveJournal]);
        assert_eq!(h.phase(), Phase::Idle);
    }
}

#[test]
fn resume_refuses_foreign_journal() {
    let mut other_host = journal_record(JournalPhase::Probation);
    other_host.host = "192.168.1.1".to_string();
    let err = SafeApply::resume(other_host.clone(), timers(), HOST, DRIVER, Instant::now())
        .err()
        .unwrap();
    assert_eq!(err, ForeignJournal(other_host));

    let mut other_driver = journal_record(JournalPhase::Applied);
    other_driver.driver = "mock".to_string();
    let err = SafeApply::resume(other_driver.clone(), timers(), HOST, DRIVER, Instant::now())
        .err()
        .unwrap();
    assert_eq!(err, ForeignJournal(other_driver));
}

#[test]
fn phase_and_event_serialize_with_kind_tag() {
    let json = serde_json::to_value(Phase::Reverting {
        attempt: 2,
        fallback: true,
    })
    .unwrap();
    assert_eq!(
        json,
        serde_json::json!({ "kind": "reverting", "attempt": 2, "fallback": true })
    );
    let json = serde_json::to_value(SafeApplyEvent::Reverted {
        reason: RevertReason::LossOfService,
        fallback: true,
    })
    .unwrap();
    assert_eq!(
        json,
        serde_json::json!({ "kind": "reverted", "reason": "lossOfService", "fallback": true })
    );
    let json = serde_json::to_value(Phase::Reverted {
        reason: RevertReason::NoReattach,
        fallback: false,
    })
    .unwrap();
    assert_eq!(
        json,
        serde_json::json!({ "kind": "reverted", "reason": "noReattach", "fallback": false })
    );
}

// ---- Fix round 1 (rulings R1-R4) ----

#[test]
fn probation_reverts_at_5s_polling() {
    let mut h = H::new();
    h.sa.set_poll_interval(Duration::from_secs(5));
    h.enter_probation();
    h.every(obs(true), 5, 15, 5);
    // Out of service from t=20, observed every 5 s.
    let before = h.every(obs(false), 20, 75, 5);
    assert!(!has_post(&before), "{before:?}");
    assert_eq!(h.step(obs(false), 80), revert_start());
    assert_eq!(
        h.finish_revert(81),
        reverted_effects(RevertReason::LossOfService)
    );
}

#[test]
fn probation_reverts_with_default_interval_at_5s_polling() {
    // No set_poll_interval: the 45 s gap floor keeps 5 s polling from looking like sleep.
    let mut h = H::new();
    h.enter_probation();
    h.every(obs(true), 5, 15, 5);
    let before = h.every(obs(false), 20, 75, 5);
    assert!(!has_post(&before), "{before:?}");
    assert_eq!(h.step(obs(false), 80), revert_start());
}

#[test]
fn loss_runs_through_session_outage() {
    let mut h = H::new();
    h.enter_probation();
    h.every(obs(true), 5, 15, 5);
    assert_eq!(h.step(obs(false), 20), vec![]);
    let before = h.every(Input::SessionUnavailable, 25, 75, 5);
    assert_eq!(before, vec![]);
    assert_eq!(h.phase(), Phase::Probation);
    // Loss reaches 60 s during the outage: the revert is due but can't be posted.
    assert_eq!(
        h.step(Input::SessionUnavailable, 80),
        vec![Effect::WriteJournal(JournalPhase::Reverting)]
    );
    assert_eq!(
        h.phase(),
        Phase::RevertPending {
            reason: RevertReason::LossOfService
        }
    );
    assert_eq!(h.every(Input::SessionUnavailable, 85, 120, 5), vec![]);
    // Session back: the next successful poll resumes the revert (R4a).
    assert_eq!(h.step(Input::Tick, 121), vec![]);
    assert_eq!(h.step(obs(false), 121), vec![Effect::Post(previous())]);
    assert_eq!(
        h.finish_revert(122),
        reverted_effects(RevertReason::LossOfService)
    );
}

#[test]
fn takeover_while_in_service_waits() {
    let mut h = H::new();
    h.enter_probation();
    h.every(obs(true), 5, 10, 5);
    // The browser took the session over while service was fine; the actor keeps ticking.
    let mut all = Vec::new();
    for s in 11..=300 {
        if s % 5 == 0 {
            all.extend(h.step(Input::SessionUnavailable, s));
        }
        all.extend(h.step(Input::Tick, s));
    }
    assert!(!has_post(&all), "{all:?}");
    assert!(!all.contains(&Effect::ArchiveJournal), "{all:?}");
    assert_eq!(h.phase(), Phase::Probation);
    assert_eq!(h.step(obs(true), 301), commit_effects());
}

#[test]
fn verifying_has_deadline() {
    let mut h = H::new();
    h.step(start_input(requested()), 0);
    h.step(Input::ConfigRead(Ok(previous())), 0);
    assert_eq!(
        h.step(Input::PostResult(Ok(())), 1),
        vec![
            Effect::WriteJournal(JournalPhase::Applied),
            Effect::ReadConfig
        ]
    );
    assert_eq!(h.step(Input::SessionUnavailable, 1), vec![]);
    let before = h.every(Input::SessionUnavailable, 6, 86, 5);
    assert_eq!(before, vec![]);
    assert_eq!(h.phase(), Phase::Verifying);
    assert_eq!(
        h.step(Input::SessionUnavailable, 91),
        vec![Effect::WriteJournal(JournalPhase::Reverting)]
    );
    assert_eq!(
        h.phase(),
        Phase::RevertPending {
            reason: RevertReason::NoReattach
        }
    );
}

#[test]
fn verifying_deadline_reverts_with_session_on_tick() {
    let mut h = H::new();
    h.step(start_input(requested()), 0);
    h.step(Input::ConfigRead(Ok(previous())), 0);
    h.step(Input::PostResult(Ok(())), 1);
    h.step(Input::SessionUnavailable, 1);
    // The re-issued read is never answered with a match; at the deadline it reverts.
    assert_eq!(h.step(Input::Tick, 2), vec![Effect::ReadConfig]);
    assert_eq!(h.tick_range(3, 90), vec![]);
    assert_eq!(h.step(Input::Tick, 91), revert_start());
}

#[test]
fn revert_post_session_loss_is_pending_not_attempt() {
    let mut h = H::new();
    h.enter_probation();
    assert_eq!(h.step(Input::RevertNow, 20), revert_start());
    assert_eq!(
        h.step(
            Input::PostResult(Err(RouterError::SessionLost { code: None })),
            20
        ),
        vec![]
    );
    assert_eq!(
        h.phase(),
        Phase::RevertPending {
            reason: RevertReason::UserRequested
        }
    );
    assert_eq!(h.step(Input::SessionUnavailable, 22), vec![]);
    assert_eq!(h.step(obs(false), 23), vec![Effect::Post(previous())]);
    assert_eq!(
        h.phase(),
        Phase::Reverting {
            attempt: 0,
            fallback: false
        }
    );
    assert_eq!(
        h.step(Input::PostResult(Err(RouterError::Timeout)), 23),
        vec![Effect::RetryAfter(Duration::from_secs(5))]
    );
    assert_eq!(h.step(Input::Tick, 28), vec![Effect::Post(previous())]);
    assert_eq!(
        h.step(Input::PostResult(Err(RouterError::Timeout)), 28),
        vec![Effect::RetryAfter(Duration::from_secs(15))]
    );
    assert_eq!(h.step(Input::Tick, 43), vec![Effect::Post(previous())]);
    assert_eq!(
        h.step(Input::PostResult(Err(RouterError::Timeout)), 43),
        vec![Effect::RetryAfter(Duration::from_secs(30))]
    );
    assert_eq!(
        h.step(Input::Tick, 73),
        vec![
            Effect::Post(fallback()),
            Effect::Event(SafeApplyEvent::FellBackToAuto)
        ]
    );
}

#[test]
fn gap_in_reattach_grace_restarts_the_window() {
    let mut h = H::new();
    h.enter_grace();
    h.tick_range(1, 30);
    // Suspended for 10 minutes, then the first Tick: the grace window restarts from it.
    assert_eq!(h.step(Input::Tick, 630), vec![]);
    assert_eq!(
        h.sa.view(h.t0 + Duration::from_secs(630)).seconds_left,
        Some(90)
    );
    assert_eq!(h.tick_range(631, 719), vec![]);
    assert_eq!(h.step(Input::Tick, 720), revert_start());
}

#[test]
fn commit_after_gap_needs_a_fresh_in_service_observation() {
    let mut h = H::new();
    h.enter_probation();
    h.observe_range(true, 1, 10);
    h.step(obs(false), 11);
    // Suspended past the probation end; the loss timer was cleared by the gap, but the last
    // observation was out of service, so a Tick must not commit.
    assert_eq!(h.step(Input::Tick, 400), vec![]);
    assert_eq!(h.phase(), Phase::Probation);
    assert_eq!(h.step(obs(true), 401), commit_effects());
}

// ---- Fix round 2 (rulings R1a, R4a) ----

#[test]
fn gap_after_loss_then_outage_reaches_revert_pending() {
    let mut h = H::new();
    h.enter_probation();
    h.observe_range(true, 1, 10);
    assert_eq!(h.step(obs(false), 20), vec![]);
    // Suspended until t=620, then only session failures.
    let before = h.every(Input::SessionUnavailable, 620, 675, 5);
    assert_eq!(before, vec![]);
    assert_eq!(
        h.step(Input::SessionUnavailable, 680),
        vec![Effect::WriteJournal(JournalPhase::Reverting)]
    );
    assert_eq!(
        h.phase(),
        Phase::RevertPending {
            reason: RevertReason::LossOfService
        }
    );
}

#[test]
fn wake_tick_does_not_commit_on_stale_observation() {
    let mut h = H::new();
    h.enter_probation();
    h.observe_range(true, 1, 10);
    // Suspended until t=400, past the probation deadline (t=180).
    let all: Vec<Effect> = [400, 401, 402]
        .into_iter()
        .flat_map(|s| h.step(Input::Tick, s))
        .collect();
    assert_eq!(all, vec![]);
    assert_eq!(h.phase(), Phase::Probation);
    assert_eq!(h.step(obs(true), 403), commit_effects());
}

#[test]
fn wake_in_grace_does_not_commit_and_still_reverts() {
    // Bad news still fires without fresh evidence: grace restarted at the wake, reverts 90 s later.
    let mut h = H::new();
    h.enter_grace();
    h.tick_range(1, 10);
    assert_eq!(h.step(Input::Tick, 500), vec![]);
    assert_eq!(h.tick_range(501, 589), vec![]);
    assert_eq!(h.step(Input::Tick, 590), revert_start());
}

#[test]
fn revert_pending_does_not_flicker_on_ticks() {
    let mut h = H::new();
    h.enter_probation();
    h.step(Input::RevertNow, 20);
    h.step(Input::SessionUnavailable, 20);
    let pending = Phase::RevertPending {
        reason: RevertReason::UserRequested,
    };
    assert_eq!(h.phase(), pending);
    for s in 21..=30 {
        assert_eq!(h.step(Input::Tick, s), vec![]);
        assert_eq!(h.phase(), pending);
    }
    // Start and KeepNow never resume it.
    assert!(!has_post(&h.step(start_input(fallback()), 31)));
    assert_eq!(h.step(Input::KeepNow, 31), vec![]);
    assert_eq!(h.phase(), pending);
    assert_eq!(h.step(obs(false), 32), vec![Effect::Post(previous())]);
    assert_eq!(
        h.phase(),
        Phase::Reverting {
            attempt: 0,
            fallback: false
        }
    );
}

#[test]
fn revert_now_resumes_a_pending_revert() {
    let mut h = H::new();
    h.enter_probation();
    h.step(Input::RevertNow, 20);
    h.step(Input::SessionUnavailable, 20);
    assert_eq!(h.step(Input::RevertNow, 25), vec![Effect::Post(previous())]);
}

#[test]
fn revert_readback_session_loss_is_pending() {
    let mut h = H::new();
    h.enter_probation();
    h.step(Input::RevertNow, 20);
    // One failed attempt first, so the counter is visibly preserved.
    h.step(Input::PostResult(Err(RouterError::Timeout)), 20);
    assert_eq!(h.step(Input::Tick, 25), vec![Effect::Post(previous())]);
    assert_eq!(
        h.step(Input::PostResult(Ok(())), 25),
        vec![Effect::ReadConfig]
    );
    assert_eq!(
        h.step(
            Input::ConfigRead(Err(RouterError::SessionLost { code: None })),
            26
        ),
        vec![]
    );
    assert_eq!(
        h.phase(),
        Phase::RevertPending {
            reason: RevertReason::UserRequested
        }
    );
    assert_eq!(h.step(Input::Tick, 27), vec![]);
    // The read-back is re-issued, not a new POST.
    assert_eq!(h.step(obs(false), 28), vec![Effect::ReadConfig]);
    assert_eq!(
        h.phase(),
        Phase::Reverting {
            attempt: 1,
            fallback: false
        }
    );
    assert_eq!(h.step(Input::ConfigRead(Ok(previous())), 28), vec![]);
    assert_eq!(
        h.step(obs(true), 29),
        reverted_effects(RevertReason::UserRequested)
    );
}

// --- loss policy (revert_on_loss) and revert target ------------------------------------------

fn loss_ignored(effects: &[Effect]) -> Vec<RevertReason> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::LossIgnored(r) => Some(*r),
            _ => None,
        })
        .collect()
}

impl H {
    fn policy_off(&mut self, s: u64) {
        self.sa
            .set_revert_on_loss(false, self.t0 + Duration::from_secs(s));
    }

    fn view(&self, s: u64) -> router_core::SafeApplyView {
        self.sa.view(self.t0 + Duration::from_secs(s))
    }
}

#[test]
fn default_policy_is_revert_on_loss() {
    let mut h = H::new();
    assert!(h.sa.revert_on_loss());
    let policy = h.view(0).policy.unwrap();
    assert!(policy.revert_on_loss && !policy.loss_ignored);
    h.enter_probation();
    // The default path never reports an ignored loss.
    let effects = h.observe_range(false, 1, 61);
    assert!(loss_ignored(&effects).is_empty());
    assert!(has_post(&effects));
}

#[test]
fn policy_off_probation_loss_is_kept_and_committed_at_the_end() {
    let mut h = H::new();
    h.enter_probation();
    h.policy_off(0);
    let effects = h.observe_range(false, 1, 179);
    assert!(!has_post(&effects), "no revert with the policy off");
    assert_eq!(loss_ignored(&effects), vec![RevertReason::LossOfService]);
    assert_eq!(h.phase(), Phase::Probation);
    assert!(h.view(100).policy.unwrap().loss_ignored);
    // Committed when the timer ends, still without service.
    assert_eq!(h.step(obs(false), 180), commit_effects());
    assert_eq!(h.phase(), Phase::Committed);
}

#[test]
fn policy_off_commit_also_falls_due_without_a_session() {
    let mut h = H::new();
    h.enter_probation();
    h.policy_off(0);
    h.observe_range(false, 1, 100);
    let effects = h.every(Input::SessionUnavailable, 101, 179, 1);
    assert!(!has_post(&effects));
    assert_eq!(h.step(Input::SessionUnavailable, 180), commit_effects());
}

#[test]
fn policy_off_grace_without_reattach_moves_to_probation() {
    let mut h = H::new();
    h.enter_grace();
    h.policy_off(0);
    assert!(!has_post(&h.observe_range(false, 1, 89)));
    let effects = h.step(obs(false), 90);
    assert_eq!(
        effects,
        vec![
            Effect::WriteJournal(JournalPhase::Probation),
            Effect::LossIgnored(RevertReason::NoReattach),
        ]
    );
    assert_eq!(h.phase(), Phase::Probation);
    // The full probation runs from the end of grace; the loss isn't reported twice.
    let effects = h.observe_range(false, 91, 269);
    assert!(!has_post(&effects));
    assert!(loss_ignored(&effects).is_empty());
    assert_eq!(h.step(Input::Tick, 270), commit_effects());
}

#[test]
fn policy_off_resumed_without_service_is_kept() {
    let (mut h, _) = H::resumed(journal_record(JournalPhase::Probation));
    h.policy_off(0);
    assert_eq!(h.phase(), Phase::ReattachGrace);
    let effects = h.observe_range(false, 1, 90);
    assert!(!has_post(&effects));
    assert_eq!(
        loss_ignored(&effects),
        vec![RevertReason::ResumedWithoutService]
    );
    assert_eq!(h.phase(), Phase::Probation);
    assert_eq!(h.step(obs(true), 270), commit_effects());
}

#[test]
fn policy_off_still_reverts_when_the_router_did_not_take_the_change() {
    // Verify mismatch.
    let mut h = H::new();
    h.policy_off(0);
    h.step(start_input(requested()), 0);
    h.step(Input::ConfigRead(Ok(previous())), 0);
    h.step(Input::PostResult(Ok(())), 0);
    assert_eq!(h.step(Input::ConfigRead(Ok(fallback())), 1), revert_start());

    // Rejected apply.
    let mut h = H::new();
    h.policy_off(0);
    h.step(start_input(requested()), 0);
    h.step(Input::ConfigRead(Ok(previous())), 0);
    assert_eq!(
        h.step(Input::PostResult(Err(api_err())), 0),
        not_applied_effects()
    );

    // A verify that never completes.
    let mut h = H::new();
    h.policy_off(0);
    h.step(start_input(requested()), 0);
    h.step(Input::ConfigRead(Ok(previous())), 0);
    h.step(Input::PostResult(Ok(())), 0);
    h.step(Input::SessionUnavailable, 0);
    let effects = h.every(Input::SessionUnavailable, 1, 90, 1);
    assert!(
        effects.contains(&Effect::WriteJournal(JournalPhase::Reverting)),
        "{effects:?}"
    );
}

#[test]
fn policy_off_revert_now_still_works() {
    let mut h = H::new();
    h.enter_probation();
    h.policy_off(0);
    h.observe_range(false, 1, 100);
    assert_eq!(h.step(Input::RevertNow, 101), revert_start());
    assert_eq!(
        h.finish_revert(102),
        reverted_effects(RevertReason::UserRequested)
    );

    let mut h = H::new();
    h.enter_grace();
    h.policy_off(0);
    assert_eq!(h.step(Input::RevertNow, 5), revert_start());
}

#[test]
fn policy_back_on_restarts_the_loss_timer() {
    let mut h = H::new();
    h.enter_probation();
    h.policy_off(0);
    h.observe_range(false, 1, 100);
    h.sa.set_revert_on_loss(true, h.t0 + Duration::from_secs(100));
    assert!(!has_post(&h.observe_range(false, 101, 159)));
    assert_eq!(h.step(obs(false), 160), revert_start());
}

fn original() -> NetConfig {
    NetConfig {
        network_mode: NetworkMode::Auto,
        lte_bands: LteBandSet::from_bands(&[1, 3, 20, 28]).unwrap(),
        legacy_band_raw: "3FFFFFFF".to_string(),
        nr_option: Some(NrOption::Sa),
    }
}

fn start_reverting_to(requested: NetConfig, revert_to: NetConfig) -> Input {
    Input::Start {
        requested,
        caps: caps(),
        current: Some(previous()),
        revert_to: Some(revert_to),
    }
}

#[test]
fn revert_to_overrides_the_revert_target_but_not_the_verify() {
    let mut h = H::new();
    h.step(start_reverting_to(requested(), original()), 0);
    h.step(Input::ConfigRead(Ok(previous())), 0);
    // The journal records the revert target.
    assert_eq!(h.sa.record().unwrap().previous, original());
    h.step(Input::PostResult(Ok(())), 0);
    // The router still shows the snapshot: read again, no mismatch.
    assert_eq!(
        h.step(Input::ConfigRead(Ok(previous())), 1),
        vec![Effect::RetryAfter(Duration::from_secs(5))]
    );
    h.step(Input::Tick, 6);
    h.step(Input::ConfigRead(Ok(requested())), 6);
    assert_eq!(h.phase(), Phase::ReattachGrace);
    assert_eq!(h.view(6).previous, Some(original()));
    assert_eq!(
        h.step(Input::RevertNow, 7),
        vec![
            Effect::WriteJournal(JournalPhase::Reverting),
            Effect::Post(original()),
        ]
    );
    h.step(Input::PostResult(Ok(())), 7);
    // The revert is confirmed against the target, not the snapshot.
    assert_eq!(h.step(Input::ConfigRead(Ok(original())), 8), vec![]);
    assert_eq!(
        h.step(obs(true), 9),
        reverted_effects(RevertReason::UserRequested)
    );
}

#[test]
fn revert_to_applies_to_automatic_reverts() {
    let mut h = H::new();
    h.step(start_reverting_to(requested(), original()), 0);
    h.step(Input::ConfigRead(Ok(previous())), 0);
    h.step(Input::PostResult(Ok(())), 0);
    h.step(Input::ConfigRead(Ok(requested())), 0);
    let effects = h.observe_range(false, 1, 90);
    assert!(effects.contains(&Effect::Post(original())), "{effects:?}");
}

#[test]
fn revert_to_equal_to_requested_is_ignored() {
    let mut h = H::new();
    h.step(start_reverting_to(requested(), requested()), 0);
    h.step(Input::ConfigRead(Ok(previous())), 0);
    assert_eq!(h.sa.record().unwrap().previous, previous());
}

#[test]
fn a_new_start_clears_revert_to_and_loss_ignored() {
    let mut h = H::new();
    h.enter_probation();
    h.policy_off(0);
    h.observe_range(false, 1, 180);
    assert_eq!(h.phase(), Phase::Committed);
    let id = h.sa.change_id().map(str::to_owned);
    h.step(start_reverting_to(fallback(), original()), 181);
    assert_ne!(h.sa.change_id().map(str::to_owned), id);
    assert!(!h.view(181).policy.unwrap().loss_ignored);
    h.step(Input::ConfigRead(Ok(previous())), 181);
    assert_eq!(h.sa.record().unwrap().previous, original());
    // The following change has no revert_to.
    h.step(Input::PostResult(Err(api_err())), 181);
    h.step(start_input(requested()), 182);
    h.step(Input::ConfigRead(Ok(previous())), 182);
    assert_eq!(h.sa.record().unwrap().previous, previous());
}
