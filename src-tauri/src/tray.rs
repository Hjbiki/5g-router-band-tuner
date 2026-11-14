//! Tray state mapping, status line, runtime-drawn icons and the tray itself.
//! The tray keeps the auto-revert watchdog visible while the window is closed.

use std::sync::Mutex;

use router_core::{AppSnapshot, Command, Phase, SessionView};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use crate::commands::AppState;
use crate::notify::{self, tr};
use crate::settings::Language;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Grey,
    Green,
    Amber,
    Red,
}

/// Safety first: a pending or failed revert, or an active change without service, is Red even
/// when no login is configured (a resumed journal with no saved password). Grey only means
/// "not set up, or logged out, and nothing is being watched".
pub fn tray_state(s: &AppSnapshot) -> TrayState {
    let phase = &s.safe_apply.phase;
    let apn = s.apn_apply.as_ref().map(|v| &v.phase);
    let closed = quit_is_safe(s);
    let failed = [Some(phase), apn]
        .into_iter()
        .flatten()
        .any(|p| matches!(p, Phase::RevertFailed | Phase::RevertPending { .. }));
    if !failed && crate::speedtest::ip_refresh_active(s) {
        // Data is off on purpose for a moment.
        TrayState::Amber
    } else if failed || (!closed && !s.in_service) {
        TrayState::Red
    } else if matches!(
        s.session,
        SessionView::NotConfigured | SessionView::LoggedOut
    ) && closed
    {
        TrayState::Grey
    } else if !s.in_service {
        TrayState::Red
    } else if [Some(phase), apn].into_iter().flatten().any(|p| {
        matches!(
            p,
            Phase::ReattachGrace | Phase::Probation | Phase::Reverting { .. }
        )
    }) || ip_reattach_active(s)
        || matches!(
            s.session,
            SessionView::Reconnecting { .. } | SessionView::TakenOver
        )
    {
        TrayState::Amber
    } else {
        TrayState::Green
    }
}

/// `"4G 3/5 \u{b7} 5G 4/5"`: the router's signal bars, for whichever links report them.
fn bars_text(s: &AppSnapshot) -> Option<String> {
    let status = s.status.as_ref()?;
    let parts: Vec<String> = [("4G", status.bars), ("5G", status.nr_bars)]
        .into_iter()
        .filter_map(|(rat, bars)| bars.map(|b| format!("{rat} {}/5", b.min(5))))
        .collect();
    (!parts.is_empty()).then(|| parts.join(" \u{b7} "))
}

/// `"B3 + n78 \u{b7} SINR 3 dB \u{b7} 4G 3/5 \u{b7} 5G 4/5"`; `"\u{2014}"` when there is no signal.
pub fn tray_status_line(s: &AppSnapshot, lang: Language) -> String {
    let line = signal_line(s, lang);
    match bars_text(s) {
        Some(bars) if line != "\u{2014}" => format!("{line} \u{b7} {bars}"),
        _ => line,
    }
}

/// The status line, with a note while an automatic speed test runs.
pub fn tray_line(s: &AppSnapshot, lang: Language, auto_test: bool) -> String {
    let line = tray_status_line(s, lang);
    if auto_test {
        format!("{line} \u{b7} {}", tr(lang, "tray.autoTestRunning"))
    } else {
        line
    }
}

fn signal_line(s: &AppSnapshot, _lang: Language) -> String {
    let Some(sig) = &s.signal else {
        return "\u{2014}".into();
    };
    let lte = sig
        .lte
        .as_ref()
        .and_then(|l| l.band)
        .map(|b| format!("B{b}"));
    let nr = sig
        .nr
        .as_ref()
        .and_then(|n| n.band.as_ref())
        .map(|b| format!("n{}", b.primary));
    let bands = match (lte, nr) {
        (Some(l), Some(n)) => format!("{l} + {n}"),
        (Some(x), None) | (None, Some(x)) => x,
        (None, None) => return "\u{2014}".into(),
    };
    let sinr = sig
        .lte
        .as_ref()
        .and_then(|l| l.sinr)
        .or_else(|| sig.nr.as_ref().and_then(|n| n.sinr));
    match sinr {
        Some(v) => format!("{bands} \u{b7} SINR {} dB", v.round() as i32),
        None => bands,
    }
}

fn rgb(s: TrayState) -> [u8; 3] {
    match s {
        TrayState::Grey => [0x8A, 0x8F, 0x98],
        TrayState::Green => [0x2E, 0x9D, 0x5B],
        TrayState::Amber => [0xE0, 0xA1, 0x00],
        TrayState::Red => [0xD9, 0x3C, 0x3C],
    }
}

