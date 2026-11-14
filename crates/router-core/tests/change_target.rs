//! Safe-apply over a change target other than the band config: no fallback, no loss policy,
//! evidence from the WAN address instead of the serving band, and its own journal files.

use std::time::{Duration, Instant};

use router_core::drivers::mock::MockScenario;
use router_core::journal::{Journal, JournalPhase, JournalRecord};
use router_core::safe_apply::{
    ChangeTarget, Effect, Evidence, ForeignJournal, Input, Phase, SafeApply, SafeApplyEvent,
    SafeApplyTimers, ServingLte,
};
use router_core::{Capabilities, LteBandSet, NetConfig, NetworkMode, NrOption, RouterError};
use serde::{Deserialize, Serialize};

const HOST: &str = "192.168.8.1";
const DRIVER: &str = "huawei";

/// A stand-in for a profile switch: which profile is the default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Profile {
    index: u8,
}

impl ChangeTarget for Profile {
    const LOSS_POLICY_APPLIES: bool = false;

    fn validate(&self, _caps: &Capabilities) -> Result<(), String> {
        if self.index == 0 {
            Err("no such profile".to_string())
        } else {
            Ok(())
        }
    }

    fn evidence_ok(&self, obs: &Evidence) -> bool {
        obs.wan_ip_present != Some(false)
    }

    fn settle_key(_obs: &Evidence) -> Option<ServingLte> {
        None
    }

    fn fallback(_previous: &Self) -> Option<Self> {
        None
    }
}

fn caps() -> Capabilities {
    Capabilities {
        supported_lte: MockScenario::zain_like().supported,
        modes: vec![NetworkMode::Auto, NetworkMode::LteOnly],
        nr_options: vec![NrOption::Nsa],
        band_edit_allowed: true,
        factory_reset_verified: false,
        data_switch: false,
        ip_methods: Vec::new(),
        apn: Default::default(),
    }
}

fn timers() -> SafeApplyTimers {
    SafeApplyTimers {
        grace: Duration::from_secs(90),
        loss: Duration::from_secs(60),
        probation: Duration::from_secs(120),
        poll_interval: Duration::from_secs(1),
        settle: Duration::from_secs(10),
    }
}

fn old() -> Profile {
    Profile { index: 1 }
}

fn new() -> Profile {
    Profile { index: 2 }
}

struct H {
    sa: SafeApply<Profile>,
    t0: Instant,
}

impl H {
    fn new() -> Self {
        Self {
            sa: SafeApply::new(timers(), HOST.to_string(), DRIVER.to_string()),
            t0: Instant::now(),
        }
    }

    fn step(&mut self, input: Input<Profile>, s: u64) -> Vec<Effect<Profile>> {
        self.sa.step(input, self.t0 + Duration::from_secs(s))
    }

    /// An in-service poll at `s`, with or without a WAN address, on LTE cell `pci`.
    fn obs(&mut self, wan: Option<bool>, pci: u16, s: u64) -> Vec<Effect<Profile>> {
        let input = Input::Observation {
            in_service: true,
            polled_at: None,
            lte: Some((3, Some(pci))),
            wan_ip_present: wan,
        };
        self.step(input, s)
    }

    fn out_of_service(&mut self, from: u64, to: u64) -> Vec<Effect<Profile>> {
        let mut all = Vec::new();
        for s in from..=to {
            all.extend(self.step(
                Input::Observation {
                    in_service: false,
                    polled_at: None,
                    lte: None,
                    wan_ip_present: Some(false),
                },
                s,
            ));
        }
        all
    }

    fn phase(&self) -> Phase {
        self.sa.phase().clone()
    }

    fn enter_grace(&mut self) {
        self.step(
            Input::Start {
                requested: new(),
                caps: caps(),
                current: Some(old()),
                revert_to: None,
            },
            0,
        );
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
            self.obs(Some(true), 100, s);
        }
        assert_eq!(self.phase(), Phase::Probation);
    }
}

fn revert_failed() -> Vec<Effect<Profile>> {
    vec![
        Effect::WriteJournal(JournalPhase::RevertFailed),
        Effect::Event(SafeApplyEvent::RevertFailed),
        Effect::RetryAfter(Duration::from_secs(30)),
    ]
}

