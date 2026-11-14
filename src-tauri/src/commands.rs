//! Tauri commands. Errors serialize as a `RouterError` or a plain string; no
//! command returns a secret.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use router_core::drivers::huawei::HuaweiH122Driver;
use router_core::drivers::mock::{MockDriver, MockScenario};
use router_core::password_marker;
use router_core::{
    spawn_actor, ActorConfig, ActorHandle, AppSnapshot, AutomationConfig, Command, Credentials,
    LogoutOutcome, NetConfig, Phase, RebootTicket, RouterDriver, RouterError, SafeApplyTimers,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::oneshot;

use crate::credentials::CredentialStore;
use crate::settings::{AutomationSettings, DriverId, Language, Settings, TimerSettings};

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum CmdError {
    Router(RouterError),
    Message(String),
}

impl From<RouterError> for CmdError {
    fn from(e: RouterError) -> Self {
        Self::Router(e)
    }
}

impl From<String> for CmdError {
    fn from(e: String) -> Self {
        Self::Message(e)
    }
}

type CmdResult<T> = Result<T, CmdError>;

/// Password to log in with: the keychain entry, or the fixed demo password for the mock
/// driver. `None` means "not set up".
fn connect_password(stored: Option<String>, driver: DriverId) -> Option<String> {
    match (stored, driver) {
        (Some(p), _) => Some(p),
        (None, DriverId::Mock) => Some(MOCK_PASSWORD.to_string()),
        (None, _) => None,
    }
}

const MOCK_PASSWORD: &str = "mock";

/// `credentials()` refuses with this while `router_core::password_marker` names the account.
const PASSWORD_REJECTED: &str = "saved password was rejected";

/// Snapshot events are forwarded at most this often (10 per second).
const SNAPSHOT_MIN_GAP: Duration = Duration::from_millis(100);

pub struct AppState {
    app: AppHandle,
    data_dir: PathBuf,
    store: Arc<dyn CredentialStore>,
    inner: Mutex<Inner>,
    /// Held for the whole of any command that replaces the actor.
    lifecycle: tokio::sync::Mutex<()>,
    /// The redacted diagnostics the user was shown; `save_diagnostics` writes exactly this.
    diagnostics: Mutex<Option<String>>,
    /// The main window is shown. Set by `set_visible` and the tray before the actor hears.
    visible: AtomicBool,
    /// The page-script zip the user was shown a summary of; `save_page_scripts` writes it.
    page_scripts: Mutex<Option<PreparedScripts>>,
    /// A `prepare_page_scripts` run is reading the router.
    page_scripts_busy: AtomicBool,
}

/// A zip of the router's page scripts, built in memory, and what it holds.
struct PreparedScripts {
    zip: Vec<u8>,
    summary: PageScriptsSummary,
}

struct Inner {
    settings: Settings,
    driver: Arc<dyn RouterDriver>,
    handle: ActorHandle,
}

fn timers(t: &TimerSettings) -> SafeApplyTimers {
    SafeApplyTimers {
        grace: Duration::from_secs(t.grace_s.into()),
        loss: Duration::from_secs(t.loss_s.into()),
        probation: Duration::from_secs(t.probation_s.into()),
        ..SafeApplyTimers::default()
    }
}

fn automation(a: &AutomationSettings) -> AutomationConfig {
    AutomationConfig {
        primary_band: a.primary_band,
        // Never armed without a band to re-lock to.
        relock_primary: a.relock_primary && a.primary_band.is_some(),
        revert_on_loss: a.revert_on_loss,
    }
}

fn make_driver(s: &Settings) -> Result<Arc<dyn RouterDriver>, RouterError> {
    Ok(match s.driver {
        DriverId::HuaweiH122 => Arc::new(HuaweiH122Driver::new(&s.host)?),
        DriverId::Mock => Arc::new(MockDriver::new(MockScenario::zain_like())),
    })
}

/// Spawn an actor and the tasks that forward its snapshots and alerts to the UI.
/// The forwarders end by themselves when the actor (and its channels) go away.
fn start_actor(
    app: &AppHandle,
    driver: Arc<dyn RouterDriver>,
    s: &Settings,
    data_dir: &std::path::Path,
) -> ActorHandle {
    let handle = spawn_actor(
        driver,
        ActorConfig {
            host: s.host.clone(),
            timers: timers(&s.timers),
            data_dir: data_dir.to_path_buf(),
            cell_alerts: s.cell_alerts,
            automation: automation(&s.automation),
        },
    );

    let driver_name = serde_json::to_value(s.driver)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    crate::history::sampler::spawn_sampler(
        app,
        handle.snapshots.clone(),
        s.host.clone(),
        driver_name,
    );

    let mut snaps = handle.snapshots.clone();
    let a = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let snap = snaps.borrow_and_update().clone();
            let _ = a.emit("snapshot", &snap);
            crate::tray::on_snapshot(&a, &snap);
            tokio::time::sleep(SNAPSHOT_MIN_GAP).await;
            if snaps.changed().await.is_err() {
                break;
            }
        }
    });

    let mut alerts = handle.alerts.subscribe();
    let a = app.clone();
    tauri::async_runtime::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            match alerts.recv().await {
                Ok(alert) => {
                    let _ = a.emit("alert", &alert);
                    crate::notify::on_alert(&a, &alert);
                }
                Err(RecvError::Lagged(n)) => {
                    log_lag(&a, n);
                    continue;
                }
                Err(RecvError::Closed) => break,
            }
        }
    });
    handle
}

/// The alert forwarder fell behind and `skipped` alerts (and their notifications) were lost.
/// Leaves a trace in `events.jsonl`; a failed append is dropped.
fn log_lag(app: &AppHandle, skipped: u64) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let dir = state.data_dir().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let rec = router_core::EventRecord {
            at_ms: crate::notify::now_ms(),
            kind: router_core::event_text::kinds::ALERTS_LAGGED.into(),
            detail: serde_json::json!({ "skipped": skipped }),
        };
        let _ = router_core::EventLog::new(dir).append(&rec);
    });
}

impl AppState {
    /// Whether the user allowed the update check; `None` until asked.
    pub fn update_check(&self) -> Option<bool> {
        self.inner.lock().unwrap().settings.update_check
    }

    /// A report waiting for `save_diagnostics`, once the user has read it.
    pub fn set_prepared_diagnostics(&self, text: String) {
        *self.diagnostics.lock().unwrap() = Some(text);
    }

    /// The router address in the current settings.
    pub fn host(&self) -> String {
        self.inner.lock().unwrap().settings.host.clone()
    }

    /// Which router a scan belongs to, and the re-attach grace it allows each change.
    pub fn scan_identity(&self) -> (String, String, std::time::Duration) {
        let g = self.inner.lock().unwrap();
        let driver = serde_json::to_value(g.settings.driver)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        (
            g.settings.host.clone(),
            driver,
            std::time::Duration::from_secs(u64::from(g.settings.timers.grace_s)),
        )
    }

