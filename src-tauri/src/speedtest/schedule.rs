//! Automatic speed tests, for the History page. Off by default.
//!
//! Every `every_h` hours (1 to 168) a test runs with the provider and size from
//! `settings.history.autoSpeedTest`. The first one comes at least 3 minutes after launch, or after
//! the user turns it on or changes the interval, and each slot gets 0 to 120 s of jitter. A test
//! never runs while the router is offline, a band change or auto-scan is open, or another test
//! runs. A slot that stays blocked for 10 minutes is skipped and logged; the next one comes a
//! full interval later. Slots missed while the PC slept are never caught up: after a sleep at
//! most one test runs.
//!
//! The test goes through the same `begin()` guard as the user's, so the actor holds back Apply,
//! step 2 and the re-lock watchdog while it runs. There's no notification; the tray line says
//! that an automatic test is running.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use router_core::event_text::kinds;
use router_core::{AppSnapshot, SessionView};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use ts_rs::TS;

use super::{band_change_open, SpeedTestState};
use crate::commands::AppState;
use crate::settings::AutoSpeedTestSettings;

pub const EVENT: &str = "auto-speedtest";
pub const STATE_FILE: &str = "auto-speedtest.json";
const HOUR_MS: u64 = 3_600_000;
/// The first test comes at least this long after launch, or after a settings change.
pub const FIRST_DELAY_MS: u64 = 180_000;
/// Each slot moves by up to this much.
pub const JITTER_MAX_MS: u64 = 120_000;
/// How long a blocked slot waits before it's skipped.
pub const GRACE_MS: u64 = 10 * 60_000;
const TICK: Duration = Duration::from_secs(30);

/// Why a slot was skipped, or what holds it back now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SkipReason {
    /// Not logged in to the router, or no service.
    Offline,
    /// The user logged out of the router.
    LoggedOut,
    BandChange,
    AutoScan,
    /// Another speed test is running.
    SpeedTest,
    /// Refresh IP is turning mobile data off and on.
    IpRefresh,
    /// An APN switch is being watched.
    ApnChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum AutoOutcome {
    Done,
    Failed,
    Cancelled,
    Skipped,
}

/// `auto-speedtest.json`: what happened last, kept across restarts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StateFile {
    pub last_attempt_ms: Option<u64>,
    pub last_outcome: Option<AutoOutcome>,
    pub last_skip_reason: Option<SkipReason>,
}

pub fn state_path(data_dir: &Path) -> PathBuf {
    data_dir.join(STATE_FILE)
}

impl StateFile {
    /// Missing or unreadable: as if no test had run.
    pub fn load(data_dir: &Path) -> Self {
        std::fs::read_to_string(state_path(data_dir))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Written to a temporary file and renamed, so a crash never leaves half a file.
    pub fn save(&self, data_dir: &Path) -> std::io::Result<()> {
        let path = state_path(data_dir);
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &path)
    }
}

/// What can hold a slot back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Blockers {
    /// The user logged out; shown instead of `offline`.
    pub logged_out: bool,
    pub offline: bool,
    pub band_change: bool,
    pub ip_refresh: bool,
    pub apn_change: bool,
    pub scan: bool,
    pub speed_test: bool,
}

impl Blockers {
    pub fn from_snapshot(s: &AppSnapshot, scan: bool, speed_test: bool) -> Self {
        Self {
            logged_out: matches!(s.session, SessionView::LoggedOut),
            offline: !matches!(s.session, SessionView::LoggedIn) || !s.in_service,
            band_change: band_change_open(&s.safe_apply),
            ip_refresh: super::ip_refresh_active(s),
            apn_change: super::apn_change_open(s),
            scan,
            speed_test,
        }
    }

