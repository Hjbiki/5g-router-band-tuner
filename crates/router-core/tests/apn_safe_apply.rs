//! Safe apply over the real APN target (`ApnTarget`): what it accepts, what counts as service,
//! the fixed timers, no fallback, no loss policy, and resuming from every journal phase.

use std::time::{Duration, Instant};

use router_core::apn::{check_target, APN_TIMERS};
use router_core::drivers::mock::{mock_profile, MockScenario};
use router_core::journal::{JournalPhase, JournalRecord, JOURNAL_VERSION};
use router_core::safe_apply::{Effect, Input, Phase, RevertReason, SafeApply, SafeApplyEvent};
use router_core::{
    ApnProfiles, ApnSupport, ApnTarget, Capabilities, NetworkMode, NrOption, RouterError,
    APN_PROFILE_CHANGED, APN_PROFILE_INVALID, APN_PROFILE_MISSING, APN_SWITCH_NOT_SUPPORTED,
    APN_TARGET_INVALID,
};

const HOST: &str = "192.168.8.1";
const DRIVER: &str = "huawei_h122";

fn caps(switch: bool) -> Capabilities {
    Capabilities {
        supported_lte: MockScenario::zain_like().supported,
        modes: vec![NetworkMode::Auto],
        nr_options: vec![NrOption::Nsa],
        band_edit_allowed: true,
        factory_reset_verified: false,
        data_switch: true,
        ip_methods: Vec::new(),
        apn: ApnSupport {
            read: true,
            switch,
            ..ApnSupport::default()
        },
    }
}

fn profiles() -> ApnProfiles {
    ApnProfiles {
        current: Some(1),
        profiles: vec![
            mock_profile(1, "Operator internet", Some("internet.example"), true),
            mock_profile(2, "Work", Some("work.example"), false),
        ],
        auto_apn: Some(false),
    }
}

fn target(index: u32) -> ApnTarget {
    let p = profiles();
    ApnTarget {
        index,
        fingerprint: p.get(index).unwrap().fingerprint.clone(),
    }
}

fn old() -> ApnTarget {
    target(1)
}

fn new() -> ApnTarget {
    target(2)
}

struct H {
    sa: SafeApply<ApnTarget>,
    t0: Instant,
}

impl H {
    fn new() -> Self {
        Self {
            sa: SafeApply::new(APN_TIMERS, HOST.to_string(), DRIVER.to_string()),
            t0: Instant::now(),
        }
    }

    fn step(&mut self, input: Input<ApnTarget>, s: u64) -> Vec<Effect<ApnTarget>> {
        self.sa.step(input, self.t0 + Duration::from_secs(s))
    }

    fn obs(
        &mut self,
        in_service: bool,
        wan: Option<bool>,
        pci: u16,
        s: u64,
    ) -> Vec<Effect<ApnTarget>> {
        self.step(
            Input::Observation {
                in_service,
                polled_at: None,
                lte: in_service.then_some((3, Some(pci))),
                wan_ip_present: wan,
            },
            s,
        )
    }

    fn phase(&self) -> Phase {
        self.sa.phase().clone()
    }

    fn start(&mut self, requested: ApnTarget, caps: Capabilities) -> Vec<Effect<ApnTarget>> {
        self.step(
            Input::Start {
                requested,
                caps,
                current: Some(old()),
                revert_to: None,
            },
            0,
        )
    }

    fn enter_grace(&mut self) {
        assert_eq!(self.start(new(), caps(true)), vec![Effect::ReadConfig]);
        assert_eq!(
            self.step(Input::ConfigRead(Ok(old())), 0),
            vec![
                Effect::WriteJournal(JournalPhase::Journaled),
                Effect::Post(new())
            ]
        );
        self.step(Input::PostResult(Ok(())), 0);
        self.step(Input::ConfigRead(Ok(new())), 0);
        assert_eq!(self.phase(), Phase::ReattachGrace);
    }

    fn enter_probation(&mut self) {
        self.enter_grace();
        for s in 0..=10 {
            self.obs(true, Some(true), 100, s);
        }
        assert_eq!(self.phase(), Phase::Probation);
    }
}

fn rejected(effects: &[Effect<ApnTarget>]) -> Option<String> {
    effects.iter().find_map(|e| match e {
        Effect::Event(SafeApplyEvent::Rejected { message }) => Some(message.clone()),
        _ => None,
    })
}

#[test]
fn the_timers_are_fixed() {
    assert_eq!(APN_TIMERS.grace, Duration::from_secs(90));
    assert_eq!(APN_TIMERS.settle, Duration::from_secs(10));
    assert_eq!(APN_TIMERS.loss, Duration::from_secs(60));
    assert_eq!(APN_TIMERS.probation, Duration::from_secs(120));
}