    /// A handle for driving band changes from outside the actor.
    pub fn change_ctl(&self) -> router_core::scan::ChangeCtl {
        let g = self.inner.lock().unwrap();
        router_core::scan::ChangeCtl {
            commands: g.handle.commands.clone(),
            snapshots: g.handle.snapshots.clone(),
            alerts: g.handle.alerts.subscribe(),
        }
    }

    /// Must run inside the async runtime (the actor is a tokio task).
    pub fn start(
        app: AppHandle,
        settings: Settings,
        data_dir: PathBuf,
        store: Arc<dyn CredentialStore>,
    ) -> Result<Self, RouterError> {
        let driver = make_driver(&settings)?;
        let handle = start_actor(&app, driver.clone(), &settings, &data_dir);
        Ok(Self {
            app,
            data_dir,
            store,
            lifecycle: tokio::sync::Mutex::new(()),
            diagnostics: Mutex::new(None),
            visible: AtomicBool::new(true),
            page_scripts: Mutex::new(None),
            page_scripts_busy: AtomicBool::new(false),
            inner: Mutex::new(Inner {
                settings,
                driver,
                handle,
            }),
        })
    }

    fn sender(&self) -> tokio::sync::mpsc::Sender<Command> {
        self.inner.lock().unwrap().handle.commands.clone()
    }

    async fn send(&self, cmd: Command) -> CmdResult<()> {
        self.sender()
            .send(cmd)
            .await
            .map_err(|_| CmdError::Message("router service is not running".into()))
    }

    /// Queues a command right away from a sync context, so commands sent this way keep their
    /// order; only a full queue falls back to a detached send.
    pub(crate) fn send_now(&self, cmd: Command) {
        use tokio::sync::mpsc::error::TrySendError;
        let tx = self.sender();
        if let Err(TrySendError::Full(cmd)) = tx.try_send(cmd) {
            tauri::async_runtime::spawn(async move {
                let _ = tx.send(cmd).await;
            });
        }
    }

    /// Fire-and-forget command from a sync context (tray and window events).
    pub(crate) fn send_detached(&self, cmd: Command) {
        let tx = self.sender();
        tauri::async_runtime::spawn(async move {
            let _ = tx.send(cmd).await;
        });
    }

    /// The router's page scripts are being read (`prepare_page_scripts`): no speed test or
    /// auto-scan may start.
    pub(crate) fn page_scripts_reading(&self) -> bool {
        self.page_scripts_busy.load(Ordering::SeqCst)
    }

    pub(crate) fn snapshot(&self) -> AppSnapshot {
        self.inner.lock().unwrap().handle.snapshots.borrow().clone()
    }

    /// At exit with a Refresh IP run open: stops the actor, which then sends one "on" (at
    /// most 5 s) before the process ends. The data-switch record stays, so the next launch
    /// confirms the switch either way. Blocking: call from the event loop, not the runtime.
    pub(crate) fn finish_ip_refresh_at_exit(&self) {
        if !crate::speedtest::ip_refresh_active(&self.snapshot()) {
            return;
        }
        let handle = self.inner.lock().unwrap().handle.clone();
        tauri::async_runtime::block_on(handle.shutdown(EXIT_SHUTDOWN_WAIT));
    }

    pub(crate) fn language(&self) -> Language {
        Language::resolve(&self.inner.lock().unwrap().settings.language)
    }

    /// `(language, cell_alerts)` for notifications.
    pub(crate) fn notify_prefs(&self) -> (Language, bool) {
        let g = self.inner.lock().unwrap();
        (
            Language::resolve(&g.settings.language),
            g.settings.cell_alerts,
        )
    }

    /// First call returns the language to show the "still running in the tray" notice
    /// in and records that it was shown; later calls return `None`.
    pub(crate) fn take_tray_notice(&self) -> Option<Language> {
        let mut g = self.inner.lock().unwrap();
        if g.settings.tray_notice_shown {
            return None;
        }
        g.settings.tray_notice_shown = true;
        let _ = g.settings.save(&self.data_dir);
        Some(Language::resolve(&g.settings.language))
    }

    /// Keeps the Refresh IP choices for the next dialog. Best effort: a failed save only loses
    /// the prefill.
    fn remember_ip_refresh(&self, target: Option<String>, max_tries: u8) {
        let mut g = self.inner.lock().unwrap();
        let mut next = g.settings.ip_refresh.clone();
        if target.is_some() {
            next.last_target = target;
        }
        next.max_tries = max_tries;
        if next == g.settings.ip_refresh {
            return;
        }
        g.settings.ip_refresh = next;
        let _ = g.settings.save(&self.data_dir);
    }

    fn account(&self) -> (String, String) {
        let g = self.inner.lock().unwrap();
        (g.settings.host.clone(), g.settings.username.clone())
    }

    /// Credentials for the configured account (keychain read off the async runtime).
    async fn credentials(&self) -> Result<Option<Credentials>, String> {
        let (host, user) = self.account();
        let driver = self.inner.lock().unwrap().settings.driver;
        if driver != DriverId::Mock && password_marker::is_marked(&self.data_dir, &host, &user) {
            return Err(PASSWORD_REJECTED.to_string());
        }
        let store = self.store.clone();
        let h = host.clone();
        let u = user.clone();
        let stored = blocking(move || store.get(&h, &u)).await?;
        Ok(connect_password(stored, driver).map(|pw| Credentials::new(user, pw)))
    }

    /// At launch and after an actor restart: connect by itself when a password is saved (or
    /// the demo driver is selected), so a resumed band change is watched without the window.
    /// Without credentials the session stays `NotConfigured`. Only the router recorded at the
    /// last verified login gets a login proof this way; anything else asks the user first.
    pub async fn auto_connect(&self) {
        if let Ok(Some(creds)) = self.credentials().await {
            let _ = self.send(Command::ConnectAutomatic(creds)).await;
        }
    }
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.inner.lock().unwrap().settings.clone()
}

/// At exit, the actor gets this long to turn mobile data back on (its own limit is 5 s).
const EXIT_SHUTDOWN_WAIT: Duration = Duration::from_secs(8);

/// The refusals of a router or driver change while something is open.
pub const RESTART_BAND_CHANGE: &str =
    "Finish or revert the current band change before changing router or driver.";
pub const RESTART_IP_REFRESH: &str =
    "Wait for the IP refresh to finish before changing router or driver.";
pub const RESTART_APN_CHANGE: &str =
    "Finish or undo the APN change before changing router or driver.";

/// Why the actor can't be restarted for a new router or driver now. The new actor would read
/// an open band, APN or data-switch record as another router's and never act on it, so the
/// change would lose its automatic undo.
pub(crate) fn restart_refusal(s: &AppSnapshot) -> Option<&'static str> {
    if crate::speedtest::band_change_open(&s.safe_apply) {
        Some(RESTART_BAND_CHANGE)
    } else if crate::speedtest::ip_refresh_active(s) {
        Some(RESTART_IP_REFRESH)
    } else if crate::speedtest::apn_change_open(s) {
        Some(RESTART_APN_CHANGE)
    } else {
        None
    }
}

