//! Headless band-config test runner.
//!
//! Applies each LTE band set of the plan through the app's own actor and safe-apply logic
//! (journal, verify, settle, probation, auto-revert), records the signal for a window after the
//! re-attach, keeps the change, and moves on. At the end it applies the best config that served
//! B3 with n41, or the everyday set (8080005) if none did. It never exits while a change
//! is active or a revert is pending.
//!
//! It uses the app's data directory, so if the runner or the PC dies mid-change the app resumes
//! the revert from the journal at its next launch. It refuses to run next to the app (the router
//! allows one admin session).
//!
//! Usage: `band-runner [--window <secs>] [--data-dir <dir>] [--mock]`
//! Stop early: create `<data dir>/band-runner.stop`. The runner reverts any active change,
//! returns to the everyday set and exits.

mod plan;

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use router_core::actor::{
    spawn_actor, ActorConfig, ActorHandle, AppSnapshot, Command, SessionView,
};
use router_core::drivers::huawei::HuaweiH122Driver;
use router_core::drivers::mock::{MockDriver, MockScenario};
use router_core::scan::{active, Applied, ChangeCtl};
use router_core::{Credentials, NetConfig, Phase, RouterDriver, SafeApplyTimers};
use serde_json::{json, Value};
use tokio::time::{sleep, Instant};

use plan::{pick_final, with_bands, Measurement, Sample, DEFAULT_BANDS, PLAN};

const APP_ID: &str = "io.github.hjbiki.router-band-tuner";
const KEYCHAIN_SERVICE: &str = "router-band-tuner";
const CONNECT_WITHIN: Duration = Duration::from_secs(180);
/// Apply → probation: verify (up to 20 s), re-attach grace (90 s), settle (10 s), plus slack.
const PROBATION_WITHIN: Duration = Duration::from_secs(8 * 60);
const SAMPLE_EVERY: Duration = Duration::from_millis(900);

struct Args {
    data_dir: Option<PathBuf>,
    window: Duration,
    mock: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        data_dir: None,
        window: Duration::from_secs(120),
        mock: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--data-dir" => a.data_dir = Some(it.next().ok_or("--data-dir needs a path")?.into()),
            "--window" => {
                let s = it.next().ok_or("--window needs seconds")?;
                a.window = Duration::from_secs(s.parse().map_err(|_| "bad --window")?);
            }
            "--mock" => a.mock = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if a.mock && a.data_dir.is_none() {
        return Err("--mock needs --data-dir (never the app's own directory)".into());
    }
    Ok(a)
}

fn app_data_dir() -> Result<PathBuf, String> {
    let base = std::env::var_os("APPDATA").ok_or("APPDATA is not set; pass --data-dir")?;
    let dir = PathBuf::from(base).join(APP_ID);
    router_core::legacy::migrate_data_dir(&dir)
        .map_err(|e| format!("can't move the old data folder: {e}"))?;
    Ok(dir)
}

fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The app holds the router's single admin session; two clients would keep kicking each other.
fn app_is_running() -> bool {
    if !cfg!(windows) {
        return false;
    }
    std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq router-band-tuner.exe", "/NH"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("router-band-tuner.exe"))
        .unwrap_or(false)
}

/// Removes the lock file when the runner ends.
struct Lock(PathBuf);