const ICON: u32 = 32;

/// A filled circle on a transparent 32x32 RGBA canvas (1 px anti-aliased edge).
pub fn icon_rgba(s: TrayState) -> Vec<u8> {
    let [r, g, b] = rgb(s);
    let c = (ICON as f32 - 1.0) / 2.0;
    let radius = ICON as f32 / 2.0 - 1.5;
    let mut out = Vec::with_capacity((ICON * ICON * 4) as usize);
    for y in 0..ICON {
        for x in 0..ICON {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            let a = (radius + 0.5 - d).clamp(0.0, 1.0);
            out.extend_from_slice(&[r, g, b, (a * 255.0).round() as u8]);
        }
    }
    out
}

fn icon(s: TrayState) -> Image<'static> {
    Image::new_owned(icon_rgba(s), ICON, ICON)
}

/// Whether the menu offers "Revert now" (only while a band change or an APN switch is being
/// watched; the actor sends it to whichever is open).
pub fn show_revert(s: &AppSnapshot) -> bool {
    let watched = |p: &Phase| matches!(p, Phase::ReattachGrace | Phase::Probation);
    watched(&s.safe_apply.phase) || s.apn_apply.as_ref().is_some_and(|v| watched(&v.phase))
}

/// A band switch for a new IP is open, between its steps too (the router may be on the
/// temporary bands).
fn ip_reattach_active(s: &AppSnapshot) -> bool {
    s.ip_reattach.as_ref().is_some_and(|v| v.active)
}

/// Nothing the app must finish: no band change, no APN switch, no band switch for a new IP and
/// no Refresh IP run, which may have mobile data off.
fn quit_is_safe(s: &AppSnapshot) -> bool {
    matches!(
        s.safe_apply.phase,
        Phase::Idle | Phase::Committed | Phase::Reverted { .. }
    ) && !crate::speedtest::ip_refresh_active(s)
        && !crate::speedtest::apn_change_open(s)
        && !ip_reattach_active(s)
}

#[derive(Clone, PartialEq)]
struct View {
    state: TrayState,
    line: String,
    revert: bool,
    lang: Language,
}

/// Managed state: the tray icon plus what it currently shows.
pub struct TrayCtl {
    tray: TrayIcon,
    shown: Mutex<Option<View>>,
    pending: Mutex<notify::PendingWatch>,
}

impl TrayCtl {
    pub fn watch(&self) -> std::sync::MutexGuard<'_, notify::PendingWatch> {
        self.pending.lock().unwrap()
    }
}

fn build_menu(app: &AppHandle, v: &View) -> tauri::Result<Menu<tauri::Wry>> {
    let open = MenuItem::with_id(app, "open", tr(v.lang, "tray.open"), true, None::<&str>)?;
    let status = MenuItem::with_id(app, "status", &v.line, false, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", tr(v.lang, "tray.quit"), true, None::<&str>)?;
    if v.revert {
        let revert = MenuItem::with_id(
            app,
            "revert",
            tr(v.lang, "tray.revertNow"),
            true,
            None::<&str>,
        )?;
        Menu::with_items(app, &[&open, &status, &revert, &sep, &quit])
    } else {
        Menu::with_items(app, &[&open, &status, &sep, &quit])
    }
}

/// Show and focus the main window and tell the actor it is visible (1 s polling).
pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    set_visible(app, true);
}

fn set_visible(app: &AppHandle, v: bool) {
    if let Some(state) = app.try_state::<AppState>() {
        state.set_window_visible(v);
        state.send_detached(Command::SetVisible(v));
    }
}

/// Window close: keep running in the tray (the watchdog must stay alive).
pub fn hide_to_tray(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
    set_visible(app, false);
    if let Some(state) = app.try_state::<AppState>() {
        if let Some(lang) = state.take_tray_notice() {
            notify::show(app, "notify.trayNotice", tr(lang, "notify.trayNotice"));
        }
    }
}

/// What the quit warning says: a Refresh IP run may leave mobile data off, an APN switch and a
/// band change are only undone at the next launch.
fn quit_warning(s: &AppSnapshot) -> &'static str {
    if crate::speedtest::ip_refresh_active(s) {
        "tray.quitConfirmData"
    } else if crate::speedtest::apn_change_open(s) {
        "tray.quitConfirmApn"
    } else {
        "tray.quitConfirm"
    }
}