/// Validates and persists the settings. A host or driver change restarts the actor
/// (refused while a band change is active); timer and cell-alert changes apply live.
/// A `username` change takes effect at the next `connect()`.
#[tauri::command]
pub async fn save_settings(
    state: State<'_, AppState>,
    scan: State<'_, crate::autoscan::ScanState>,
    mut s: Settings,
) -> CmdResult<()> {
    s.normalize();
    s.validate()?;
    let _lifecycle = state.lifecycle.lock().await;
    let (old, tx, snapshot) = {
        let g = state.inner.lock().unwrap();
        let snapshot = g.handle.snapshots.borrow().clone();
        (g.settings.clone(), g.handle.commands.clone(), snapshot)
    };
    // The UI may hold a stale copy; the one-time tray notice and guide must not come back.
    s.tray_notice_shown |= old.tray_notice_shown;
    s.onboarding_done |= old.onboarding_done;
    let restart = old.host != s.host || old.driver != s.driver;
    if restart && scan.is_running() {
        return Err(crate::autoscan::ERR_RUNNING.to_string().into());
    }
    if let Some(why) = restart.then(|| restart_refusal(&snapshot)).flatten() {
        return Err(why.to_string().into());
    }
    // Build the new driver first so a bad host leaves everything untouched.
    let new_driver = if restart {
        Some(make_driver(&s)?)
    } else {
        None
    };
    s.save(&state.data_dir).map_err(|e| e.to_string())?;
    crate::sync_autostart(&state.app, s.start_at_login);
    let prune =
        crate::history::maintenance::lowered(old.history.retention_days, s.history.retention_days);

    if let Some(driver) = new_driver {
        let old_handle = state.inner.lock().unwrap().handle.clone();
        // Wait for the old actor to exit before a new one touches the journal or router.
        old_handle.shutdown(Duration::from_secs(15)).await;
        let handle = start_actor(&state.app, driver.clone(), &s, &state.data_dir);
        {
            let mut g = state.inner.lock().unwrap();
            g.settings = s;
            g.driver = driver;
            g.handle = handle;
        }
        state.auto_connect().await;
    } else {
        let t = timers(&s.timers);
        let cell_alerts = s.cell_alerts;
        let auto = automation(&s.automation);
        state.inner.lock().unwrap().settings = s;
        let _ = tx.send(Command::UpdateTimers(t)).await;
        if old.cell_alerts != cell_alerts {
            let _ = tx.send(Command::SetCellAlerts(cell_alerts)).await;
        }
        if automation(&old.automation) != auto {
            let _ = tx.send(Command::SetAutomation(auto)).await;
        }
    }
    // After the new settings are in place: the prune reads the retention from them.
    if prune {
        crate::history::maintenance::prune_soon(&state.app);
    }
    Ok(())
}

#[tauri::command]
pub async fn save_credentials(state: State<'_, AppState>, password: String) -> CmdResult<()> {
    let (host, user) = state.account();
    let store = state.store.clone();
    blocking(move || store.set(&host, &user, &password)).await?;
    password_marker::clear(&state.data_dir);
    Ok(())
}

#[tauri::command]
pub async fn forget_credentials(state: State<'_, AppState>) -> CmdResult<()> {
    let (host, user) = state.account();
    let store = state.store.clone();
    blocking(move || store.delete(&host, &user)).await?;
    password_marker::clear(&state.data_dir);
    router_core::identity_file::remove(&state.data_dir);
    Ok(())
}

/// Whether a password is saved for the configured account, and whether the router rejected
/// it. Never carries the password itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CredentialStatus {
    pub saved: bool,
    pub rejected: bool,
}

/// The keychain value is only tested for presence and dropped straight away.
fn credential_status_of(
    store: &dyn CredentialStore,
    data_dir: &std::path::Path,
    host: &str,
    user: &str,
    driver: DriverId,
) -> Result<CredentialStatus, String> {
    if driver == DriverId::Mock {
        return Ok(CredentialStatus {
            saved: true,
            rejected: false,
        });
    }
    let saved = store.get(host, user)?.is_some();
    Ok(CredentialStatus {
        saved,
        rejected: password_marker::is_marked(data_dir, host, user),
    })
}

/// Lets the UI tell "no password saved" from "rejected last time" from "saved, not logged in".
#[tauri::command]
pub async fn credential_status(state: State<'_, AppState>) -> CmdResult<CredentialStatus> {
    let (host, user) = state.account();
    let driver = state.inner.lock().unwrap().settings.driver;
    let store = state.store.clone();
    let dir = state.data_dir.clone();
    Ok(blocking(move || credential_status_of(store.as_ref(), &dir, &host, &user, driver)).await?)
}

/// Keychain access can block (OS prompts, locked keyring), so keep it off the runtime.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn connect(state: State<'_, AppState>) -> CmdResult<()> {
    let Some(creds) = state.credentials().await? else {
        return Err("no saved password".to_string().into());
    };
    state.send(Command::Connect(creds)).await
}

/// The user read the "is this your router?" question and said yes.
#[tauri::command]
pub async fn connect_trusting(state: State<'_, AppState>) -> CmdResult<()> {
    let Some(creds) = state.credentials().await? else {
        return Err("no saved password".to_string().into());
    };
    state.send(Command::ConnectTrusting(creds)).await
}

/// Logs out of the router and stays out, across restarts too, until Connect. The saved
/// password stays. Refused while a band change, scan or speed test holds the router.
#[tauri::command]
pub async fn logout(state: State<'_, AppState>) -> CmdResult<LogoutOutcome> {
    let (tx, rx) = oneshot::channel();
    state.send(Command::Logout(tx)).await?;
    rx.await
        .map_err(|_| CmdError::Message("router service stopped".into()))?
        .map_err(Into::into)
}

/// Starts Refresh IP: mobile data off and on, until the address starts with `target` (one or
/// two numbers) or `max_tries` (1 to 5) is used up; one try without a target. The actor refuses
/// it while anything else holds the router. On a start, the choices are kept for the dialog.
#[tauri::command]
pub async fn ip_refresh_start(
    state: State<'_, AppState>,
    scan: State<'_, crate::autoscan::ScanState>,
    st: State<'_, crate::speedtest::SpeedTestState>,
    target: Option<String>,
    max_tries: u8,
) -> CmdResult<()> {
    if scan.is_running() {
        return Err(crate::autoscan::ERR_RUNNING.to_string().into());
    }
    if st.is_running() {
        return Err(router_core::SPEED_TEST_RUNNING.to_string().into());
    }
    let target = target
        .map(|t| t.trim().trim_end_matches('.').to_string())
        .filter(|t| !t.is_empty());
    let (tx, rx) = oneshot::channel();
    state
        .send(Command::StartIpRefresh {
            target: target.clone(),
            max_tries,
            reply: tx,
        })
        .await?;
    rx.await
        .map_err(|_| CmdError::Message("router service stopped".into()))?
        .map_err(CmdError::from)?;
    state.remember_ip_refresh(target, max_tries);
    Ok(())
}