#[test]
fn a_switch_needs_the_capability_and_a_well_formed_target() {
    let mut h = H::new();
    let e = h.start(new(), caps(false));
    assert_eq!(rejected(&e).as_deref(), Some(APN_SWITCH_NOT_SUPPORTED));
    for bad in [
        ApnTarget {
            index: 0,
            fingerprint: new().fingerprint,
        },
        ApnTarget {
            index: 2,
            fingerprint: "not-a-fingerprint".into(),
        },
        ApnTarget {
            index: 2,
            fingerprint: "ABCDEF0123456789".into(),
        },
    ] {
        let e = h.start(bad, caps(true));
        assert_eq!(rejected(&e).as_deref(), Some(APN_TARGET_INVALID));
    }
    // The profile already in use.
    let e = h.start(old(), caps(true));
    assert!(rejected(&e).is_some());
    assert_eq!(h.phase(), Phase::Idle);
}

#[test]
fn the_profile_list_decides_what_may_be_switched_to() {
    let p = profiles();
    assert_eq!(check_target(&p, &new()), Ok(()));
    let msg = |t: &ApnTarget| match check_target(&p, t) {
        Err(RouterError::Validation { message }) => message,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        msg(&ApnTarget {
            index: 9,
            fingerprint: new().fingerprint
        }),
        APN_PROFILE_MISSING
    );
    assert_eq!(
        msg(&ApnTarget {
            index: 2,
            fingerprint: old().fingerprint
        }),
        APN_PROFILE_CHANGED
    );
    let mut broken = profiles();
    broken.profiles[1].valid = false;
    assert_eq!(
        check_target(&broken, &new()),
        Err(RouterError::Validation {
            message: APN_PROFILE_INVALID.into()
        })
    );
}

#[test]
fn a_read_back_with_another_fingerprint_is_a_mismatch() {
    let mut h = H::new();
    h.start(new(), caps(true));
    h.step(Input::ConfigRead(Ok(old())), 0);
    h.step(Input::PostResult(Ok(())), 0);
    // Same index, but the profile was edited under us: not what was asked for.
    let edited = ApnTarget {
        index: 2,
        fingerprint: "0000000000000000".into(),
    };
    let mut all = Vec::new();
    for s in 0..=25 {
        let e = if s % 5 == 0 {
            h.step(Input::ConfigRead(Ok(edited.clone())), s)
        } else {
            h.step(Input::Tick, s)
        };
        all.extend(e);
    }
    assert!(all.contains(&Effect::Post(old())), "{all:?}");
}

#[test]
fn service_without_an_internet_address_is_not_evidence() {
    let mut h = H::new();
    h.enter_grace();
    for s in 0..=40 {
        h.obs(true, Some(false), 100, s);
    }
    assert_eq!(h.phase(), Phase::ReattachGrace);
    // A cell change doesn't restart the window for an APN; an unknown address counts.
    for s in 41..=51 {
        h.obs(true, None, 100 + u16::try_from(s).unwrap(), s);
    }
    assert_eq!(h.phase(), Phase::Probation);
}

#[test]
fn a_profile_without_data_is_undone_whatever_the_loss_policy() {
    let mut h = H::new();
    h.sa.set_revert_on_loss(false, h.t0);
    assert!(h.sa.revert_on_loss(), "always on for an APN");
    h.enter_grace();
    let mut all = Vec::new();
    for s in 1..=90 {
        all.extend(h.obs(false, Some(false), 0, s));
    }
    assert!(all.contains(&Effect::Post(old())), "{all:?}");
    assert!(!all.iter().any(|e| matches!(e, Effect::LossIgnored(_))));
    h.step(Input::PostResult(Ok(())), 90);
    h.step(Input::ConfigRead(Ok(old())), 90);
    let done = h.obs(true, Some(true), 100, 91);
    assert!(done.contains(&Effect::Event(SafeApplyEvent::Reverted {
        reason: RevertReason::NoReattach,
        fallback: false
    })));
}

#[test]
fn a_loss_in_probation_reverts() {
    let mut h = H::new();
    h.sa.set_revert_on_loss(false, h.t0);
    h.enter_probation();
    let mut all = Vec::new();
    for s in 11..=72 {
        all.extend(h.obs(false, Some(false), 0, s));
    }
    assert!(all.contains(&Effect::Post(old())), "{all:?}");
}