#[test]
fn the_target_validates_the_request() {
    let mut h = H::new();
    let effects = h.step(
        Input::Start {
            requested: Profile { index: 0 },
            caps: caps(),
            current: Some(old()),
            revert_to: None,
        },
        0,
    );
    assert_eq!(
        effects,
        vec![Effect::Event(SafeApplyEvent::Rejected {
            message: "no such profile".to_string()
        })]
    );
    assert_eq!(h.phase(), Phase::Idle);
}

#[test]
fn no_wan_address_is_not_evidence() {
    let mut h = H::new();
    h.enter_grace();
    for s in 0..=30 {
        h.obs(Some(false), 100, s);
    }
    assert_eq!(h.phase(), Phase::ReattachGrace);
    // Unknown counts, and a cell change doesn't restart the window for this target.
    for s in 31..=41 {
        h.obs(None, 100 + u16::try_from(s).unwrap(), s);
    }
    assert_eq!(h.phase(), Phase::Probation);
}

#[test]
fn the_loss_policy_does_not_apply() {
    let mut h = H::new();
    h.sa.set_revert_on_loss(false, h.t0);
    assert!(h.sa.revert_on_loss());
    assert!(h.sa.view(h.t0).policy.unwrap().revert_on_loss);
    h.enter_probation();
    let effects = h.out_of_service(11, 71);
    assert!(
        effects.contains(&Effect::Post(old())),
        "a loss always reverts: {effects:?}"
    );
    assert!(!effects.iter().any(|e| matches!(e, Effect::LossIgnored(_))));
}

#[test]
fn no_reattach_with_the_policy_off_still_reverts() {
    let mut h = H::new();
    h.sa.set_revert_on_loss(false, h.t0);
    h.enter_grace();
    let effects = h.out_of_service(1, 90);
    assert!(effects.contains(&Effect::Post(old())), "{effects:?}");
    assert_eq!(
        h.phase(),
        Phase::Reverting {
            attempt: 0,
            fallback: false
        }
    );
}

#[test]
fn without_a_fallback_three_failed_reverts_end_in_revert_failed() {
    let mut h = H::new();
    h.enter_probation();
    h.step(Input::RevertNow, 20);
    h.step(Input::PostResult(Err(RouterError::Timeout)), 20);
    assert_eq!(h.step(Input::Tick, 25), vec![Effect::Post(old())]);
    h.step(Input::PostResult(Err(RouterError::Timeout)), 25);
    assert_eq!(h.step(Input::Tick, 40), vec![Effect::Post(old())]);
    h.step(Input::PostResult(Err(RouterError::Timeout)), 40);
    let effects = h.step(Input::Tick, 70);
    assert_eq!(effects, revert_failed());
    assert!(!effects.contains(&Effect::Event(SafeApplyEvent::FellBackToAuto)));
    assert_eq!(h.phase(), Phase::RevertFailed);
    // Retried every 30 s while the app runs, to the old profile again.
    let retry = h.step(Input::Tick, 100);
    assert!(retry.contains(&Effect::Post(old())), "{retry:?}");
}

#[test]
fn a_restored_profile_without_service_is_revert_failed() {
    let mut h = H::new();
    h.enter_probation();
    h.step(Input::RevertNow, 20);
    h.step(Input::PostResult(Ok(())), 20);
    assert_eq!(h.step(Input::ConfigRead(Ok(old())), 21), vec![]);
    let mut all = h.out_of_service(22, 110);
    all.extend(h.step(Input::Tick, 111));
    assert!(!all.iter().any(|e| matches!(e, Effect::Post(_))), "{all:?}");
    assert_eq!(all, revert_failed());
    assert_eq!(h.phase(), Phase::RevertFailed);
}

#[test]
fn a_profile_change_commits_after_probation() {
    let mut h = H::new();
    h.enter_probation();
    let mut all = Vec::new();
    for s in 11..=130 {
        all.extend(h.obs(Some(true), 100, s));
    }
    assert!(all.contains(&Effect::Event(SafeApplyEvent::Committed)));
    assert_eq!(h.phase(), Phase::Committed);
    let rec = h.sa.record().unwrap();
    assert_eq!((rec.previous, rec.requested), (old(), new()));
}