/// Gets a new IP by switching bands for a moment: a two-step band change through safe apply
/// (other bands, then the ones from before), with the automatic undo of every band change. The
/// UI offers it only as a button; the actor refuses it while anything else holds the router.
#[tauri::command]
pub async fn ip_reattach_start(
    state: State<'_, AppState>,
    scan: State<'_, crate::autoscan::ScanState>,
    st: State<'_, crate::speedtest::SpeedTestState>,
) -> CmdResult<()> {
    if scan.is_running() {
        return Err(crate::autoscan::ERR_RUNNING.to_string().into());
    }
    if st.is_running() {
        return Err(router_core::SPEED_TEST_RUNNING.to_string().into());
    }
    let (tx, rx) = oneshot::channel();
    state.send(Command::StartIpReattach(tx)).await?;
    rx.await
        .map_err(|_| CmdError::Message("router service stopped".into()))?
        .map_err(CmdError::from)
}

/// Stops Refresh IP; data is turned back on first when it is off.
#[tauri::command]
pub async fn ip_refresh_stop(state: State<'_, AppState>) -> CmdResult<()> {
    state.send(Command::StopIpRefresh).await
}

/// The "Turn mobile data on" button: one "on", then a read of the switch.
#[tauri::command]
pub async fn data_switch_on(state: State<'_, AppState>) -> CmdResult<()> {
    let (tx, rx) = oneshot::channel();
    state.send(Command::TurnDataOn(tx)).await?;
    rx.await
        .map_err(|_| CmdError::Message("router service stopped".into()))?
        .map_err(Into::into)
}

/// The APN profiles from the last read (no passwords), or `None` before the first read.
#[tauri::command]
pub fn apn_profiles(state: State<'_, AppState>) -> Option<router_core::ApnProfiles> {
    state.snapshot().apn
}

/// Reads the APN profiles again now; the snapshot carries the result.
#[tauri::command]
pub async fn apn_refresh(state: State<'_, AppState>) -> CmdResult<()> {
    state.send(Command::ApnRefresh).await
}

/// Switches the APN profile in use through safe apply (automatic undo when it gives no data).
/// `fingerprint` is the one the user saw; the backend refuses when the profile changed since.
/// Refused during an auto-scan or speed test before it reaches the actor.
#[tauri::command]
pub async fn apn_switch(
    state: State<'_, AppState>,
    scan: State<'_, crate::autoscan::ScanState>,
    st: State<'_, crate::speedtest::SpeedTestState>,
    index: u32,
    fingerprint: String,
) -> CmdResult<()> {
    if scan.is_running() {
        return Err(crate::autoscan::ERR_RUNNING.to_string().into());
    }
    if st.is_running() {
        return Err(router_core::SPEED_TEST_RUNNING.to_string().into());
    }
    let (tx, rx) = oneshot::channel();
    let target = router_core::ApnTarget { index, fingerprint };
    state.send(Command::ApnSwitch { target, reply: tx }).await?;
    rx.await
        .map_err(|_| CmdError::Message("router service stopped".into()))?
        .map_err(Into::into)
}

/// Refuses an APN add, edit or delete during an auto-scan or speed test, before the actor.
fn apn_write_blocked(
    scan: &crate::autoscan::ScanState,
    st: &crate::speedtest::SpeedTestState,
) -> CmdResult<()> {
    if scan.is_running() {
        return Err(crate::autoscan::ERR_RUNNING.to_string().into());
    }
    if st.is_running() {
        return Err(router_core::SPEED_TEST_RUNNING.to_string().into());
    }
    Ok(())
}

async fn apn_reply<T>(rx: oneshot::Receiver<Result<T, router_core::RouterError>>) -> CmdResult<T> {
    rx.await
        .map_err(|_| CmdError::Message("router service stopped".into()))?
        .map_err(Into::into)
}

/// Adds an APN profile; returns its index. The draft may hold a password: it goes to the
/// router and nowhere else (never logged, stored or put in an error).
#[tauri::command]
pub async fn apn_add(
    state: State<'_, AppState>,
    scan: State<'_, crate::autoscan::ScanState>,
    st: State<'_, crate::speedtest::SpeedTestState>,
    draft: router_core::ApnDraft,
) -> CmdResult<u32> {
    apn_write_blocked(&scan, &st)?;
    let (tx, rx) = oneshot::channel();
    state.send(Command::ApnAdd { draft, reply: tx }).await?;
    apn_reply(rx).await
}

/// Rewrites a profile that isn't in use. `fingerprint` is the one the user saw.
#[tauri::command]
pub async fn apn_edit(
    state: State<'_, AppState>,
    scan: State<'_, crate::autoscan::ScanState>,
    st: State<'_, crate::speedtest::SpeedTestState>,
    index: u32,
    fingerprint: String,
    draft: router_core::ApnDraft,
) -> CmdResult<()> {
    apn_write_blocked(&scan, &st)?;
    let (tx, rx) = oneshot::channel();
    let target = router_core::ApnTarget { index, fingerprint };
    let cmd = Command::ApnEdit {
        target,
        draft,
        reply: tx,
    };
    state.send(cmd).await?;
    apn_reply(rx).await
}

/// Deletes a profile that isn't in use. `fingerprint` is the one the user saw.
#[tauri::command]
pub async fn apn_delete(
    state: State<'_, AppState>,
    scan: State<'_, crate::autoscan::ScanState>,
    st: State<'_, crate::speedtest::SpeedTestState>,
    index: u32,
    fingerprint: String,
) -> CmdResult<()> {
    apn_write_blocked(&scan, &st)?;
    let (tx, rx) = oneshot::channel();
    let target = router_core::ApnTarget { index, fingerprint };
    state.send(Command::ApnDelete { target, reply: tx }).await?;
    apn_reply(rx).await
}

/// Exact request an APN switch would send; no network call.
#[tauri::command]
pub fn apn_preview(state: State<'_, AppState>, index: u32, fingerprint: String) -> ApplyPreviewDto {
    let driver = state.inner.lock().unwrap().driver.clone();
    let p = driver.apn_preview(&router_core::ApnTarget { index, fingerprint });
    ApplyPreviewDto {
        fields: p.fields,
        body: p.body,
    }
}

#[tauri::command]
pub fn get_snapshot(state: State<'_, AppState>) -> AppSnapshot {
    state
        .inner
        .lock()
        .unwrap()
        .handle
        .snapshots
        .borrow()
        .clone()
}

#[derive(Debug, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, rename = "ApplyPreview")]
pub struct ApplyPreviewDto {
    pub fields: Vec<(String, String)>,
    pub body: String,
}

