//! The auto-scan against the mock router, through the real actor and safe apply, in paused time.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use router_core::actor::{
    spawn_actor, ActorConfig, ActorHandle, AppSnapshot, Command, SessionView,
};
use router_core::drivers::mock::{Fault, MockDriver, MockScenario};
use router_core::scan::{
    self, Applied, ChangeCtl, GiveUpAfter, ScanFile, ScanHost, ScanMode, ScanOutcome, ScanSpeed,
    ScanStatus,
};
use router_core::{
    AutomationConfig, Credentials, LteBandSet, NetConfig, Phase, RevertReason, SafeApplyTimers,
};
use tokio::sync::watch;
use tokio::time::{sleep, Instant};

fn set(b: &[u8]) -> LteBandSet {
    LteBandSet::from_bands(b).unwrap()
}

struct Host {
    published: Mutex<Vec<ScanFile>>,
    /// Pressed when this many speed tests have run.
    stop_after: Option<(usize, watch::Sender<bool>)>,
    tests: Mutex<usize>,
    fail_tests: bool,
    fail_first: Mutex<usize>,
}

#[async_trait]
impl ScanHost for Host {
    async fn measure(&self) -> Result<ScanSpeed, String> {
        let mut n = self.tests.lock().unwrap();
        *n += 1;
        if let Some((at, tx)) = &self.stop_after {
            if *n >= *at {
                let _ = tx.send(true);
            }
        }
        if self.fail_tests {
            return Err("no internet".into());
        }
        let mut first = self.fail_first.lock().unwrap();
        if *first > 0 {
            *first -= 1;
            return Err("stopped during the upload".into());
        }
        Ok(ScanSpeed {
            download_mbps: Some(50.0 + *n as f64),
            upload_mbps: Some(5.0),
            idle_ping_ms: Some(40.0),
            ping_down_ms: Some(200.0),
            ping_up_ms: Some(300.0),
            bytes_used: 1000,
        })
    }

    fn publish(&self, file: &ScanFile) {
        self.published.lock().unwrap().push(file.clone());
    }

    fn now_ms(&self) -> u64 {
        1
    }
}

fn host() -> Host {
    Host {
        published: Mutex::new(Vec::new()),
        stop_after: None,
        tests: Mutex::new(0),
        fail_tests: false,
        fail_first: Mutex::new(0),
    }
}

struct Rig {
    h: ActorHandle,
    ctl: ChangeCtl,
    mock: Arc<MockDriver>,
    _dir: tempfile::TempDir,
}

async fn rig(automation: AutomationConfig) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockDriver::new(MockScenario::zain_like()));
    let h = spawn_actor(
        mock.clone(),
        ActorConfig {
            host: "mock".to_string(),
            timers: SafeApplyTimers::default(),
            data_dir: dir.path().to_path_buf(),
            cell_alerts: false,
            automation,
        },
    );
    h.commands
        .send(Command::Connect(Credentials::new("admin", "mock")))
        .await
        .unwrap();
    let mut ctl = ChangeCtl {
        commands: h.commands.clone(),
        snapshots: h.snapshots.clone(),
        alerts: h.alerts.subscribe(),
    };
    ctl.wait_for(Duration::from_secs(10), ready)
        .await
        .expect("connected");
    Rig {
        h,
        ctl,
        mock,
        _dir: dir,
    }
}

fn ready(s: &AppSnapshot) -> bool {
    s.session == SessionView::LoggedIn && s.config.is_some() && s.capabilities.is_some()
}

fn start_config(r: &Rig) -> NetConfig {
    r.ctl.snap().config.unwrap()
}

#[tokio::test(start_paused = true)]
async fn quick_scan_measures_each_config_and_ends_on_the_starting_bands() {
    let mut r = rig(AutomationConfig::default()).await;
    let start = start_config(&r);
    let mut file = ScanFile::new("mock", "mock", ScanMode::Quick, start.clone(), &[1, 3], 0);
    let h = host();
    let (_tx, stop) = watch::channel(false);
    scan::run(&mut r.ctl, &mut file, Duration::from_secs(90), &h, stop).await;

    assert_eq!(file.status, ScanStatus::Done);
    assert!(!file.return_failed);
    assert_eq!(file.results.len(), file.queue.len());
    assert!(file
        .results
        .iter()
        .all(|x| x.outcome == ScanOutcome::Measured));
    assert!(file.results[0].is_start, "the current set goes first");
    assert_eq!(r.ctl.snap().config.as_ref(), Some(&start));
    assert!(!scan::active(&r.ctl.snap().safe_apply.phase));
    // The record is written while the scan runs, not just at the end.
    assert!(h.published.lock().unwrap().len() > file.results.len());
}