    /// The first reason, in the order the status shows them.
    pub fn first(&self) -> Option<SkipReason> {
        [
            (self.logged_out, SkipReason::LoggedOut),
            (self.offline, SkipReason::Offline),
            (self.band_change, SkipReason::BandChange),
            (self.ip_refresh, SkipReason::IpRefresh),
            (self.apn_change, SkipReason::ApnChange),
            (self.scan, SkipReason::AutoScan),
            (self.speed_test, SkipReason::SpeedTest),
        ]
        .into_iter()
        .find_map(|(on, r)| on.then_some(r))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Idle,
    /// Nothing to do before this time.
    WaitUntil(u64),
    Run,
    /// Skip the slot; the `u64` is the time recorded as the attempt.
    Skip(SkipReason, u64),
}

/// The next slot: a full interval after the last attempt, never sooner than 3 minutes after
/// `anchor_ms` (launch, or the settings change), plus the jitter.
pub fn next_slot(
    cfg: &AutoSpeedTestSettings,
    last_attempt_ms: Option<u64>,
    anchor_ms: u64,
    jitter_ms: u64,
) -> u64 {
    let after_last = last_attempt_ms.map_or(0, |l| l + u64::from(cfg.every_h) * HOUR_MS);
    after_last.max(anchor_ms + FIRST_DELAY_MS) + jitter_ms.min(JITTER_MAX_MS)
}

/// What to do at `now` for a slot that has been due since `due_since_ms`.
pub fn decide(now_ms: u64, slot_ms: u64, due_since_ms: u64, b: &Blockers) -> Decision {
    if now_ms < slot_ms {
        return Decision::WaitUntil(slot_ms);
    }
    match b.first() {
        None => Decision::Run,
        Some(r) if now_ms >= due_since_ms + GRACE_MS => Decision::Skip(r, due_since_ms),
        Some(_) => Decision::WaitUntil(due_since_ms + GRACE_MS),
    }
}

/// The scheduler's memory between ticks. Pure: the loop feeds it the time, the settings, the
/// last attempt and the blockers.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    slot_ms: Option<u64>,
    /// When the slot was first seen due. After a sleep that's later than the slot, so the grace
    /// window starts when the PC is back.
    due_since_ms: Option<u64>,
    /// The interval the slot was computed for; `None` while off.
    every_h: Option<u16>,
    /// The slot just handed out as `Run`, so a lost `begin()` race can give it back.
    taken: Option<(u64, u64)>,
}

impl Plan {
    pub fn slot_ms(&self) -> Option<u64> {
        self.slot_ms
    }

    pub fn step(
        &mut self,
        now_ms: u64,
        cfg: &AutoSpeedTestSettings,
        last_attempt_ms: Option<u64>,
        blockers: &Blockers,
        jitter_ms: u64,
    ) -> Decision {
        if !cfg.enabled {
            *self = Self::default();
            return Decision::Idle;
        }
        if self.every_h != Some(cfg.every_h) {
            // Launch, turned on, or a new interval: count from now.
            self.every_h = Some(cfg.every_h);
            self.slot_ms = Some(next_slot(cfg, last_attempt_ms, now_ms, jitter_ms));
            self.due_since_ms = None;
        }
        let slot = *self
            .slot_ms
            .get_or_insert_with(|| next_slot(cfg, last_attempt_ms, 0, jitter_ms));
        if now_ms >= slot && self.due_since_ms.is_none() {
            self.due_since_ms = Some(now_ms);
        }
        let d = decide(now_ms, slot, self.due_since_ms.unwrap_or(now_ms), blockers);
        if matches!(d, Decision::Run | Decision::Skip(..)) {
            self.taken =
                matches!(d, Decision::Run).then(|| (slot, self.due_since_ms.unwrap_or(now_ms)));
            self.slot_ms = None;
            self.due_since_ms = None;
        }
        d
    }