/// Exact request that would be sent; no network call.
#[tauri::command]
pub fn preview_config(state: State<'_, AppState>, cfg: NetConfig) -> ApplyPreviewDto {
    let driver = state.inner.lock().unwrap().driver.clone();
    let p = driver.apply_preview(&cfg);
    ApplyPreviewDto {
        fields: p.fields,
        body: p.body,
    }
}

#[tauri::command]
pub async fn apply_config(
    state: State<'_, AppState>,
    scan: State<'_, crate::autoscan::ScanState>,
    cfg: NetConfig,
) -> CmdResult<()> {
    if scan.is_running() {
        return Err(crate::autoscan::ERR_RUNNING.to_string().into());
    }
    state.send(Command::Apply(cfg)).await
}

#[tauri::command]
pub async fn keep_now(state: State<'_, AppState>) -> CmdResult<()> {
    state.send(Command::KeepNow).await
}

#[tauri::command]
pub async fn revert_now(state: State<'_, AppState>) -> CmdResult<()> {
    state.send(Command::RevertNow).await
}

#[tauri::command]
pub async fn request_reboot(state: State<'_, AppState>) -> CmdResult<RebootTicket> {
    let (tx, rx) = oneshot::channel();
    state.send(Command::RequestReboot(tx)).await?;
    rx.await
        .map_err(|_| CmdError::Message("router service stopped".into()))?
        .map_err(Into::into)
}

#[tauri::command]
pub async fn confirm_reboot(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let (tx, rx) = oneshot::channel();
    state.send(Command::ConfirmReboot(id, tx)).await?;
    rx.await
        .map_err(|_| CmdError::Message("router service stopped".into()))?
        .map_err(Into::into)
}

#[tauri::command]
pub async fn request_factory_reset(state: State<'_, AppState>) -> CmdResult<RebootTicket> {
    let (tx, rx) = oneshot::channel();
    state.send(Command::RequestFactoryReset(tx)).await?;
    rx.await
        .map_err(|_| CmdError::Message("router service stopped".into()))?
        .map_err(Into::into)
}

#[tauri::command]
pub async fn confirm_factory_reset(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let (tx, rx) = oneshot::channel();
    state.send(Command::ConfirmFactoryReset(id, tx)).await?;
    rx.await
        .map_err(|_| CmdError::Message("router service stopped".into()))?
        .map_err(Into::into)
}

#[tauri::command]
pub async fn set_visible(state: State<'_, AppState>, v: bool) -> CmdResult<()> {
    state.set_window_visible(v);
    state.send(Command::SetVisible(v)).await
}

/// The About tab's developer link. The URL is fixed here and the command takes no input, so the
/// webview needs no opener permission and can't open anything else.
pub const DEVELOPER_URL: &str = "https://creators.sa/hibiki";

#[tauri::command]
pub fn open_developer_page(app: AppHandle) -> CmdResult<()> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(DEVELOPER_URL, None::<&str>)
        .map_err(|e| e.to_string().into())
}

/// Builds the redacted diagnostics file (read-only router GETs plus the event-log tail) and
/// keeps it for `save_diagnostics`. The UI shows the returned text before anything is written.
/// Refused while a band change is open: the router is re-attaching then.
#[tauri::command]
pub async fn prepare_diagnostics(state: State<'_, AppState>) -> CmdResult<String> {
    let (driver, phase) = {
        let g = state.inner.lock().unwrap();
        let phase = g.handle.snapshots.borrow().safe_apply.phase.clone();
        (g.driver.clone(), phase)
    };
    if !matches!(
        phase,
        Phase::Idle | Phase::Committed | Phase::Reverted { .. }
    ) {
        return Err("Finish or revert the current band change first."
            .to_string()
            .into());
    }
    let reads = driver.diagnostic_reads().await;
    let log = state.data_dir.join("events.jsonl");
    let events = blocking(move || match std::fs::read_to_string(&log) {
        Ok(t) => Ok(router_core::diagnostics::parse_events(&t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.to_string()),
    })
    .await?;
    let info = driver.info();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let text = router_core::diagnostics::build_report(
        &state.app.package_info().version.to_string(),
        info.id,
        info.name,
        now_ms,
        &reads,
        &events,
    )
    .map_err(|leaks| {
        format!(
            "Export stopped: the privacy check found {}. Nothing was saved.",
            leaks.join(", ")
        )
    })?;
    *state.diagnostics.lock().unwrap() = Some(text.clone());
    Ok(text)
}

/// Asks where to save the prepared diagnostics and writes them. `false` when cancelled.
#[tauri::command]
pub async fn save_diagnostics(state: State<'_, AppState>) -> CmdResult<bool> {
    use tauri_plugin_dialog::DialogExt;
    let Some(text) = state.diagnostics.lock().unwrap().clone() else {
        return Err("Prepare the diagnostics first.".to_string().into());
    };
    let app = state.app.clone();
    let saved = blocking(move || {
        let Some(target) = app
            .dialog()
            .file()
            .set_file_name("router-band-tuner-diagnostics.json")
            .add_filter("JSON", &["json"])
            .blocking_save_file()
        else {
            return Ok(false);
        };
        let path = target.into_path().map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| e.to_string())?;
        Ok(true)
    })
    .await?;
    Ok(saved)
}

/// One file "Save router page scripts" left out, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PageScriptSkip {
    pub path: String,
    pub reason: PageScriptSkipReason,
}

/// Why a page script was left out; the UI words it (`pageScripts.reason.*`), and the manifest
/// keeps the English from `SkipKind::text`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PageScriptSkipReason {
    Timeout,
    Unreachable,
    Refused,
    TooLarge,
    TooLargeUnpacked,
    BadGzip,
    OverTotal,
    StillFlagged,
    NotFollowed,
    Other,
}

impl From<&router_core::page_scripts::SkipKind> for PageScriptSkipReason {
    fn from(k: &router_core::page_scripts::SkipKind) -> Self {
        use router_core::page_scripts::SkipKind as K;
        match k {
            K::Timeout => Self::Timeout,
            K::Unreachable => Self::Unreachable,
            K::Refused => Self::Refused,
            K::TooLarge => Self::TooLarge,
            K::TooLargeUnpacked => Self::TooLargeUnpacked,
            K::BadGzip => Self::BadGzip,
            K::OverTotal => Self::OverTotal,
            K::StillFlagged => Self::StillFlagged,
            K::NotFollowed => Self::NotFollowed,
            K::Other(_) => Self::Other,
        }
    }
}

/// What a prepared page-script zip holds, shown before it is saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PageScriptsSummary {
    pub files: u32,
    /// All saved files together, unpacked.
    #[ts(type = "number")]
    pub bytes: u64,
    /// The zip's own size.
    #[ts(type = "number")]
    pub zip_bytes: u64,
    pub skipped: Vec<PageScriptSkip>,
    pub missing: Vec<String>,
    /// The limit that ended the run early.
    #[ts(optional = nullable)]
    pub stopped: Option<String>,
}