impl Lock {
    fn take(dir: &Path) -> Result<Self, String> {
        let path = dir.join("band-runner.lock");
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| {
                format!(
                    "{} exists: another runner is active, or one crashed (delete the file if so)",
                    path.display()
                )
            })?;
        let _ = write!(f, "{}", std::process::id());
        Ok(Self(path))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// JSON Lines results file plus a short line per record on stdout.
struct Recorder {
    file: File,
    started: Instant,
}

impl Recorder {
    fn log(&mut self, kind: &str, detail: Value) {
        let line = json!({ "at_ms": wall_ms(), "kind": kind, "detail": detail });
        let _ = writeln!(self.file, "{line}");
        let _ = self.file.flush();
        if kind != "sample" {
            println!(
                "[+{:>4}s] {kind} {detail}",
                self.started.elapsed().as_secs()
            );
        }
    }
}

struct StepResult {
    name: String,
    config: NetConfig,
    outcome: String,
    kept: bool,
    measurement: Option<Measurement>,
}

struct Runner {
    h: ActorHandle,
    ctl: ChangeCtl,
    rec: Recorder,
    window: Duration,
    stop_file: PathBuf,
    data_dir: PathBuf,
    /// The stop file was seen (and consumed).
    stopped: bool,
    /// The final restore runs even after a stop.
    finishing: bool,
}

/// Consumes the stop file on first sight, so the final restore isn't stopped by it too.
fn check_stop(stop_file: &Path, stopped: &mut bool, finishing: bool, rec: &mut Recorder) -> bool {
    if !*stopped && stop_file.exists() {
        let _ = std::fs::remove_file(stop_file);
        *stopped = true;
        rec.log("stop_requested", json!({}));
    }
    *stopped && !finishing
}

impl Runner {
    fn snap(&self) -> AppSnapshot {
        self.h.snapshots.borrow().clone()
    }

    fn stop_requested(&mut self) -> bool {
        check_stop(
            &self.stop_file,
            &mut self.stopped,
            self.finishing,
            &mut self.rec,
        )
    }

    /// Only once no change is active *and* no journal is on disk: an unwatched change must
    /// never be left behind (the dry run caught an exit right after an Apply was queued).
    async fn until_safe_to_exit(&mut self) {
        loop {
            self.until_no_change().await;
            sleep(Duration::from_secs(2)).await;
            let journal = self.data_dir.join("pending-change.json").exists();
            let phase = self.snap().safe_apply.phase;
            if !active(&phase) && !journal {
                return;
            }
            self.rec.log(
                "waiting_before_exit",
                json!({ "phase": phase, "journal": journal }),
            );
        }
    }

    /// Waits until `pred` holds or `within` passes; `None` on timeout.
    async fn wait_for(
        &mut self,
        within: Duration,
        pred: impl Fn(&AppSnapshot) -> bool,
    ) -> Option<AppSnapshot> {
        self.ctl.wait_for(within, pred).await
    }

    /// Never returns while a change is active, however long a failing revert takes.
    async fn until_no_change(&mut self) -> Phase {
        let rec = &mut self.rec;
        self.ctl
            .until_no_change(&mut |p| rec.log("waiting_for_change_to_end", json!({ "phase": p })))
            .await
    }

    /// Applies `cfg` and waits for probation (re-attached and settled), or for the outcome.
    async fn apply(&mut self, cfg: &NetConfig) -> Applied {
        let (file, finishing) = (self.stop_file.clone(), self.finishing);
        let (stopped, rec) = (&mut self.stopped, &mut self.rec);
        let mut stop = || check_stop(&file, stopped, finishing, rec);
        self.ctl.apply(cfg, PROBATION_WITHIN, &mut stop).await
    }

    async fn connect(&mut self, creds: Credentials) -> Result<(), String> {
        let _ = self.h.commands.send(Command::Connect(creds)).await;
        let s = self
            .wait_for(CONNECT_WITHIN, |s| {
                let failed = matches!(
                    s.session,
                    SessionView::BadCredentials
                        | SessionView::LockedOut { .. }
                        | SessionView::LoginRejected { .. }
                        | SessionView::AlgorithmMismatch
                        | SessionView::ConfirmRouter { .. }
                );
                failed
                    || (s.session == SessionView::LoggedIn
                        && s.config.is_some()
                        && s.capabilities.is_some())
            })
            .await
            .ok_or("no session within 3 minutes")?;
        if s.session != SessionView::LoggedIn {
            return Err(format!("login failed: {:?} (not retried)", s.session));
        }
        Ok(())
    }