/// Quit, asking first while a change is still being watched. Exiting keeps the journal, so
/// the next launch resumes the revert; during a Refresh IP run the exit also tries to turn
/// mobile data back on (`AppState::finish_ip_refresh_at_exit`).
fn request_quit(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        app.exit(0);
        return;
    };
    let snapshot = state.snapshot();
    if quit_is_safe(&snapshot) {
        app.exit(0);
        return;
    }
    let lang = state.language();
    let a = app.clone();
    app.dialog()
        .message(tr(lang, quit_warning(&snapshot)))
        .title("5G Router Band Tuner")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            tr(lang, "tray.quit").into(),
            tr(lang, "tray.cancel").into(),
        ))
        .show(move |confirmed| {
            if confirmed {
                a.exit(0);
            }
        });
}

/// Create the tray icon. Call from `setup` after `AppState` is managed.
pub fn init(app: &AppHandle) -> tauri::Result<()> {
    let lang = app.state::<AppState>().language();
    let v = View {
        state: TrayState::Grey,
        line: "\u{2014}".into(),
        revert: false,
        lang,
    };
    let menu = build_menu(app, &v)?;
    let tray = TrayIconBuilder::with_id("main")
        .icon(icon(v.state))
        .tooltip(format!("5G Router Band Tuner \u{b7} {}", v.line))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            "open" => show_main(app),
            "revert" => {
                if let Some(state) = app.try_state::<AppState>() {
                    state.send_detached(Command::RevertNow);
                }
            }
            "quit" => request_quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    app.manage(TrayCtl {
        tray,
        shown: Mutex::new(Some(v)),
        pending: Mutex::new(notify::PendingWatch::default()),
    });
    Ok(())
}