    /// `begin()` was refused (another test won the race): the slot is still open, held back by
    /// that test like any other blocker.
    pub fn give_back(&mut self) {
        if let Some((slot, due)) = self.taken.take() {
            self.slot_ms = Some(slot);
            self.due_since_ms = Some(due);
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AutoSpeedTestStatus {
    pub enabled: bool,
    #[ts(type = "number | null")]
    pub next_slot_ms: Option<u64>,
    #[ts(type = "number | null")]
    pub last_attempt_ms: Option<u64>,
    pub last_outcome: Option<AutoOutcome>,
    pub last_skip_reason: Option<SkipReason>,
    /// What holds the next test back now.
    pub blocked_by: Option<SkipReason>,
    /// An automatic test is running now.
    pub running: bool,
}

/// Managed state: the status the command returns, and whether an automatic test is running.
#[derive(Default)]
pub struct AutoTestState {
    status: Mutex<AutoSpeedTestStatus>,
    running: AtomicBool,
}

impl AutoTestState {
    pub fn status(&self) -> AutoSpeedTestStatus {
        *self.status.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// `true` when the status changed.
    fn set(&self, s: AutoSpeedTestStatus) -> bool {
        let mut g = self.status.lock().unwrap_or_else(|p| p.into_inner());
        let changed = *g != s;
        *g = s;
        changed
    }
}

fn now_ms() -> u64 {
    crate::notify::now_ms()
}

fn jitter() -> u64 {
    rand::random_range(0..=JITTER_MAX_MS)
}

fn status_of(
    cfg: &AutoSpeedTestSettings,
    plan: &Plan,
    file: &StateFile,
    blockers: &Blockers,
    running: bool,
) -> AutoSpeedTestStatus {
    AutoSpeedTestStatus {
        enabled: cfg.enabled,
        next_slot_ms: if cfg.enabled { plan.slot_ms() } else { None },
        last_attempt_ms: file.last_attempt_ms,
        last_outcome: file.last_outcome,
        last_skip_reason: file.last_skip_reason,
        blocked_by: if cfg.enabled && !running {
            blockers.first()
        } else {
            None
        },
        running,
    }
}

fn publish(app: &AppHandle, s: AutoSpeedTestStatus) {
    if let Some(st) = app.try_state::<AutoTestState>() {
        if st.set(s) {
            let _ = app.emit(EVENT, s);
        }
    }
}

fn refresh_tray(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        crate::tray::on_snapshot(app, &state.snapshot());
    }
}

fn blockers_now(app: &AppHandle, state: &AppState) -> Blockers {
    let scan = app
        .try_state::<crate::autoscan::ScanState>()
        .is_some_and(|s| s.is_running());
    let test = app
        .try_state::<SpeedTestState>()
        .is_some_and(|s| s.is_running());
    Blockers::from_snapshot(&state.snapshot(), scan, test)
}

/// Starts the 30-second tick. Reads the settings on every tick, so a change applies within 30 s.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(dir) = app
            .try_state::<AppState>()
            .map(|s| s.data_dir().to_path_buf())
        else {
            return;
        };
        let mut file = StateFile::load(&dir);
        let mut plan = Plan::default();
        while let Some(state) = app.try_state::<AppState>() {
            let cfg = state.history_settings().auto_speed_test;
            let blockers = blockers_now(&app, &state);
            let now = now_ms();
            let decision = plan.step(now, &cfg, file.last_attempt_ms, &blockers, jitter());
            match decision {
                Decision::Run => {
                    run_one(&app, &dir, &mut file, &mut plan, cfg, now).await;
                    continue;
                }
                Decision::Skip(reason, at) => {
                    file = StateFile {
                        last_attempt_ms: Some(at),
                        last_outcome: Some(AutoOutcome::Skipped),
                        last_skip_reason: Some(reason),
                    };
                    let (d, f) = (dir.clone(), file);
                    let _ = tokio::task::spawn_blocking(move || {
                        let _ = f.save(&d);
                        crate::history::log_event(
                            &d,
                            kinds::SPEEDTEST_AUTO_SKIPPED,
                            serde_json::json!({ "reason": reason, "slotMs": at }),
                        );
                    })
                    .await;
                    continue;
                }
                Decision::Idle | Decision::WaitUntil(_) => {}
            }
            publish(&app, status_of(&cfg, &plan, &file, &blockers, false));
            tokio::time::sleep(TICK).await;
        }
    });
}