#[test]
fn it_commits_after_two_minutes_of_service() {
    let mut h = H::new();
    h.enter_probation();
    let mut all = Vec::new();
    for s in 11..=131 {
        all.extend(h.obs(true, Some(true), 100, s));
    }
    assert!(all.contains(&Effect::Event(SafeApplyEvent::Committed)));
    assert_eq!(h.phase(), Phase::Committed);
}

#[test]
fn no_fallback_ends_in_revert_failed_and_keeps_retrying_the_old_profile() {
    let mut h = H::new();
    h.enter_probation();
    h.step(Input::RevertNow, 20);
    let refused = || {
        Input::PostResult(Err(RouterError::Validation {
            message: APN_PROFILE_CHANGED.into(),
        }))
    };
    h.step(refused(), 20);
    assert_eq!(h.step(Input::Tick, 25), vec![Effect::Post(old())]);
    h.step(refused(), 25);
    assert_eq!(h.step(Input::Tick, 40), vec![Effect::Post(old())]);
    h.step(refused(), 40);
    let effects = h.step(Input::Tick, 70);
    assert_eq!(
        effects,
        vec![
            Effect::WriteJournal(JournalPhase::RevertFailed),
            Effect::Event(SafeApplyEvent::RevertFailed),
            Effect::RetryAfter(Duration::from_secs(30)),
        ]
    );
    assert_eq!(h.phase(), Phase::RevertFailed);
    let retry = h.step(Input::Tick, 100);
    assert!(retry.contains(&Effect::Post(old())), "{retry:?}");
    assert!(!retry.iter().any(|e| *e == Effect::Post(new())));
}

fn record(phase: JournalPhase) -> JournalRecord<ApnTarget> {
    JournalRecord {
        version: JOURNAL_VERSION,
        id: "a-1".into(),
        driver: DRIVER.into(),
        host: HOST.into(),
        previous: old(),
        requested: new(),
        phase,
        phase_changed_at_ms: 2,
        created_at_ms: 1,
    }
}

#[test]
fn resumes_from_every_phase() {
    use JournalPhase::*;
    let t0 = Instant::now();
    for phase in [
        Journaled,
        Applied,
        Verified,
        ReattachGrace,
        Probation,
        Reverting,
        RevertFailed,
        Committed,
        Reverted,
    ] {
        let (sa, effects) = SafeApply::resume(record(phase), APN_TIMERS, HOST, DRIVER, t0).unwrap();
        let expected: Vec<Effect<ApnTarget>> = match phase {
            Journaled | Applied => vec![Effect::ReadConfig],
            Verified | ReattachGrace | Probation => {
                vec![Effect::WriteJournal(JournalPhase::ReattachGrace)]
            }
            Reverting | RevertFailed => vec![
                Effect::WriteJournal(JournalPhase::Reverting),
                Effect::Post(old()),
            ],
            Committed | Reverted => vec![Effect::ArchiveJournal],
        };
        assert_eq!(effects, expected, "{phase:?}");
        let want = match phase {
            Journaled | Applied => Phase::Verifying,
            Verified | ReattachGrace | Probation => Phase::ReattachGrace,
            Reverting | RevertFailed => Phase::Reverting {
                attempt: 0,
                fallback: false,
            },
            Committed | Reverted => Phase::Idle,
        };
        assert_eq!(*sa.phase(), want, "{phase:?}");
    }
    // A journal for another router is never resumed.
    let mut foreign = record(JournalPhase::Probation);
    foreign.driver = "mock".into();
    assert!(SafeApply::resume(foreign, APN_TIMERS, HOST, DRIVER, t0).is_err());
}

#[test]
fn a_resumed_switch_that_never_got_service_goes_back() {
    let t0 = Instant::now();
    let (mut sa, _) = SafeApply::resume(
        record(JournalPhase::Probation),
        APN_TIMERS,
        HOST,
        DRIVER,
        t0,
    )
    .unwrap();
    let mut all = Vec::new();
    for s in 1..=91 {
        all.extend(sa.step(
            Input::Observation {
                in_service: false,
                polled_at: None,
                lte: None,
                wan_ip_present: Some(false),
            },
            t0 + Duration::from_secs(s),
        ));
    }
    assert!(all.contains(&Effect::Post(old())), "{all:?}");
}

#[test]
fn the_journal_holds_index_and_fingerprint_only() {
    let json = serde_json::to_string(&record(JournalPhase::Probation)).unwrap();
    assert!(
        !json.contains("work.example") && !json.contains("Work"),
        "{json}"
    );
    assert!(json.contains("\"index\":2"), "{json}");
}