#[tokio::test(start_paused = true)]
async fn a_custom_combination_is_tried_after_the_single_bands() {
    let mut r = rig(AutomationConfig::default()).await;
    let start = start_config(&r);
    let mut file = ScanFile::new("mock", "mock", ScanMode::Quick, start.clone(), &[1], 0);
    file.add_custom(&[set(&[1, 3])]);
    assert_eq!(file.custom_sets, vec![set(&[1, 3])]);
    let h = host();
    let (_tx, stop) = watch::channel(false);
    scan::run(&mut r.ctl, &mut file, Duration::from_secs(90), &h, stop).await;

    assert_eq!(file.status, ScanStatus::Done);
    let combo = file.results.last().unwrap();
    assert_eq!(combo.bands, set(&[1, 3]), "the combination comes last");
    assert_eq!(combo.outcome, ScanOutcome::Measured);
    assert!(!file.return_failed);
    assert_eq!(r.ctl.snap().config.as_ref(), Some(&start));
}

#[tokio::test(start_paused = true)]
async fn a_band_with_no_service_is_recorded_and_the_scan_moves_on() {
    let mut r = rig(AutomationConfig::default()).await;
    let start = start_config(&r);
    // The mock has no cell on B43.
    let mut file = ScanFile::new("mock", "mock", ScanMode::Quick, start.clone(), &[43, 1], 0);
    let (_tx, stop) = watch::channel(false);
    scan::run(
        &mut r.ctl,
        &mut file,
        Duration::from_secs(90),
        &host(),
        stop,
    )
    .await;

    let of = |b: &[u8]| file.results.iter().find(|x| x.bands == set(b)).unwrap();
    assert_eq!(of(&[43]).outcome, ScanOutcome::NoService);
    assert_eq!(of(&[43]).download_mbps, None);
    assert_eq!(of(&[1]).outcome, ScanOutcome::Measured);
    assert_eq!(file.status, ScanStatus::Done);
    assert_eq!(r.ctl.snap().config.as_ref(), Some(&start));
}

/// With auto-undo off nothing else would leave a dead band: the scan has to.
#[tokio::test(start_paused = true)]
async fn a_dead_band_is_left_even_with_auto_undo_off() {
    let mut r = rig(AutomationConfig {
        revert_on_loss: false,
        ..AutomationConfig::default()
    })
    .await;
    let start = start_config(&r);
    let mut file = ScanFile::new("mock", "mock", ScanMode::Quick, start.clone(), &[43], 0);
    let (_tx, stop) = watch::channel(false);
    scan::run(
        &mut r.ctl,
        &mut file,
        Duration::from_secs(90),
        &host(),
        stop,
    )
    .await;

    let dead = file.results.iter().find(|x| x.bands == set(&[43])).unwrap();
    assert_eq!(dead.outcome, ScanOutcome::NoService);
    assert_eq!(r.ctl.snap().config.as_ref(), Some(&start));
}

#[tokio::test(start_paused = true)]
async fn stop_returns_to_the_starting_bands_and_keeps_the_results() {
    let mut r = rig(AutomationConfig::default()).await;
    let start = start_config(&r);
    let mut file = ScanFile::new(
        "mock",
        "mock",
        ScanMode::Quick,
        start.clone(),
        &[1, 3, 28],
        0,
    );
    let (tx, stop) = watch::channel(false);
    let h = Host {
        stop_after: Some((2, tx)),
        ..host()
    };
    scan::run(&mut r.ctl, &mut file, Duration::from_secs(90), &h, stop).await;

    assert_eq!(file.status, ScanStatus::Stopped);
    assert_eq!(file.results.len(), 1, "the stopped test isn't counted");
    assert!(file.can_continue());
    assert_eq!(r.ctl.snap().config.as_ref(), Some(&start));
    assert!(r.ctl.snap().safe_apply.sequence.is_none());
}