async fn run_one(
    app: &AppHandle,
    dir: &Path,
    file: &mut StateFile,
    plan: &mut Plan,
    cfg: AutoSpeedTestSettings,
    now: u64,
) {
    let Some(st) = app.try_state::<AutoTestState>() else {
        return;
    };
    st.running.store(true, Ordering::Relaxed);
    let outcome = super::run_auto(app, cfg).await;
    st.running.store(false, Ordering::Relaxed);
    let Some(result) = outcome else {
        // Another test claimed the slot first: wait for it like any other blocker.
        plan.give_back();
        refresh_tray(app);
        tokio::time::sleep(TICK).await;
        return;
    };
    *file = StateFile {
        last_attempt_ms: Some(now),
        last_outcome: Some(match result {
            Ok(_) => AutoOutcome::Done,
            Err(ref e) if e == "cancelled" => AutoOutcome::Cancelled,
            Err(_) => AutoOutcome::Failed,
        }),
        last_skip_reason: None,
    };
    let (d, f) = (dir.to_path_buf(), *file);
    let _ = tokio::task::spawn_blocking(move || f.save(&d)).await;
    refresh_tray(app);
}

/// Marks the tray while an automatic test runs; called by `run_auto` once the test has begun.
pub(crate) fn on_started(app: &AppHandle) {
    if let (Some(st), Some(state)) = (
        app.try_state::<AutoTestState>(),
        app.try_state::<AppState>(),
    ) {
        let cfg = state.history_settings().auto_speed_test;
        let mut s = st.status();
        s.enabled = cfg.enabled;
        s.running = true;
        s.blocked_by = None;
        publish(app, s);
    }
    refresh_tray(app);
}