#[test]
fn the_band_target_ignores_the_wan_address() {
    let mut sa: SafeApply = SafeApply::new(timers(), HOST.to_string(), DRIVER.to_string());
    let t0 = Instant::now();
    let at = |s: u64| t0 + Duration::from_secs(s);
    let previous = NetConfig {
        network_mode: NetworkMode::LteOnly,
        lte_bands: LteBandSet::from_bands(&[1, 3]).unwrap(),
        legacy_band_raw: "3FFFFFFF".to_string(),
        nr_option: Some(NrOption::Nsa),
    };
    let requested = NetConfig {
        lte_bands: LteBandSet::from_bands(&[3]).unwrap(),
        ..previous.clone()
    };
    sa.step(
        Input::Start {
            requested: requested.clone(),
            caps: caps(),
            current: Some(previous.clone()),
            revert_to: None,
        },
        at(0),
    );
    sa.step(Input::ConfigRead(Ok(previous)), at(0));
    sa.step(Input::PostResult(Ok(())), at(0));
    sa.step(Input::ConfigRead(Ok(requested)), at(0));
    for s in 0..=10 {
        sa.step(
            Input::Observation {
                in_service: true,
                polled_at: None,
                lte: Some((3, Some(100))),
                wan_ip_present: Some(false),
            },
            at(s),
        );
    }
    assert_eq!(*sa.phase(), Phase::Probation);
}

fn profile_record(phase: JournalPhase) -> JournalRecord<Profile> {
    JournalRecord {
        version: 1,
        id: "p-1".to_string(),
        driver: DRIVER.to_string(),
        host: HOST.to_string(),
        previous: old(),
        requested: new(),
        phase,
        phase_changed_at_ms: 2000,
        created_at_ms: 1000,
    }
}

#[test]
fn a_journal_with_its_own_files() {
    let dir = tempfile::tempdir().unwrap();
    let j = Journal::with_files(dir.path().to_path_buf(), "pending-profile.json", "profiles");
    assert_eq!(j.load_record::<Profile>().unwrap(), None);
    let rec = profile_record(JournalPhase::Probation);
    j.write_record(&rec).unwrap();
    assert!(dir.path().join("pending-profile.json").exists());
    assert!(!dir.path().join("pending-change.json").exists());
    // The band journal doesn't see it.
    assert_eq!(Journal::new(dir.path().to_path_buf()).load().unwrap(), None);
    assert_eq!(j.load_record::<Profile>().unwrap(), Some(rec.clone()));

    j.archive_record(&rec).unwrap();
    assert_eq!(j.load_record::<Profile>().unwrap(), None);
    assert!(dir.path().join("profiles").join("1000-p-1.json").exists());
    assert!(!dir.path().join("journal-archive").exists());
    assert_eq!(j.archived_record::<Profile>("p-1").unwrap(), Some(rec));
}

#[test]
fn a_profile_journal_resumes_and_a_foreign_one_is_kept() {
    let t0 = Instant::now();
    let (sa, effects) = SafeApply::resume(
        profile_record(JournalPhase::Applied),
        timers(),
        HOST,
        DRIVER,
        t0,
    )
    .unwrap();
    assert_eq!(effects, vec![Effect::ReadConfig]);
    assert_eq!(*sa.phase(), Phase::Verifying);

    let mut rec = profile_record(JournalPhase::Reverting);
    rec.host = "192.168.1.1".to_string();
    let Err(ForeignJournal(kept)) = SafeApply::resume(rec.clone(), timers(), HOST, DRIVER, t0)
    else {
        panic!("a journal for another host is foreign");
    };
    assert_eq!(kept, rec);
}

#[test]
fn a_resumed_revert_goes_back_to_the_old_profile() {
    let t0 = Instant::now();
    let (_, effects) = SafeApply::resume(
        profile_record(JournalPhase::Reverting),
        timers(),
        HOST,
        DRIVER,
        t0,
    )
    .unwrap();
    assert_eq!(
        effects,
        vec![
            Effect::WriteJournal(JournalPhase::Reverting),
            Effect::Post(old())
        ]
    );
}