#[tokio::test(start_paused = true)]
async fn thorough_adds_combinations_of_the_bands_that_worked() {
    let mut r = rig(AutomationConfig::default()).await;
    let start = start_config(&r);
    let mut file = ScanFile::new(
        "mock",
        "mock",
        ScanMode::Thorough,
        start.clone(),
        &[1, 3, 43],
        0,
    );
    let (_tx, stop) = watch::channel(false);
    scan::run(
        &mut r.ctl,
        &mut file,
        Duration::from_secs(90),
        &host(),
        stop,
    )
    .await;

    assert_eq!(file.status, ScanStatus::Done);
    // The starting set may hold anything; only the added combinations are chosen here.
    let tried: Vec<_> = file
        .results
        .iter()
        .filter(|x| !x.is_start)
        .map(|x| x.bands)
        .collect();
    assert!(tried.contains(&set(&[1, 3])));
    assert!(!tried.iter().any(|b| b.contains(43) && b.bands().len() > 1));
    assert_eq!(r.ctl.snap().config.as_ref(), Some(&start));
}

#[tokio::test(start_paused = true)]
async fn an_internet_drop_fails_the_test_only() {
    let mut r = rig(AutomationConfig::default()).await;
    let start = start_config(&r);
    let mut file = ScanFile::new("mock", "mock", ScanMode::Quick, start, &[1, 3], 0);
    let h = Host {
        fail_tests: true,
        ..host()
    };
    let (_tx, stop) = watch::channel(false);
    scan::run(&mut r.ctl, &mut file, Duration::from_secs(90), &h, stop).await;

    assert_eq!(file.status, ScanStatus::Done);
    assert!(file
        .results
        .iter()
        .all(|x| x.outcome == ScanOutcome::TestFailed));
    assert_eq!(file.results[0].note.as_deref(), Some("no internet"));
}

#[tokio::test(start_paused = true)]
async fn repeated_tests_are_averaged_and_keep_their_spread() {
    let mut r = rig(AutomationConfig::default()).await;
    let start = start_config(&r);
    let mut file = ScanFile::new("mock", "mock", ScanMode::Quick, start, &[], 0);
    file.repeats = 3;
    let h = host();
    let (_tx, stop) = watch::channel(false);
    scan::run(&mut r.ctl, &mut file, Duration::from_secs(90), &h, stop).await;

    let one = &file.results[0];
    assert_eq!(one.tests, 3);
    assert_eq!(one.download_mbps, Some(52.0));
    assert_eq!(one.download_min_mbps, Some(51.0));
    assert_eq!(one.download_max_mbps, Some(53.0));
    assert_eq!(one.bytes_used, 3000);
    assert_eq!(*h.tests.lock().unwrap(), 3);
}

#[tokio::test(start_paused = true)]
async fn a_band_with_no_cell_is_given_up_on_early_and_counts_as_no_service() {
    let mut r = rig(AutomationConfig::default()).await;
    let dead = NetConfig {
        lte_bands: set(&[43]),
        ..start_config(&r)
    };
    let limits = GiveUpAfter {
        reattach: Duration::from_secs(60),
        no_service: Duration::from_secs(30),
    };
    let t = Instant::now();
    let applied = r
        .ctl
        .apply_limited(&dead, Duration::from_secs(300), Some(limits), &mut || false)
        .await;
    assert!(
        matches!(
            applied,
            Applied::Reverted {
                reason: RevertReason::NoReattach,
                ..
            }
        ),
        "{applied:?}"
    );
    // 30 s of no service, then the revert's own 15 s reconnect on the mock.
    assert!(t.elapsed() < Duration::from_secs(50), "{:?}", t.elapsed());
}