#[tauri::command]
pub fn auto_speed_test_status(st: State<'_, AutoTestState>) -> AutoSpeedTestStatus {
    st.status()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{SpeedTestProvider, SpeedTestSize};

    const T0: u64 = 1_793_491_200_000;
    const MIN: u64 = 60_000;

    fn cfg(enabled: bool, every_h: u16) -> AutoSpeedTestSettings {
        AutoSpeedTestSettings {
            enabled,
            every_h,
            provider: SpeedTestProvider::Cloudflare,
            size: SpeedTestSize::Light,
        }
    }

    fn free() -> Blockers {
        Blockers::default()
    }

    fn offline() -> Blockers {
        Blockers {
            offline: true,
            ..Blockers::default()
        }
    }

    #[test]
    fn disabled_is_idle() {
        let mut p = Plan::default();
        assert_eq!(p.step(T0, &cfg(false, 3), None, &free(), 0), Decision::Idle);
        assert_eq!(p.slot_ms(), None);
    }

    #[test]
    fn the_first_slot_is_at_least_3_minutes_after_launch() {
        assert_eq!(next_slot(&cfg(true, 3), None, T0, 0), T0 + 3 * MIN);
        // A recent test elsewhere in the past still waits a full interval.
        assert_eq!(
            next_slot(&cfg(true, 3), Some(T0 - HOUR_MS), T0, 0),
            T0 + 2 * HOUR_MS
        );
        // An old one doesn't make it sooner than 3 minutes.
        assert_eq!(
            next_slot(&cfg(true, 3), Some(T0 - 9 * HOUR_MS), T0, 0),
            T0 + 3 * MIN
        );
        let mut p = Plan::default();
        assert_eq!(
            p.step(T0, &cfg(true, 3), None, &free(), 0),
            Decision::WaitUntil(T0 + 3 * MIN)
        );
    }

    #[test]
    fn jitter_stays_within_0_to_120_s() {
        for _ in 0..1000 {
            let j = jitter();
            assert!(j <= JITTER_MAX_MS);
        }
        assert_eq!(
            next_slot(&cfg(true, 1), None, T0, 10 * JITTER_MAX_MS),
            T0 + 3 * MIN + JITTER_MAX_MS
        );
    }

    #[test]
    fn it_waits_before_the_slot_and_runs_at_it() {
        let mut p = Plan::default();
        let c = cfg(true, 3);
        p.step(T0, &c, None, &free(), 0);
        assert_eq!(
            p.step(T0 + MIN, &c, None, &free(), 0),
            Decision::WaitUntil(T0 + 3 * MIN)
        );
        assert_eq!(p.step(T0 + 3 * MIN, &c, None, &free(), 0), Decision::Run);
        // The next slot is a full interval after the attempt.
        let last = Some(T0 + 3 * MIN);
        assert_eq!(
            p.step(T0 + 4 * MIN, &c, last, &free(), 0),
            Decision::WaitUntil(T0 + 3 * MIN + 3 * HOUR_MS)
        );
    }

    #[test]
    fn blocked_within_the_grace_window_it_waits_then_runs() {
        let mut p = Plan::default();
        let c = cfg(true, 3);
        p.step(T0, &c, None, &free(), 0);
        let due = T0 + 3 * MIN;
        let band = Blockers {
            band_change: true,
            ..Blockers::default()
        };
        assert_eq!(
            p.step(due, &c, None, &band, 0),
            Decision::WaitUntil(due + GRACE_MS)
        );
        assert_eq!(
            p.step(due + 9 * MIN, &c, None, &band, 0),
            Decision::WaitUntil(due + GRACE_MS)
        );
        assert_eq!(
            p.step(due + 9 * MIN + 30_000, &c, None, &free(), 0),
            Decision::Run
        );
    }

    #[test]
    fn blocked_past_the_grace_window_it_skips_and_waits_a_full_interval() {
        let mut p = Plan::default();
        let c = cfg(true, 2);
        p.step(T0, &c, None, &free(), 0);
        let due = T0 + 3 * MIN;
        assert!(matches!(
            p.step(due, &c, None, &offline(), 0),
            Decision::WaitUntil(_)
        ));
        assert_eq!(
            p.step(due + GRACE_MS, &c, None, &offline(), 0),
            Decision::Skip(SkipReason::Offline, due)
        );
        // The skip is recorded as the attempt; the next slot is 2 h after the skipped one.
        assert_eq!(
            p.step(due + GRACE_MS + 30_000, &c, Some(due), &free(), 0),
            Decision::WaitUntil(due + 2 * HOUR_MS)
        );
    }

    #[test]
    fn a_two_day_sleep_gives_exactly_one_test() {
        let mut p = Plan::default();
        let c = cfg(true, 1);
        p.step(T0, &c, None, &free(), 0);
        assert_eq!(p.step(T0 + 3 * MIN, &c, None, &free(), 0), Decision::Run);
        let mut last = Some(T0 + 3 * MIN);
        // Asleep for two days, then 30-second ticks for an hour.
        let wake = T0 + 2 * 24 * HOUR_MS;
        let mut runs = 0;
        let mut skips = 0;
        for i in 0..120 {
            let now = wake + i * 30_000;
            match p.step(now, &c, last, &free(), 0) {
                Decision::Run => {
                    runs += 1;
                    last = Some(now);
                }
                Decision::Skip(..) => skips += 1,
                _ => {}
            }
        }
        assert_eq!((runs, skips), (1, 0));
    }

    #[test]
    fn after_a_sleep_the_grace_window_starts_at_wake() {
        let mut p = Plan::default();
        let c = cfg(true, 1);
        p.step(T0, &c, Some(T0 - 30 * MIN), &free(), 0);
        let wake = T0 + 5 * HOUR_MS;
        // The router is still coming back: it waits rather than skipping straight away.
        assert_eq!(
            p.step(wake, &c, Some(T0 - 30 * MIN), &offline(), 0),
            Decision::WaitUntil(wake + GRACE_MS)
        );
        assert_eq!(
            p.step(wake + 2 * MIN, &c, Some(T0 - 30 * MIN), &free(), 0),
            Decision::Run
        );
    }

    #[test]
    fn an_interval_change_recomputes_the_slot() {
        let mut p = Plan::default();
        let last = Some(T0);
        p.step(T0 + MIN, &cfg(true, 5), last, &free(), 0);
        assert_eq!(p.slot_ms(), Some(T0 + 5 * HOUR_MS));
        p.step(T0 + 2 * MIN, &cfg(true, 1), last, &free(), 0);
        assert_eq!(p.slot_ms(), Some(T0 + HOUR_MS));
        // Shorter than what's already passed: at least 3 minutes from the change.
        let now = T0 + 2 * HOUR_MS;
        p.step(now, &cfg(true, 2), last, &free(), 0);
        assert_eq!(p.slot_ms(), Some(now + 3 * MIN));
        // Turning it off and on counts from the moment it's back on.
        p.step(now + MIN, &cfg(false, 2), last, &free(), 0);
        p.step(now + 2 * MIN, &cfg(true, 2), last, &free(), 0);
        assert_eq!(p.slot_ms(), Some(now + 5 * MIN));
    }

    #[test]
    fn a_lost_begin_race_counts_as_the_speed_test_blocker() {
        let mut p = Plan::default();
        let c = cfg(true, 3);
        p.step(T0, &c, None, &free(), 0);
        let due = T0 + 3 * MIN;
        assert_eq!(p.step(due, &c, None, &free(), 0), Decision::Run);
        // begin() said no: the user's test got there first.
        p.give_back();
        let busy = Blockers {
            speed_test: true,
            ..Blockers::default()
        };
        assert_eq!(
            p.step(due + 30_000, &c, None, &busy, 0),
            Decision::WaitUntil(due + GRACE_MS)
        );
        assert_eq!(
            p.step(due + GRACE_MS, &c, None, &busy, 0),
            Decision::Skip(SkipReason::SpeedTest, due)
        );
    }

    #[test]
    fn blockers_come_from_the_snapshot() {
        use router_core::{Phase, SafeApplyView};
        let mut s = AppSnapshot {
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
            captured_at_ms: 0,
            relock: None,
            wan: None,
            ip_refresh: None,
            data_switch_off: false,
            apn: None,
            apn_apply: None,
            ip_reattach: None,
        };
        assert_eq!(
            Blockers::from_snapshot(&s, false, false).first(),
            Some(SkipReason::Offline)
        );
        s.session = SessionView::LoggedIn;
        s.in_service = true;
        assert_eq!(Blockers::from_snapshot(&s, false, false).first(), None);
        assert_eq!(
            Blockers::from_snapshot(&s, true, true).first(),
            Some(SkipReason::AutoScan)
        );
        assert_eq!(
            Blockers::from_snapshot(&s, false, true).first(),
            Some(SkipReason::SpeedTest)
        );
        // A Refresh IP run holds a slot back, by name.
        s.ip_refresh = Some(super::super::running_ip_refresh());
        let b = Blockers::from_snapshot(&s, false, false);
        assert!(b.ip_refresh);
        assert_eq!(b.first(), Some(SkipReason::IpRefresh));
        s.ip_refresh = None;
        // So does an APN switch.
        s.apn_apply = Some(super::super::apn_change(Phase::Probation));
        let b = Blockers::from_snapshot(&s, false, false);
        assert!(b.apn_change);
        assert_eq!(b.first(), Some(SkipReason::ApnChange));
        s.apn_apply = None;
        // Logged out reads as that, not as offline.
        s.session = SessionView::LoggedOut;
        s.in_service = false;
        let b = Blockers::from_snapshot(&s, false, false);
        assert!(b.offline);
        assert_eq!(b.first(), Some(SkipReason::LoggedOut));
    }

    #[test]
    fn the_state_file_round_trips_and_a_bad_one_reads_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(StateFile::load(dir.path()), StateFile::default());
        let f = StateFile {
            last_attempt_ms: Some(T0),
            last_outcome: Some(AutoOutcome::Skipped),
            last_skip_reason: Some(SkipReason::BandChange),
        };
        f.save(dir.path()).unwrap();
        assert_eq!(StateFile::load(dir.path()), f);
        let text = std::fs::read_to_string(state_path(dir.path())).unwrap();
        assert!(text.contains("\"lastOutcome\": \"skipped\""), "{text}");
        assert!(!dir.path().join("auto-speedtest.json.tmp").exists());
        std::fs::write(state_path(dir.path()), "{not json").unwrap();
        assert_eq!(StateFile::load(dir.path()), StateFile::default());
    }

    #[test]
    fn the_status_shows_what_blocks_the_next_test() {
        let mut p = Plan::default();
        let c = cfg(true, 3);
        p.step(T0, &c, None, &offline(), 0);
        let s = status_of(&c, &p, &StateFile::default(), &offline(), false);
        assert_eq!(s.next_slot_ms, Some(T0 + 3 * MIN));
        assert_eq!(s.blocked_by, Some(SkipReason::Offline));
        let off = status_of(&cfg(false, 3), &p, &StateFile::default(), &offline(), false);
        assert_eq!((off.next_slot_ms, off.blocked_by), (None, None));
    }
}