    /// Samples every poll for the window. With `in_probation`, a revert during the window ends
    /// it early; the reason is returned.
    async fn measure(&mut self, step: &str, in_probation: bool) -> (Vec<Sample>, Option<String>) {
        let mut samples = Vec::new();
        let end = Instant::now() + self.window;
        let mut last_sample: Option<Instant> = None;
        while Instant::now() < end {
            let s = self.snap();
            if in_probation && s.safe_apply.phase != Phase::Probation {
                let p = self.until_no_change().await;
                return (samples, Some(format!("left probation: {p:?}")));
            }
            if self.stop_requested() {
                return (samples, Some("stop requested".into()));
            }
            let due = last_sample.is_none_or(|t| t.elapsed() >= SAMPLE_EVERY);
            if due && s.session == SessionView::LoggedIn {
                if let Some(sig) = &s.signal {
                    let sample = Sample::from_signal(sig, s.in_service);
                    self.rec
                        .log("sample", json!({ "step": step, "sample": sample }));
                    samples.push(sample);
                    last_sample = Some(Instant::now());
                }
            }
            sleep(Duration::from_millis(200)).await;
        }
        (samples, None)
    }

    async fn run_step(&mut self, name: &str, cfg: NetConfig) -> StepResult {
        self.rec
            .log("step_started", json!({ "step": name, "config": cfg }));
        let applied = self.apply(&cfg).await;
        self.rec.log(
            "step_applied",
            json!({ "step": name, "outcome": format!("{applied:?}") }),
        );
        let (outcome, kept, measurement) = match applied {
            Applied::Probation(secs) => {
                let (samples, aborted) = self.measure(name, true).await;
                let m = Measurement::of(&samples);
                match aborted {
                    Some(why) => (format!("reverted during window ({why})"), false, Some(m)),
                    None => {
                        let kept = self.ctl.keep().await;
                        let o = if kept {
                            format!("kept (re-attached after {secs:.0} s)")
                        } else {
                            "keep failed".to_string()
                        };
                        (o, kept, Some(m))
                    }
                }
            }
            Applied::AlreadyActive => {
                // Nothing changes, but let the readings settle like a real step would.
                sleep(Duration::from_secs(10)).await;
                let (samples, aborted) = self.measure(name, false).await;
                let kept = aborted.is_none();
                let o = match aborted {
                    Some(why) => format!("already active, window cut short ({why})"),
                    None => "already active (measured as is)".into(),
                };
                (o, kept, Some(Measurement::of(&samples)))
            }
            Applied::Rejected(m) => (format!("rejected: {m}"), false, None),
            Applied::Reverted { reason, fallback } => (
                format!("reverted: {reason:?}, fallback {fallback}"),
                false,
                None,
            ),
            Applied::TimedOut(p) => (
                format!("reverted: runner timeout, ended {p:?}"),
                false,
                None,
            ),
            Applied::Stopped => ("stopped".into(), false, None),
        };
        let r = StepResult {
            name: name.to_string(),
            config: cfg,
            outcome,
            kept,
            measurement,
        };
        self.rec.log(
            "step_result",
            json!({
                "step": r.name, "config": r.config, "outcome": r.outcome,
                "kept": r.kept, "measurement": r.measurement,
            }),
        );
        r
    }