/// The real router once refused a request right after a speed test. The change closed so fast the
/// scan never saw it, waited out its limit and called the band "no service".
#[tokio::test(start_paused = true)]
async fn a_refused_request_is_seen_at_once_and_tried_again() {
    let mut r = rig(AutomationConfig::default()).await;
    let start = start_config(&r);
    let wanted = NetConfig {
        lte_bands: set(&[1]),
        ..start.clone()
    };
    r.mock.set_fault(Some(Fault::RefuseOneApply));
    let t = Instant::now();
    let refused = r
        .ctl
        .apply(&wanted, Duration::from_secs(210), &mut || false)
        .await;
    assert!(
        matches!(
            refused,
            Applied::Reverted {
                reason: RevertReason::ApplyRejected,
                ..
            }
        ),
        "{refused:?}"
    );
    assert!(t.elapsed() < Duration::from_secs(30), "{:?}", t.elapsed());

    r.mock.set_fault(Some(Fault::RefuseOneApply));
    let mut file = ScanFile::new("mock", "mock", ScanMode::Quick, start, &[1], 0);
    let (_tx, stop) = watch::channel(false);
    scan::run(
        &mut r.ctl,
        &mut file,
        Duration::from_secs(90),
        &host(),
        stop,
    )
    .await;
    let b1 = file.results.iter().find(|x| x.bands == set(&[1])).unwrap();
    assert_eq!(b1.outcome, ScanOutcome::Measured);
}

#[tokio::test(start_paused = true)]
async fn a_failed_speed_test_gets_one_more_go() {
    let mut r = rig(AutomationConfig::default()).await;
    let start = start_config(&r);
    let mut file = ScanFile::new("mock", "mock", ScanMode::Quick, start, &[], 0);
    let h = Host {
        fail_first: Mutex::new(1),
        ..host()
    };
    let (_tx, stop) = watch::channel(false);
    scan::run(&mut r.ctl, &mut file, Duration::from_secs(90), &h, stop).await;
    assert_eq!(file.results[0].outcome, ScanOutcome::Measured);
    assert_eq!(file.results[0].tests, 1);
}

/// With the router gone the return would keep trying for many minutes, with the app locked.
#[tokio::test(start_paused = true)]
async fn stop_ends_the_return_to_the_starting_bands() {
    let mut r = rig(AutomationConfig::default()).await;
    let start = start_config(&r);
    let elsewhere = NetConfig {
        lte_bands: set(&[43]),
        ..start
    };
    r.mock.set_fault(Some(Fault::Unreachable));
    let (tx, stop) = watch::channel(false);
    let mut pressed = scan::stop_pressed_again(&stop);
    let switch = tx.clone();
    let press = tokio::spawn(async move {
        sleep(Duration::from_secs(20)).await;
        let _ = switch.send(true);
    });
    let t = Instant::now();
    let back = scan::return_to_start(
        &mut r.ctl,
        &elsewhere,
        Duration::from_secs(210),
        8,
        &mut pressed,
    )
    .await;
    press.await.unwrap();
    assert!(!back);
    assert!(t.elapsed() < Duration::from_secs(40), "{:?}", t.elapsed());
}

/// The automation that would fight a scan stays out of it, and comes back afterwards.
#[tokio::test(start_paused = true)]
async fn a_starred_band_does_not_turn_a_test_into_two_steps() {
    let mut r = rig(AutomationConfig {
        primary_band: Some(3),
        relock_primary: true,
        ..AutomationConfig::default()
    })
    .await;
    let start = start_config(&r);
    r.h.commands
        .send(Command::SetScanActive(true))
        .await
        .unwrap();
    let wanted = NetConfig {
        lte_bands: set(&[1, 3]),
        ..start
    };
    let (_tx, stop) = watch::channel(false);
    let mut never = {
        let stop = stop.clone();
        move || *stop.borrow()
    };
    let t = Instant::now();
    r.ctl
        .apply(&wanted, Duration::from_secs(300), &mut never)
        .await;
    assert_eq!(r.ctl.snap().safe_apply.phase, Phase::Probation);
    assert!(r.ctl.snap().safe_apply.sequence.is_none());
    assert_eq!(r.ctl.snap().config.as_ref(), Some(&wanted));
    assert!(t.elapsed() < Duration::from_secs(300));
    let _ = (&r.mock, sleep(Duration::from_millis(1)).await);
}