const SCRIPTS_NOT_LOGGED_IN: &str = "Log in to the router first.";
const SCRIPTS_ROUTER_BUSY: &str =
    "Wait until the current band change, Refresh IP, APN change, auto-scan or speed test has finished.";
const SCRIPTS_RUNNING: &str = "The router's page scripts are already being read.";

/// Why the page-script saver can't start from this snapshot: it needs the app's session, and
/// nothing may be holding the router (a speed test included).
fn page_scripts_refusal(
    s: &AppSnapshot,
    scan_running: bool,
    speed_test_running: bool,
) -> Option<&'static str> {
    if !matches!(s.session, router_core::SessionView::LoggedIn) {
        Some(SCRIPTS_NOT_LOGGED_IN)
    } else if scan_running
        || speed_test_running
        || restart_refusal(s).is_some()
        || s.ip_reattach.as_ref().is_some_and(|r| r.active)
    {
        Some(SCRIPTS_ROUTER_BUSY)
    } else {
        None
    }
}

/// `http://host` for a bare host from the settings.
fn router_origin(host: &str) -> String {
    let host = host.trim();
    if host.starts_with("http://") || host.starts_with("https://") {
        host.to_string()
    } else {
        format!("http://{host}")
    }
}

/// The zip's `MANIFEST.txt`: versions, then every path with its size, whether it was a gzip
/// body and how many values were redacted, then what was left out. No identifiers.
fn page_scripts_manifest(
    app_version: &str,
    driver_id: &str,
    firmware: &str,
    bundle: &router_core::page_scripts::ScriptBundle,
) -> String {
    let mut lines =
        vec![
        "5G Router Band Tuner: router page scripts".to_string(),
        format!("App version: {app_version}"),
        format!("Driver: {driver_id}"),
        format!("Firmware: {firmware}"),
        format!(
            "Read-only: the app only downloaded these files with GETs, starting at {}.",
            router_core::page_scripts::START_PAGE
        ),
        "Redaction: CSRF token values are blanked; IMEI- and ICCID-length numbers and IP and MAC \
         addresses are replaced with [redacted-<kind>]."
            .to_string(),
        "This is the router maker's code. Keep it private: don't commit it or post it publicly."
            .to_string(),
        String::new(),
        format!("Files ({}): path, bytes, gzip, redactions", bundle.files.len()),
    ];
    for f in &bundle.files {
        lines.push(format!(
            "{}\t{}\t{}\t{}{}",
            f.path,
            f.bytes.len(),
            if f.gunzipped { "gzip" } else { "-" },
            f.redactions,
            if f.not_utf8 { "\tnot UTF-8" } else { "" }
        ));
    }
    lines.push(String::new());
    lines.push(format!("Skipped ({}): path, reason", bundle.skipped.len()));
    for (p, why) in &bundle.skipped {
        lines.push(format!("{p}\t{}", why.text()));
    }
    lines.push(String::new());
    lines.push(format!(
        "Missing ({}): the router answered with its error page",
        bundle.missing.len()
    ));
    lines.extend(bundle.missing.iter().cloned());
    if let Some(why) = &bundle.stopped {
        lines.push(String::new());
        lines.push(format!("Stopped early at {why}."));
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// The files under their router paths (without the leading `/`) plus `MANIFEST.txt`, deflated.
fn page_scripts_zip(
    bundle: &router_core::page_scripts::ScriptBundle,
    manifest: &str,
) -> Result<Vec<u8>, String> {
    use std::io::Write as _;
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    w.start_file("MANIFEST.txt", opts)
        .map_err(|e| e.to_string())?;
    w.write_all(manifest.as_bytes())
        .map_err(|e| e.to_string())?;
    for f in &bundle.files {
        // Validated by the saver already: `[A-Za-z0-9_./-]`, no `..`, under /js/, /lib/ or /html/.
        if !router_core::page_scripts::is_allowed_path(&f.path) {
            continue;
        }
        w.start_file(f.path.trim_start_matches('/'), opts)
            .map_err(|e| e.to_string())?;
        w.write_all(&f.bytes).map_err(|e| e.to_string())?;
    }
    Ok(w.finish().map_err(|e| e.to_string())?.into_inner())
}

/// Clears the busy flag of `prepare_page_scripts` however it ends, and tells the actor when it
/// had granted the read (`BeginPageScripts`).
struct BusyGuard<'a> {
    flag: &'a AtomicBool,
    state: &'a AppState,
    granted: bool,
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        if self.granted {
            self.state.send_now(Command::EndPageScripts);
        }
        self.flag.store(false, Ordering::SeqCst);
    }
}

/// Reads the router's own web page code with the app's session (GETs only, see
/// `router_core::page_scripts`) and builds the zip in memory for `save_page_scripts`. Refused
/// unless logged in with nothing holding the router. While it reads, the actor holds back band
/// changes, Refresh IP, APN changes, reboots and logouts, and no speed test or auto-scan starts.
/// Returns what the zip holds.
#[tauri::command]
pub async fn prepare_page_scripts(
    state: State<'_, AppState>,
    scan: State<'_, crate::autoscan::ScanState>,
    st: State<'_, crate::speedtest::SpeedTestState>,
) -> CmdResult<PageScriptsSummary> {
    let (driver, snapshot, host) = {
        let g = state.inner.lock().unwrap();
        let snapshot = g.handle.snapshots.borrow().clone();
        (g.driver.clone(), snapshot, g.settings.host.clone())
    };
    if let Some(why) = page_scripts_refusal(&snapshot, scan.is_running(), st.is_running()) {
        return Err(why.to_string().into());
    }
    if state.page_scripts_busy.swap(true, Ordering::SeqCst) {
        return Err(SCRIPTS_RUNNING.to_string().into());
    }
    let mut busy = BusyGuard {
        flag: &state.page_scripts_busy,
        state: state.inner(),
        granted: false,
    };
    // A speed test or scan that started between the check and the flag.
    if scan.is_running() || st.is_running() {
        return Err(SCRIPTS_ROUTER_BUSY.to_string().into());
    }
    // The actor's own check, made where band changes, Refresh IP and the rest start, so none
    // of them can slip in after the snapshot; once granted, they are refused until the end.
    let (tx, rx) = oneshot::channel();
    state.send(Command::BeginPageScripts(tx)).await?;
    match rx.await {
        Ok(Ok(())) => busy.granted = true,
        Ok(Err(RouterError::Validation { message })) if message == router_core::NOT_CONNECTED => {
            return Err(SCRIPTS_NOT_LOGGED_IN.to_string().into());
        }
        Ok(Err(_)) => return Err(SCRIPTS_ROUTER_BUSY.to_string().into()),
        Err(_) => return Err(CmdError::Message("router service stopped".into())),
    }
    *state.page_scripts.lock().unwrap() = None;
    let bundle = router_core::page_scripts::collect(
        driver.as_ref(),
        &router_origin(&host),
        &router_core::page_scripts::Limits::default(),
    )
    .await?;
    let firmware = snapshot
        .device
        .as_ref()
        .map(|d| d.firmware.clone())
        .unwrap_or_default();
    let manifest = page_scripts_manifest(
        &state.app.package_info().version.to_string(),
        driver.info().id,
        &firmware,
        &bundle,
    );
    let summary = PageScriptsSummary {
        files: bundle.files.len() as u32,
        bytes: bundle.total_bytes() as u64,
        zip_bytes: 0,
        skipped: bundle
            .skipped
            .iter()
            .map(|(path, why)| PageScriptSkip {
                path: path.clone(),
                reason: PageScriptSkipReason::from(why),
            })
            .collect(),
        missing: bundle.missing.clone(),
        stopped: bundle.stopped.clone(),
    };
    let zip = blocking(move || page_scripts_zip(&bundle, &manifest)).await?;
    let summary = PageScriptsSummary {
        zip_bytes: zip.len() as u64,
        ..summary
    };
    *state.page_scripts.lock().unwrap() = Some(PreparedScripts {
        zip,
        summary: summary.clone(),
    });
    Ok(summary)
}