    /// Applies `cfg` as the final config and lets probation run to its own commit.
    async fn finish_on(&mut self, cfg: &NetConfig) -> bool {
        if self.snap().config.as_ref() == Some(cfg) {
            return true;
        }
        match self.apply(cfg).await {
            Applied::Probation(_) => matches!(self.until_no_change().await, Phase::Committed),
            Applied::AlreadyActive => true,
            other => {
                self.rec.log(
                    "final_apply_failed",
                    json!({ "outcome": format!("{other:?}") }),
                );
                false
            }
        }
    }
}

fn fmt_stat(s: &plan::Stat) -> String {
    match (s.avg, s.min, s.max) {
        (Some(a), Some(lo), Some(hi)) => format!("{a:.1} ({lo:.1}..{hi:.1})"),
        _ => "-".into(),
    }
}

fn summary(start: &NetConfig, results: &[StepResult], end: &str) -> String {
    let mut out = String::new();
    let mask = |c: &NetConfig| serde_json::to_value(c.lte_bands).unwrap_or_default();
    out.push_str(&format!("Start config LTE mask: {}\n\n", mask(start)));
    for r in results {
        out.push_str(&format!(
            "{} (mask {}): {}\n",
            r.name,
            mask(&r.config),
            r.outcome
        ));
        if let Some(m) = &r.measurement {
            out.push_str(&format!(
                "  samples {} | in service {:.0}% | LTE SINR {} dB | LTE RSRP {} dBm\n",
                m.samples,
                m.in_service_ratio * 100.0,
                fmt_stat(&m.lte_sinr),
                fmt_stat(&m.lte_rsrp)
            ));
            out.push_str(&format!(
                "  NR present {:.0}% | NR band at end {} | NR SINR {} dB | NR RSRP {} dBm\n",
                m.nr_present_ratio * 100.0,
                m.end_nr_band.map_or("-".into(), |b| format!("n{b}")),
                fmt_stat(&m.nr_sinr),
                fmt_stat(&m.nr_rsrp)
            ));
            let cells: Vec<String> = m
                .cells_seen
                .iter()
                .map(|(b, p)| {
                    format!(
                        "B{}/PCI {}",
                        b.map_or("?".into(), |b| b.to_string()),
                        p.map_or("?".into(), |p| p.to_string())
                    )
                })
                .collect();
            out.push_str(&format!("  cells: {}\n", cells.join(" -> ")));
        }
    }
    out.push_str(&format!("\n{end}\n"));
    out
}

async fn run(args: Args) -> Result<i32, String> {
    let data_dir = match &args.data_dir {
        Some(d) => d.clone(),
        None => app_data_dir()?,
    };
    std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
    if !args.mock && app_is_running() {
        return Err("the app is running: quit it first (the router allows one session)".into());
    }
    let _lock = Lock::take(&data_dir)?;

    let settings: Value = if args.mock {
        json!({ "host": "mock", "username": "admin" })
    } else {
        let raw = std::fs::read_to_string(data_dir.join("settings.json"))
            .map_err(|e| format!("can't read the app's settings.json: {e}"))?;
        serde_json::from_str(&raw).map_err(|e| e.to_string())?
    };
    let host = settings["host"]
        .as_str()
        .unwrap_or("192.168.8.1")
        .to_string();
    let user = settings["username"].as_str().unwrap_or("admin").to_string();
    let secs = |k: &str, d: u64| Duration::from_secs(settings["timers"][k].as_u64().unwrap_or(d));
    let timers = SafeApplyTimers {
        grace: secs("graceS", 90),
        loss: secs("lossS", 60),
        probation: secs("probationS", 180),
        ..SafeApplyTimers::default()
    };

    let (driver, password): (Arc<dyn RouterDriver>, String) = if args.mock {
        (
            Arc::new(MockDriver::new(MockScenario::zain_like())),
            "mock".into(),
        )
    } else {
        // Read-only: the app moves a pre-rename password to the new service on its next start.
        let saved = |service: &str| {
            keyring::Entry::new(service, &format!("{host}/{user}"))
                .and_then(|e| e.get_password())
                .ok()
        };
        let pw = saved(KEYCHAIN_SERVICE)
            .or_else(|| saved(router_core::legacy::LEGACY_KEYCHAIN_SERVICE))
            .ok_or("no saved password in the keychain: save it in the app first")?;
        if data_dir
            .join(router_core::password_marker::MARKER_FILE)
            .exists()
        {
            return Err("the router rejected the saved password: re-save it in the app".into());
        }
        let d = HuaweiH122Driver::new(&host).map_err(|e| e.to_string())?;
        (Arc::new(d), pw)
    };

    let runs = data_dir.join("band-runs");
    std::fs::create_dir_all(&runs).map_err(|e| e.to_string())?;
    let stamp = wall_ms();
    let results_path = runs.join(format!("run-{stamp}.jsonl"));
    let file = File::create(&results_path).map_err(|e| e.to_string())?;
    let stop_file = data_dir.join("band-runner.stop");
    let _ = std::fs::remove_file(&stop_file);

    let h = spawn_actor(
        driver,
        ActorConfig {
            host: host.clone(),
            timers,
            data_dir: data_dir.clone(),
            cell_alerts: true,
            automation: router_core::actor::AutomationConfig::default(),
        },
    );
    let ctl = ChangeCtl {
        commands: h.commands.clone(),
        snapshots: h.snapshots.clone(),
        alerts: h.alerts.subscribe(),
    };
    let mut r = Runner {
        h,
        ctl,
        rec: Recorder {
            file,
            started: Instant::now(),
        },
        window: args.window,
        stop_file,
        data_dir: data_dir.clone(),
        stopped: false,
        finishing: false,
    };
    r.rec.log(
        "run_started",
        json!({ "results": results_path, "window_s": args.window.as_secs(), "mock": args.mock }),
    );

    if let Err(e) = r.connect(Credentials::new(user, password)).await {
        r.rec.log("run_aborted", json!({ "error": e }));
        r.h.shutdown(Duration::from_secs(15)).await;
        return Err(e);
    }
    // A change left by the app or an earlier run is finished first.
    if active(&r.snap().safe_apply.phase) {
        r.rec.log(
            "resumed_change",
            json!({ "phase": r.snap().safe_apply.phase }),
        );
        r.until_no_change().await;
    }
    let base = r.snap().config.ok_or("no config read")?;
    r.rec.log("start_config", json!({ "config": base }));

    let mut results = Vec::new();
    for t in PLAN {
        if r.stop_requested() {
            break;
        }
        let res = r.run_step(t.name, with_bands(&base, t.bands)).await;
        let stopped = res.outcome == "stopped";
        results.push(res);
        if stopped {
            break;
        }
    }

    let default = with_bands(&base, DEFAULT_BANDS);
    r.stop_requested();
    let stopped = r.stopped;
    r.finishing = true;
    let picks: Vec<_> = results
        .iter()
        .map(|x| (x.kept, x.measurement.as_ref()))
        .collect();
    let choice = if stopped { None } else { pick_final(&picks) };
    let (target, why) = match choice {
        Some(i) => (
            results[i].config.clone(),
            format!("best B3 + n41: {}", results[i].name),
        ),
        None => (
            default.clone(),
            "no step qualified (or stopped): everyday set 8080005".to_string(),
        ),
    };
    r.rec
        .log("final_choice", json!({ "config": target, "why": why }));
    let mut ok = r.finish_on(&target).await;
    if !ok && target != default {
        r.rec.log("final_fallback", json!({ "config": default }));
        ok = r.finish_on(&default).await;
    }
    r.until_safe_to_exit().await;
    let end = r.snap();
    let end_line = format!(
        "End: {} | router config LTE mask {} | phase {:?}",
        if ok {
            "final config committed"
        } else {
            "FINAL CONFIG NOT CONFIRMED"
        },
        end.config
            .as_ref()
            .map(|c| serde_json::to_value(c.lte_bands).unwrap_or_default())
            .unwrap_or_default(),
        end.safe_apply.phase
    );
    r.rec.log(
        "run_finished",
        json!({ "ok": ok, "end_config": end.config }),
    );
    let text = summary(&base, &results, &format!("Final choice: {why}\n{end_line}"));
    let _ = std::fs::write(runs.join(format!("run-{stamp}.summary.txt")), &text);
    println!("\n{text}");
    r.h.shutdown(Duration::from_secs(15)).await;
    Ok(if ok { 0 } else { 2 })
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let code = match parse_args() {
        Ok(a) => match run(a).await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("band-runner: {e}");
                1
            }
        },
        Err(e) => {
            eprintln!("band-runner: {e}");
            1
        }
    };
    std::process::exit(code);
}