/// Called for every forwarded snapshot: refresh the tray and fire the
/// "close the router's web page" notification once per RevertPending occurrence.
pub fn on_snapshot(app: &AppHandle, snap: &AppSnapshot) {
    let Some(ctl) = app.try_state::<TrayCtl>() else {
        return;
    };
    let lang = app
        .try_state::<AppState>()
        .map_or(Language::En, |s| s.language());
    if ctl.pending.lock().unwrap().observe(snap) {
        notify::show(app, "notify.closeWebUi", tr(lang, "notify.closeWebUi"));
    }
    let auto_test = app
        .try_state::<crate::speedtest::schedule::AutoTestState>()
        .is_some_and(|s| s.is_running());
    let v = View {
        state: tray_state(snap),
        line: tray_line(snap, lang, auto_test),
        revert: show_revert(snap),
        lang,
    };
    let mut shown = ctl.shown.lock().unwrap();
    if shown.as_ref() == Some(&v) {
        return;
    }
    let old = shown.replace(v.clone());
    if old.as_ref().map(|o| o.state) != Some(v.state) {
        let _ = ctl.tray.set_icon(Some(icon(v.state)));
    }
    let _ = ctl
        .tray
        .set_tooltip(Some(format!("5G Router Band Tuner \u{b7} {}", v.line)));
    if let Ok(menu) = build_menu(app, &v) {
        let _ = ctl.tray.set_menu(Some(menu));
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use router_core::Duplex;
    use router_core::{LteSignal, NrBandInfo, NrSignal, RevertReason, SafeApplyView, Signal};

    /// A logged-in, in-service snapshot with nothing open, for tests elsewhere in the app too.
    pub(crate) fn snap() -> AppSnapshot {
        AppSnapshot {
            session: SessionView::LoggedIn,
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
            in_service: true,
            foreign_journal: None,
            captured_at_ms: 0,
            relock: None,
            wan: None,
            ip_refresh: None,
            data_switch_off: false,
            apn: None,
            apn_apply: None,
            ip_reattach: None,
        }
    }

    fn with_phase(p: Phase) -> AppSnapshot {
        let mut s = snap();
        s.safe_apply.phase = p;
        s
    }

    #[test]
    fn an_apn_switch_is_watched_like_a_band_change() {
        let mut s = snap();
        s.apn_apply = Some(crate::speedtest::apn_change(Phase::Probation));
        assert!(!quit_is_safe(&s));
        assert!(show_revert(&s));
        assert_eq!(tray_state(&s), TrayState::Amber);
        s.apn_apply = Some(crate::speedtest::apn_change(Phase::RevertFailed));
        assert_eq!(tray_state(&s), TrayState::Red);
        assert!(!show_revert(&s));
        s.apn_apply = Some(crate::speedtest::apn_change(Phase::Committed));
        assert!(quit_is_safe(&s));
        assert!(!show_revert(&s));
    }

    #[test]
    fn a_refresh_ip_run_is_amber_and_needs_a_quit_warning() {
        let mut s = snap();
        assert!(quit_is_safe(&s));
        s.ip_refresh = Some(crate::speedtest::running_ip_refresh());
        s.in_service = false;
        assert!(!quit_is_safe(&s));
        assert_eq!(tray_state(&s), TrayState::Amber);
        s.safe_apply.phase = Phase::RevertFailed;
        assert_eq!(tray_state(&s), TrayState::Red);
        let mut done = snap();
        done.ip_refresh = Some(crate::speedtest::running_ip_refresh());
        done.ip_refresh.as_mut().unwrap().active = false;
        assert!(quit_is_safe(&done));
    }

    /// Between its steps the safe apply is committed, but the router may be on the temporary
    /// bands: still amber, and quitting asks first.
    #[test]
    fn a_band_switch_for_a_new_ip_is_busy_between_its_steps() {
        let mut s = with_phase(Phase::Committed);
        assert_eq!(tray_state(&s), TrayState::Green);
        s.ip_reattach = Some(router_core::IpReattachView {
            active: true,
            from_range: Some("10.45".into()),
            last: None,
        });
        assert_eq!(tray_state(&s), TrayState::Amber);
        assert!(!quit_is_safe(&s));
        assert_eq!(quit_warning(&s), "tray.quitConfirm");
        s.ip_reattach.as_mut().unwrap().active = false;
        assert_eq!(tray_state(&s), TrayState::Green);
        assert!(quit_is_safe(&s));
    }

    #[test]
    fn the_quit_warning_names_what_stays_open() {
        let mut s = snap();
        s.safe_apply.phase = Phase::Probation;
        assert_eq!(quit_warning(&s), "tray.quitConfirm");
        s.apn_apply = Some(crate::speedtest::apn_change(Phase::Probation));
        assert_eq!(quit_warning(&s), "tray.quitConfirmApn");
        s.ip_refresh = Some(crate::speedtest::running_ip_refresh());
        assert_eq!(quit_warning(&s), "tray.quitConfirmData");
    }

    #[test]
    fn tray_state_mapping() {
        let mut s = snap();
        assert_eq!(tray_state(&s), TrayState::Green);
        s.session = SessionView::NotConfigured;
        assert_eq!(tray_state(&s), TrayState::Grey);
        // Logged out by the user: grey, not the red of a lost connection.
        s.session = SessionView::LoggedOut;
        s.in_service = false;
        assert_eq!(tray_state(&s), TrayState::Grey);
        assert_eq!(
            tray_state(&AppSnapshot {
                session: SessionView::LoggedOut,
                in_service: false,
                ..with_phase(Phase::Probation)
            }),
            TrayState::Red
        );

        let mut s = snap();
        s.in_service = false;
        assert_eq!(tray_state(&s), TrayState::Red);
        assert_eq!(tray_state(&with_phase(Phase::RevertFailed)), TrayState::Red);
        assert_eq!(
            tray_state(&with_phase(Phase::RevertPending {
                reason: RevertReason::NoReattach
            })),
            TrayState::Red
        );
        // In service but Probation -> Amber.
        assert_eq!(tray_state(&with_phase(Phase::Probation)), TrayState::Amber);
        assert_eq!(
            tray_state(&with_phase(Phase::ReattachGrace)),
            TrayState::Amber
        );
        assert_eq!(
            tray_state(&with_phase(Phase::Reverting {
                attempt: 1,
                fallback: false
            })),
            TrayState::Amber
        );
        let mut s = snap();
        s.session = SessionView::Reconnecting {
            reason: "x".into(),
            retry_in_s: 2,
        };
        assert_eq!(tray_state(&s), TrayState::Amber);
        s.session = SessionView::TakenOver;
        assert_eq!(tray_state(&s), TrayState::Amber);
        // Not in service beats amber phases.
        let mut s = with_phase(Phase::Probation);
        s.in_service = false;
        assert_eq!(tray_state(&s), TrayState::Red);
        assert_eq!(tray_state(&with_phase(Phase::Committed)), TrayState::Green);
    }

    /// C2: safety colours win over grey. Grey only when nothing is being watched.
    #[test]
    fn safety_states_beat_not_configured() {
        let pending = Phase::RevertPending {
            reason: RevertReason::ResumedWithoutService,
        };
        let not_configured = |p: Phase, in_service: bool| {
            let mut s = with_phase(p);
            s.session = SessionView::NotConfigured;
            s.in_service = in_service;
            s
        };
        assert_eq!(tray_state(&not_configured(pending, false)), TrayState::Red);
        assert_eq!(
            tray_state(&not_configured(Phase::RevertFailed, false)),
            TrayState::Red
        );
        for p in [
            Phase::Verifying,
            Phase::ReattachGrace,
            Phase::Probation,
            Phase::Reverting {
                attempt: 0,
                fallback: false,
            },
        ] {
            assert_eq!(
                tray_state(&not_configured(p.clone(), false)),
                TrayState::Red,
                "{p:?}"
            );
        }
        // Grey only for a closed change.
        for p in [
            Phase::Idle,
            Phase::Committed,
            Phase::Reverted {
                reason: RevertReason::NoReattach,
                fallback: false,
            },
        ] {
            assert_eq!(
                tray_state(&not_configured(p.clone(), false)),
                TrayState::Grey,
                "{p:?}"
            );
        }
    }

    fn lte(band: Option<u16>, sinr: Option<f32>) -> LteSignal {
        LteSignal {
            band,
            sinr,
            ..Default::default()
        }
    }

    fn nr(band: Option<u16>, sinr: Option<f32>) -> NrSignal {
        NrSignal {
            band: band.map(|primary| NrBandInfo {
                primary,
                alternates: vec![],
                duplex: Duplex::Tdd,
                dl_mhz: 3500.0,
            }),
            sinr,
            ..Default::default()
        }
    }

    #[test]
    fn icons_are_filled_circles() {
        for (st, c) in [
            (TrayState::Grey, [0x8A, 0x8F, 0x98]),
            (TrayState::Green, [0x2E, 0x9D, 0x5B]),
            (TrayState::Amber, [0xE0, 0xA1, 0x00]),
            (TrayState::Red, [0xD9, 0x3C, 0x3C]),
        ] {
            let px = icon_rgba(st);
            assert_eq!(px.len(), 32 * 32 * 4);
            let at = |x: usize, y: usize| &px[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4];
            assert_eq!(at(16, 16), &[c[0], c[1], c[2], 255]);
            assert_eq!(at(0, 0)[3], 0);
        }
    }

    #[test]
    fn status_line_formats() {
        let mut s = snap();
        s.signal = Some(Signal {
            serving: None,
            lte: Some(lte(Some(3), Some(3.2))),
            nr: Some(nr(Some(78), Some(9.0))),
        });
        assert_eq!(
            tray_status_line(&s, Language::En),
            "B3 + n78 \u{b7} SINR 3 dB"
        );
        s.signal = None;
        assert_eq!(tray_status_line(&s, Language::En), "\u{2014}");
        s.signal = Some(Signal {
            serving: None,
            lte: None,
            nr: Some(nr(Some(78), Some(-1.6))),
        });
        assert_eq!(tray_status_line(&s, Language::Ar), "n78 \u{b7} SINR -2 dB");
        s.signal = Some(Signal {
            serving: None,
            lte: Some(lte(Some(1), None)),
            nr: None,
        });
        assert_eq!(tray_status_line(&s, Language::En), "B1");
    }

    #[test]
    fn the_line_says_when_an_automatic_test_runs() {
        let s = snap();
        assert_eq!(tray_line(&s, Language::En, false), "\u{2014}");
        assert_eq!(
            tray_line(&s, Language::En, true),
            "\u{2014} \u{b7} Automatic speed test running"
        );
        assert_eq!(
            tray_line(&s, Language::Ar, true),
            "\u{2014} \u{b7} يجري اختبار سرعة تلقائي"
        );
    }

    #[test]
    fn status_line_shows_the_bars_the_router_reports() {
        let mut s = snap();
        s.signal = Some(Signal {
            serving: None,
            lte: Some(lte(Some(3), Some(3.2))),
            nr: None,
        });
        s.status = Some(router_core::Status {
            connection: router_core::ConnState::Connected,
            network_type: 101,
            operator: None,
            bars: Some(3),
            nr_bars: Some(4),
            endc: None,
            endc_restricted: None,
        });
        assert_eq!(
            tray_status_line(&s, Language::En),
            "B3 \u{b7} SINR 3 dB \u{b7} 4G 3/5 \u{b7} 5G 4/5"
        );
        // Without a signal the bars say nothing more than the dash.
        s.signal = None;
        assert_eq!(tray_status_line(&s, Language::En), "\u{2014}");
    }
}