/// Asks where to save the prepared page scripts and writes the zip. `false` when cancelled.
/// Logs `diagnostics.page_scripts_saved` with the counts only.
#[tauri::command]
pub async fn save_page_scripts(state: State<'_, AppState>) -> CmdResult<bool> {
    use tauri_plugin_dialog::DialogExt;
    let prepared = state
        .page_scripts
        .lock()
        .unwrap()
        .as_ref()
        .map(|p| (p.zip.clone(), p.summary.clone()));
    let Some((zip, summary)) = prepared else {
        return Err("Read the router's page scripts first.".to_string().into());
    };
    let app = state.app.clone();
    let saved = blocking(move || {
        let Some(target) = app
            .dialog()
            .file()
            .set_file_name("router-page-scripts.zip")
            .add_filter("ZIP", &["zip"])
            .blocking_save_file()
        else {
            return Ok(false);
        };
        let path = target.into_path().map_err(|e| e.to_string())?;
        std::fs::write(path, zip).map_err(|e| e.to_string())?;
        Ok(true)
    })
    .await?;
    if saved {
        *state.page_scripts.lock().unwrap() = None;
        let dir = state.data_dir.clone();
        let rec = router_core::EventRecord {
            at_ms: crate::notify::now_ms(),
            kind: router_core::event_text::kinds::DIAGNOSTICS_PAGE_SCRIPTS_SAVED.into(),
            detail: serde_json::json!({
                "files": summary.files,
                "bytes": summary.bytes,
                "skipped": summary.skipped.len(),
            }),
        };
        // A failed append only loses the log line; the file is saved.
        let _ = blocking(move || {
            router_core::EventLog::new(dir)
                .append(&rec)
                .map_err(|e| e.to_string())
        })
        .await;
    }
    Ok(saved)
}

// Speed test: read-only accessors for `crate::speedtest`.
impl AppState {
    pub(crate) fn speed_test_settings(&self) -> crate::settings::SpeedTestSettings {
        self.inner.lock().unwrap().settings.speed_test
    }

    /// Retention and automatic speed tests.
    pub(crate) fn history_settings(&self) -> crate::settings::HistorySettings {
        self.inner.lock().unwrap().settings.history
    }

    /// Whether the main window is shown (not hidden to the tray).
    pub(crate) fn is_visible(&self) -> bool {
        self.visible.load(Ordering::Relaxed)
    }

    /// Records the window's visibility; callers then tell the actor.
    pub(crate) fn set_window_visible(&self, v: bool) {
        self.visible.store(v, Ordering::Relaxed);
    }

    pub(crate) fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }
}

// Privacy check: what `crate::privacy_check` needs.
impl AppState {
    /// The driver and the live snapshot, taken together.
    pub(crate) fn driver_and_snapshot(&self) -> (Arc<dyn RouterDriver>, AppSnapshot) {
        let g = self.inner.lock().unwrap();
        let snapshot = g.handle.snapshots.borrow().clone();
        (g.driver.clone(), snapshot)
    }

    /// The demo driver is selected, so nothing outside the app is contacted for its screens.
    pub(crate) fn is_demo(&self) -> bool {
        self.inner.lock().unwrap().settings.driver == DriverId::Mock
    }

    /// `prepare_page_scripts` is reading the router.
    pub(crate) fn page_scripts_running(&self) -> bool {
        self.page_scripts_busy.load(Ordering::SeqCst)
    }
}

/// Why an extra read of the router can't start now: it needs the app's session, and nothing may
/// be holding the router (a speed test, a scan and the page-script saver included).
pub(crate) fn router_busy_refusal(
    s: &AppSnapshot,
    scan_running: bool,
    speed_test_running: bool,
    page_scripts_running: bool,
) -> Option<&'static str> {
    match page_scripts_refusal(s, scan_running, speed_test_running) {
        None if page_scripts_running => Some(SCRIPTS_RUNNING),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A router or driver change waits for every open change, so none loses its undo.
    #[test]
    fn a_router_change_waits_for_every_open_change() {
        let base = crate::tray::tests::snap();
        assert_eq!(restart_refusal(&base), None);
        let mut band = base.clone();
        band.safe_apply.phase = Phase::Probation;
        assert_eq!(restart_refusal(&band), Some(RESTART_BAND_CHANGE));
        let mut seq = base.clone();
        seq.safe_apply.sequence = Some(router_core::ApplySequenceView {
            step: 1,
            total: 2,
            primary: 3,
            final_config: NetConfig {
                network_mode: router_core::NetworkMode::Auto,
                lte_bands: router_core::LteBandSet::AUTO,
                legacy_band_raw: "0".to_string(),
                nr_option: None,
            },
            origin: router_core::SequenceOrigin::User,
            step1: None,
        });
        assert_eq!(restart_refusal(&seq), Some(RESTART_BAND_CHANGE));
        let mut ip = base.clone();
        ip.ip_refresh = Some(crate::speedtest::running_ip_refresh());
        assert_eq!(restart_refusal(&ip), Some(RESTART_IP_REFRESH));
        let mut apn = base.clone();
        apn.apn_apply = Some(crate::speedtest::apn_change(Phase::ReattachGrace));
        assert_eq!(restart_refusal(&apn), Some(RESTART_APN_CHANGE));
        apn.apn_apply = Some(crate::speedtest::apn_change(Phase::Committed));
        assert_eq!(restart_refusal(&apn), None);
    }

    /// C2: launch connects by itself when a password is saved, or always for the demo driver.
    #[test]
    fn startup_password_rules() {
        assert_eq!(
            connect_password(Some("pw".into()), DriverId::HuaweiH122).as_deref(),
            Some("pw")
        );
        assert_eq!(connect_password(None, DriverId::HuaweiH122), None);
        assert_eq!(
            connect_password(None, DriverId::Mock).as_deref(),
            Some(MOCK_PASSWORD)
        );
        assert_eq!(
            connect_password(Some("saved".into()), DriverId::Mock).as_deref(),
            Some("saved")
        );
    }

    /// An in-memory keychain for tests.
    #[derive(Default)]
    struct MemStore(Mutex<std::collections::HashMap<String, String>>);

    impl CredentialStore for MemStore {
        fn get(&self, host: &str, user: &str) -> Result<Option<String>, String> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .get(&format!("{host}/{user}"))
                .cloned())
        }
        fn set(&self, host: &str, user: &str, pw: &str) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .insert(format!("{host}/{user}"), pw.to_string());
            Ok(())
        }
        fn delete(&self, host: &str, user: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(&format!("{host}/{user}"));
            Ok(())
        }
    }

    #[test]
    fn credential_status_reports_saved_and_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemStore::default();
        let (h, u) = ("192.168.8.1", "admin");
        let status = |driver| credential_status_of(&store, dir.path(), h, u, driver).unwrap();
        let st = |saved, rejected| CredentialStatus { saved, rejected };

        assert_eq!(status(DriverId::HuaweiH122), st(false, false));
        store.set(h, u, "pw").unwrap();
        assert_eq!(status(DriverId::HuaweiH122), st(true, false));
        password_marker::mark(dir.path(), h, u).unwrap();
        assert_eq!(status(DriverId::HuaweiH122), st(true, true));
        // The marker names one account; another username has nothing saved or rejected.
        assert_eq!(
            credential_status_of(&store, dir.path(), h, "other", DriverId::HuaweiH122).unwrap(),
            st(false, false)
        );
        // The demo driver always has its fixed password and is never marked.
        assert_eq!(status(DriverId::Mock), st(true, false));
        store.delete(h, u).unwrap();
        password_marker::clear(dir.path());
        assert_eq!(status(DriverId::Mock), st(true, false));
    }

    #[test]
    fn page_scripts_need_a_session_and_an_idle_router() {
        let mut base = crate::tray::tests::snap();
        base.session = router_core::SessionView::LoggedIn;
        assert_eq!(page_scripts_refusal(&base, false, false), None);
        assert_eq!(
            page_scripts_refusal(&base, true, false),
            Some(SCRIPTS_ROUTER_BUSY)
        );
        assert_eq!(
            page_scripts_refusal(&base, false, true),
            Some(SCRIPTS_ROUTER_BUSY)
        );
        let mut out = base.clone();
        out.session = router_core::SessionView::LoggedOut;
        assert_eq!(
            page_scripts_refusal(&out, false, false),
            Some(SCRIPTS_NOT_LOGGED_IN)
        );
        let mut band = base.clone();
        band.safe_apply.phase = Phase::Probation;
        assert_eq!(
            page_scripts_refusal(&band, false, false),
            Some(SCRIPTS_ROUTER_BUSY)
        );
        let mut ip = base.clone();
        ip.ip_refresh = Some(crate::speedtest::running_ip_refresh());
        assert_eq!(
            page_scripts_refusal(&ip, false, false),
            Some(SCRIPTS_ROUTER_BUSY)
        );
        let mut apn = base.clone();
        apn.apn_apply = Some(crate::speedtest::apn_change(Phase::ReattachGrace));
        assert_eq!(
            page_scripts_refusal(&apn, false, false),
            Some(SCRIPTS_ROUTER_BUSY)
        );
        let mut re = base.clone();
        re.ip_reattach = Some(router_core::IpReattachView {
            active: true,
            from_range: None,
            last: None,
        });
        assert_eq!(
            page_scripts_refusal(&re, false, false),
            Some(SCRIPTS_ROUTER_BUSY)
        );
    }

    #[test]
    fn an_extra_router_read_waits_for_the_page_script_saver_too() {
        let mut base = crate::tray::tests::snap();
        base.session = router_core::SessionView::LoggedIn;
        assert_eq!(router_busy_refusal(&base, false, false, false), None);
        assert_eq!(
            router_busy_refusal(&base, false, false, true),
            Some(SCRIPTS_RUNNING)
        );
        assert_eq!(
            router_busy_refusal(&base, false, true, false),
            Some(SCRIPTS_ROUTER_BUSY)
        );
        let mut out = base.clone();
        out.session = router_core::SessionView::LoggedOut;
        // Logged out is the reason given, whatever else is going on.
        assert_eq!(
            router_busy_refusal(&out, false, false, true),
            Some(SCRIPTS_NOT_LOGGED_IN)
        );
    }

    #[test]
    fn the_router_origin_gets_a_scheme() {
        assert_eq!(router_origin("192.168.8.1"), "http://192.168.8.1");
        assert_eq!(router_origin(" http://10.0.0.1 "), "http://10.0.0.1");
    }

    #[tokio::test(start_paused = true)]
    async fn the_page_script_zip_holds_the_files_and_a_manifest() {
        use std::io::Read as _;
        let m = MockDriver::new(MockScenario::zain_like());
        m.login(&Credentials::new("admin", MOCK_PASSWORD))
            .await
            .unwrap();
        let limits = router_core::page_scripts::Limits {
            gap: Duration::ZERO,
            ..Default::default()
        };
        let bundle = router_core::page_scripts::collect(&m, "http://192.168.8.1", &limits)
            .await
            .unwrap();
        let manifest = page_scripts_manifest("1.2.3", "mock", "0.0.0.0(mock)", &bundle);
        assert!(manifest.contains("Firmware: 0.0.0.0(mock)"));
        assert!(manifest.contains("/js/main.js\t"));
        assert!(manifest.contains("/lib/mock-lib.js\t"));
        assert!(manifest.contains("\tgzip\t"));
        assert!(manifest.contains("\n/js/missing.js\n"));
        assert!(manifest.contains("/js/hidden.js\tan identifier"));
        let zip = page_scripts_zip(&bundle, &manifest).unwrap();
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(zip)).unwrap();
        let mut names: Vec<String> = z.file_names().map(|n| n.unwrap().into_owned()).collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "MANIFEST.txt",
                "html/content.html",
                "html/settings.html",
                "js/deep.js",
                "js/main.js",
                "js/settings.js",
                "lib/mock-lib.js",
            ]
        );
        let mut main = String::new();
        z.by_name("js/main.js")
            .unwrap()
            .read_to_string(&mut main)
            .unwrap();
        assert!(main.contains("[redacted-number]"));
        assert_eq!(m.writes(), 0);
    }

    #[test]
    fn relock_is_never_armed_without_a_primary_band() {
        let a = AutomationSettings {
            primary_band: None,
            relock_primary: true,
            revert_on_loss: true,
        };
        assert!(!automation(&a).relock_primary);
        let a = AutomationSettings {
            primary_band: Some(3),
            ..a
        };
        assert!(automation(&a).relock_primary);
    }
}
