//! Desktop notifications and the Rust-side en/ar string table (keys match the UI
//! JSON `notify.*` / `tray.*` keys).

use std::path::Path;

use router_core::{
    Alert, AppSnapshot, AutomationEvent, EventLog, EventRecord, IpMethod, IpReattachOutcome,
    IpRefreshAlert, IpRefreshOutcome, Phase, SafeApplyEvent, SequenceOrigin, SessionView,
    WidenFallback,
};
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::commands::AppState;
use crate::settings::Language;
use crate::tray::TrayCtl;

/// Every key in the table; the UI JSON files must contain all of them.
pub const NOTIFY_KEYS: &[&str] = &[
    "notify.reverted",
    "notify.revertedToAuto",
    "notify.fellBackToAuto",
    "notify.revertFailed",
    "notify.closeWebUi",
    "notify.cellChanged",
    "notify.sessionTakenOver",
    "notify.confirmRouter",
    "notify.trayNotice",
    "notify.lossIgnored",
    "notify.widenFailedOriginal",
    "notify.widenFailedPrimary",
    "notify.relockStarted",
    "notify.relockGaveUp",
    "notify.sequenceResumed",
    "notify.sequenceDiscarded",
    "notify.dataOnFailed",
    "notify.ipRefreshResumed",
    "notify.ipRefreshSearchNoInternet",
    "notify.apnReverted",
    "notify.apnRevertFailed",
    "notify.apnUnexpectedSwitch",
    "notify.apnUnexpectedSwitchKept",
    "notify.ipReattachChanged",
    "notify.ipReattachUnchanged",
    "notify.ipReattachDone",
    "notify.widenFailedReattach",
    "notify.reattachResumed",
    "tray.open",
    "tray.revertNow",
    "tray.quit",
    "tray.cancel",
    "tray.quitConfirm",
    "tray.quitConfirmData",
    "tray.quitConfirmApn",
    "tray.autoTestRunning",
];

/// `(key, en, ar)`; Arabic is Modern Standard Arabic.
const TABLE: &[(&str, &str, &str)] = &[
    (
        "notify.reverted",
        "Your previous bands were restored.",
        "تمت استعادة النطاقات السابقة.",
    ),
    (
        "notify.revertedToAuto",
        "Switched to Auto (all bands): your previous bands could not be restored.",
        "تم التبديل إلى الوضع التلقائي (جميع النطاقات): تعذّرت استعادة النطاقات السابقة.",
    ),
    (
        "notify.fellBackToAuto",
        "Your previous bands could not be restored, so the router was set to automatic band selection.",
        "تعذّرت استعادة النطاقات السابقة، لذا تم ضبط الراوتر على الاختيار التلقائي للنطاقات.",
    ),
    (
        "notify.revertFailed",
        "Could not restore your previous bands. Open the app now.",
        "تعذّرت استعادة النطاقات السابقة. افتح التطبيق الآن.",
    ),
    (
        "notify.closeWebUi",
        "Close the router's web page so the app can restore your previous bands.",
        "أغلق صفحة الويب الخاصة بالراوتر ليتمكن التطبيق من استعادة النطاقات السابقة.",
    ),
    ("notify.cellChanged", "Cell changed", "تغيّرت الخلية"),
    (
        "notify.sessionTakenOver",
        "Another session took over the router login. Close the router's web page so the app can reconnect.",
        "استولت جلسة أخرى على تسجيل الدخول إلى الراوتر. أغلق صفحة الويب الخاصة بالراوتر ليعيد التطبيق الاتصال.",
    ),
    (
        "notify.confirmRouter",
        "The app didn't log in, because it doesn't recognise this router. Open the app to say whether it's yours.",
        "لم يسجّل التطبيق الدخول لأنه لا يتعرّف على هذا الراوتر. افتح التطبيق لتحدد هل هو راوترك.",
    ),
    (
        "notify.trayNotice",
        "Still running in the tray so it can protect your band change. Use the tray menu to quit.",
        "ما زال التطبيق يعمل في شريط النظام لحماية تغيير النطاقات. استخدم قائمة الشريط للإنهاء.",
    ),
    (
        "notify.lossIgnored",
        "The connection dropped after a band change. Auto-undo is off, so the new bands were kept.",
        "انقطع الاتصال بعد تغيير النطاقات. التراجع التلقائي متوقف، لذا أُبقيت النطاقات الجديدة.",
    ),
    (
        "notify.widenFailedOriginal",
        "Adding the other bands after {band} did not work. Your bands from before were restored.",
        "لم تنجح إضافة النطاقات الأخرى بعد {band}. تمت استعادة النطاقات التي كانت قبل التغيير.",
    ),
    (
        "notify.widenFailedPrimary",
        "Adding the other bands after {band} did not work. The router stays on {band} alone, which has a connection.",
        "لم تنجح إضافة النطاقات الأخرى بعد {band}. سيبقى الراوتر على {band} وحده، وهو متصل.",
    ),
    (
        "notify.relockStarted",
        "The router moved off {band}. Locking it back through safe apply.",
        "انتقل الراوتر عن {band}. تجري إعادته إليه عبر التطبيق الآمن.",
    ),
    (
        "notify.relockGaveUp",
        "Could not keep the router on {band} after 3 tries in a row. Automatic re-lock is paused.",
        "تعذّر إبقاء الراوتر على {band} بعد 3 محاولات متتالية. أُوقفت إعادة التثبيت التلقائية مؤقتًا.",
    ),
    (
        "notify.sequenceResumed",
        "The app was closed during a two-step change to {band}. It is continuing through safe apply.",
        "أُغلق التطبيق أثناء تغيير على خطوتين إلى {band}. يتابع التغيير الآن عبر التطبيق الآمن.",
    ),
    (
        "notify.sequenceDiscarded",
        "A saved two-step band change could not be continued and was set aside. No bands were changed for it.",
        "تعذّرت متابعة تغيير محفوظ للنطاقات على خطوتين، فوُضع جانبًا. لم يُغيَّر أي نطاق بسببه.",
    ),
    (
        "notify.dataOnFailed",
        "Mobile data didn't come back on after an IP refresh. The app keeps trying; open it to see.",
        "لم تعد بيانات الجوال بعد تحديث عنوان الإنترنت. يواصل التطبيق المحاولة، افتحه للاطلاع.",
    ),
    (
        "notify.ipRefreshResumed",
        "The app was closed while mobile data was off for an IP refresh. Data is back on.",
        "أُغلق التطبيق وبيانات الجوال متوقفة لتحديث عنوان الإنترنت. عادت البيانات الآن.",
    ),
    (
        "notify.ipRefreshSearchNoInternet",
        "The router hasn't reconnected after the network search. Open the app; a restart may help.",
        "لم يعد الراوتر إلى الاتصال بعد البحث عن الشبكات. افتح التطبيق، فقد تفيد إعادة تشغيل الراوتر.",
    ),
    (
        "notify.apnReverted",
        "The new APN didn't connect, so the router went back to the APN it had before.",
        "لم يتصل APN الجديد، فأعاد الراوتر APN الذي كان يستخدمه من قبل.",
    ),
    (
        "notify.apnRevertFailed",
        "Could not switch the router back to its previous APN. Open the app now.",
        "تعذّرت إعادة الراوتر إلى APN السابق. افتح التطبيق الآن.",
    ),
    (
        "notify.apnUnexpectedSwitch",
        "After an APN profile was saved, the router switched to another APN by itself. The app is switching it back.",
        "بعد حفظ ملف APN، انتقل الراوتر من تلقاء نفسه إلى APN آخر. يعيده التطبيق الآن إلى ما كان عليه.",
    ),
    (
        "notify.apnUnexpectedSwitchKept",
        "After an APN profile was saved, the router switched to another APN by itself. Open the app to check it.",
        "بعد حفظ ملف APN، انتقل الراوتر من تلقاء نفسه إلى APN آخر. افتح التطبيق للتحقق منه.",
    ),
    (
        "notify.ipReattachChanged",
        "The bands from before are back, and the router has a new IP address range.",
        "عادت النطاقات السابقة، وتغيّرت بداية عنوان الإنترنت.",
    ),
    (
        "notify.ipReattachUnchanged",
        "The bands from before are back, but the IP address range is the same as before.",
        "عادت النطاقات السابقة، لكن بداية عنوان الإنترنت بقيت كما كانت.",
    ),
    (
        "notify.ipReattachDone",
        "The band switch for a new IP address is done, and the bands from before are back.",
        "انتهى تبديل النطاقات للحصول على عنوان إنترنت جديد، وعادت النطاقات السابقة.",
    ),
    (
        "notify.widenFailedReattach",
        "Your bands couldn't be put back after the switch for a new IP address. The router stays on the bands it switched to, which have a connection. You can set your bands again on the Bands page.",
        "تعذّرت إعادة نطاقاتك بعد التبديل للحصول على عنوان إنترنت جديد. سيبقى الراوتر على النطاقات التي انتقل إليها، وهي متصلة. يمكنك ضبط نطاقاتك من جديد في صفحة النطاقات.",
    ),
    (
        "notify.reattachResumed",
        "The app was closed while switching bands for a new IP address. It is putting your bands back through safe apply.",
        "أُغلق التطبيق أثناء تبديل النطاقات للحصول على عنوان إنترنت جديد. يعيد نطاقاتك الآن عبر التطبيق الآمن.",
    ),
    ("tray.open", "Open", "فتح"),
    ("tray.revertNow", "Revert now", "تراجع الآن"),
    ("tray.quit", "Quit", "إنهاء"),
    ("tray.cancel", "Cancel", "إلغاء"),
    (
        "tray.autoTestRunning",
        "Automatic speed test running",
        "يجري اختبار سرعة تلقائي",
    ),
    (
        "tray.quitConfirm",
        "A band change is still being watched. If you quit, it will not be reverted automatically until the app starts again. Quit anyway?",
        "ما زال تغيير النطاقات قيد المراقبة. إذا أنهيت التطبيق فلن تتم الاستعادة التلقائية حتى يُشغَّل مرة أخرى. هل تريد الإنهاء؟",
    ),
    (
        "tray.quitConfirmData",
        "An IP refresh is running. The app tries to turn mobile data back on as it quits, but if that fails, your network stays without internet until the app starts again. Quit anyway?",
        "يجري تحديث عنوان الإنترنت. سيحاول التطبيق إعادة تشغيل بيانات الجوال عند الإنهاء، وإن لم ينجح فستبقى شبكتك بلا إنترنت حتى يُشغَّل التطبيق مرة أخرى. هل تريد الإنهاء؟",
    ),
    (
        "tray.quitConfirmApn",
        "An APN change is still being watched. If you quit, it will not be undone automatically until the app starts again. Quit anyway?",
        "ما زال تغيير APN قيد المراقبة. إذا أنهيت التطبيق فلن يُتراجَع عنه تلقائيًا حتى يُشغَّل مرة أخرى. هل تريد الإنهاء؟",
    ),
];

/// Look up a string; an unknown key returns the key itself.
pub fn tr(lang: Language, key: &'static str) -> &'static str {
    TABLE
        .iter()
        .find(|(k, _, _)| *k == key)
        .map_or(key, |(_, en, ar)| match lang {
            Language::En => en,
            Language::Ar => ar,
        })
}

/// Text for an alert, or `None` when it should stay silent. `TakenOver` dedupe is
/// handled by [`PendingWatch`].
pub fn alert_text(alert: &Alert, cell_alerts: bool, lang: Language) -> Option<String> {
    alert_notice(alert, cell_alerts, lang).map(|(_, t)| t)
}

/// The table key and the text for an alert, or `None` when it should stay silent.
pub fn alert_notice(
    alert: &Alert,
    cell_alerts: bool,
    lang: Language,
) -> Option<(&'static str, String)> {
    let plain = |key: &'static str| Some((key, tr(lang, key).to_string()));
    match alert {
        Alert::SafeApply { event } => match event {
            SafeApplyEvent::Reverted {
                fallback: false, ..
            } => plain("notify.reverted"),
            SafeApplyEvent::Reverted { fallback: true, .. } => plain("notify.revertedToAuto"),
            SafeApplyEvent::FellBackToAuto => plain("notify.fellBackToAuto"),
            SafeApplyEvent::RevertFailed => plain("notify.revertFailed"),
            _ => None,
        },
        Alert::CellChanged { from, to } if cell_alerts => Some((
            "notify.cellChanged",
            format!(
                "{}: {} \u{2192} {}",
                tr(lang, "notify.cellChanged"),
                from.band,
                to.band
            ),
        )),
        Alert::Session {
            view: SessionView::TakenOver,
        } => plain("notify.sessionTakenOver"),
        Alert::Session {
            view: SessionView::ConfirmRouter { .. },
        } => plain("notify.confirmRouter"),
        Alert::Automation { event } => automation_text(event, lang),
        // A switch the user undid is announced in the app; only the automatic undo and a
        // failed one get a notification.
        Alert::Apn {
            event:
                SafeApplyEvent::Reverted {
                    reason: router_core::RevertReason::UserRequested,
                    ..
                },
        } => None,
        Alert::Apn {
            event: SafeApplyEvent::Reverted { .. },
        } => plain("notify.apnReverted"),
        Alert::Apn {
            event: SafeApplyEvent::RevertFailed,
        } => plain("notify.apnRevertFailed"),
        Alert::ApnUnexpectedSwitch { reverting: true } => plain("notify.apnUnexpectedSwitch"),
        Alert::ApnUnexpectedSwitch { reverting: false } => plain("notify.apnUnexpectedSwitchKept"),
        // Other outcomes show in the app, which started the run.
        Alert::IpRefresh {
            event: IpRefreshAlert::DataOnFailed { .. },
        } => plain("notify.dataOnFailed"),
        Alert::IpRefresh {
            event: IpRefreshAlert::Ended { result },
        } if result.outcome == IpRefreshOutcome::Resumed => plain("notify.ipRefreshResumed"),
        // After a search the router reconnects by itself, and nothing confirmed that it did: a
        // run of minutes often ends with the window in the tray, so this one is announced.
        Alert::IpRefresh {
            event: IpRefreshAlert::Ended { result },
        } if result.method == IpMethod::NetworkSearch
            && result.outcome == IpRefreshOutcome::OnlineTimeout =>
        {
            plain("notify.ipRefreshSearchNoInternet")
        }
        // A band switch for a new IP takes minutes, so its end is announced. A failed step is
        // already announced by the safe-apply revert or `WidenFailed`, and the user's own
        // revert or a replaced switch stays in the app.
        Alert::IpReattachEnded { result } => match result.outcome {
            IpReattachOutcome::Changed => plain("notify.ipReattachChanged"),
            IpReattachOutcome::Unchanged => plain("notify.ipReattachUnchanged"),
            IpReattachOutcome::Unknown => plain("notify.ipReattachDone"),
            IpReattachOutcome::Step1Failed
            | IpReattachOutcome::Step2Failed
            | IpReattachOutcome::Reverted
            | IpReattachOutcome::Cancelled => None,
        },
        _ => None,
    }
}

/// Step 1 failing and a step 2 that fell back to Auto are already announced by the safe-apply
/// revert notifications, so they stay silent here. Returns the table key with the text.
fn automation_text(event: &AutomationEvent, lang: Language) -> Option<(&'static str, String)> {
    let plain = |key: &'static str| Some((key, tr(lang, key).to_string()));
    let with_band = |key: &'static str, band: u8| {
        Some((key, tr(lang, key).replace("{band}", &format!("B{band}"))))
    };
    match *event {
        AutomationEvent::LossIgnored { .. } => plain("notify.lossIgnored"),
        // Step 1 of a band switch for a new IP isn't one band, so it isn't named.
        AutomationEvent::WidenFailed {
            origin: SequenceOrigin::IpReattach,
            ended_on,
            ..
        } => match ended_on {
            WidenFallback::Auto => None,
            WidenFallback::Original | WidenFallback::PrimaryOnly => {
                plain("notify.widenFailedReattach")
            }
        },
        AutomationEvent::WidenFailed {
            primary, ended_on, ..
        } => match ended_on {
            WidenFallback::Original => with_band("notify.widenFailedOriginal", primary),
            WidenFallback::PrimaryOnly => with_band("notify.widenFailedPrimary", primary),
            WidenFallback::Auto => None,
        },
        AutomationEvent::RelockStarted { primary, .. } => {
            with_band("notify.relockStarted", primary)
        }
        AutomationEvent::RelockGaveUp { primary, .. } => with_band("notify.relockGaveUp", primary),
        AutomationEvent::SequenceResumed {
            origin: SequenceOrigin::IpReattach,
            ..
        } => plain("notify.reattachResumed"),
        AutomationEvent::SequenceResumed { primary, .. } => {
            with_band("notify.sequenceResumed", primary)
        }
        AutomationEvent::SequenceDiscarded => plain("notify.sequenceDiscarded"),
        AutomationEvent::PrimaryLockFailed { .. } => None,
    }
}

/// "Once per occurrence" gates, driven by snapshots and alerts.
#[derive(Debug, Default)]
pub struct PendingWatch {
    /// The close-web-UI notice already fired in this `RevertPending` occurrence.
    close_web_fired: bool,
    taken_over: bool,
}

impl PendingWatch {
    /// True once per `RevertPending` occurrence, when the router's own web page holds the login
    /// (session `TakenOver`). Never for an unreachable router or a missing login: closing a
    /// browser tab wouldn't help there.
    pub fn observe(&mut self, s: &AppSnapshot) -> bool {
        let pending = matches!(s.safe_apply.phase, Phase::RevertPending { .. });
        let fire = pending && !self.close_web_fired && s.session == SessionView::TakenOver;
        if !pending {
            self.close_web_fired = false;
        } else if fire {
            self.close_web_fired = true;
        }
        if s.session != SessionView::TakenOver {
            self.taken_over = false;
        }
        fire
    }

    /// True the first time a `TakenOver` alert arrives in an occurrence.
    pub fn taken_over_first(&mut self) -> bool {
        !std::mem::replace(&mut self.taken_over, true)
    }
}

/// Shows a desktop notification and logs the attempt to `events.jsonl` as a `notify` line with
/// the table `key` (never the localized body).
///
/// `"result": "queued"` only means the notification plugin took it: plugin 2.4.0 hands it to a
/// background task and discards what Windows answers, so it is not proof that a toast appeared.
/// `"appId": false` means the exe runs from `target\debug` or `target\release` (a dev or
/// `--no-bundle` build). The plugin then sets no Windows app identity, and Windows may show the
/// toast under another app's name or drop it. An installed build logs `"appId": true`.
pub fn show(app: &AppHandle, key: &'static str, body: &str) {
    let res = app
        .notification()
        .builder()
        .title("5G Router Band Tuner")
        .body(body)
        .show();
    log_attempt(app, key, res.map_err(|e| e.to_string()));
}

/// Milliseconds since the Unix epoch, 0 if the clock is before it.
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Mirrors the plugin's rule: it sets the Windows app identity only when the exe's folder
/// doesn't end in `target\debug` or `target\release`.
fn app_id_applies(exe_dir: &Path) -> bool {
    let sep = std::path::MAIN_SEPARATOR;
    let dir = exe_dir.display().to_string();
    !(dir.ends_with(&format!("{sep}target{sep}debug"))
        || dir.ends_with(&format!("{sep}target{sep}release")))
}

fn exe_has_app_id() -> bool {
    tauri::utils::platform::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(app_id_applies))
        .unwrap_or(false)
}

/// Longest error text kept in a `notify` line.
const MAX_ERROR_CHARS: usize = 200;

/// The `notify` line for one attempt.
pub(crate) fn attempt_record(
    at_ms: u64,
    key: &str,
    result: &Result<(), String>,
    app_id: bool,
) -> EventRecord {
    let mut detail = serde_json::json!({
        "key": key,
        "result": if result.is_ok() { "queued" } else { "error" },
        "appId": app_id,
    });
    if let Err(e) = result {
        let text: String = e.chars().take(MAX_ERROR_CHARS).collect();
        detail["error"] = serde_json::Value::String(text);
    }
    EventRecord {
        at_ms,
        kind: router_core::event_text::kinds::NOTIFY.into(),
        detail,
    }
}

/// Appended off the calling thread: tray and window handlers call `show` on the main thread.
/// A failed append is dropped.
fn log_attempt(app: &AppHandle, key: &'static str, result: Result<(), String>) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let dir = state.data_dir().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let rec = attempt_record(now_ms(), key, &result, exe_has_app_id());
        let _ = EventLog::new(dir).append(&rec);
    });
}

/// Called for every forwarded alert.
pub fn on_alert(app: &AppHandle, alert: &Alert) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    // A scan changes bands on purpose, and some of those changes are meant to fail.
    let scanning = app
        .try_state::<crate::autoscan::ScanState>()
        .is_some_and(|s| s.is_running());
    if scanning && matches!(alert, Alert::SafeApply { .. } | Alert::CellChanged { .. }) {
        return;
    }
    let (lang, cell_alerts) = state.notify_prefs();
    let Some((key, text)) = alert_notice(alert, cell_alerts, lang) else {
        return;
    };
    if matches!(
        alert,
        Alert::Session {
            view: SessionView::TakenOver
        }
    ) {
        let Some(ctl) = app.try_state::<TrayCtl>() else {
            return;
        };
        if !ctl.watch().taken_over_first() {
            return;
        }
    }
    show(app, key, &text);
}

#[cfg(test)]
mod tests {
    use super::*;
    use router_core::{CellKey, RevertReason, SafeApplyView};

    #[test]
    fn every_key_has_en_and_ar() {
        for k in NOTIFY_KEYS {
            let (_, en, ar) = TABLE
                .iter()
                .find(|(t, _, _)| t == k)
                .unwrap_or_else(|| panic!("{k} missing from TABLE"));
            assert!(!en.is_empty() && !ar.is_empty(), "{k}");
            assert_ne!(en, ar, "{k}");
            assert!(
                ar.chars().any(|c| ('\u{600}'..='\u{6FF}').contains(&c)),
                "{k}"
            );
        }
        assert_eq!(TABLE.len(), NOTIFY_KEYS.len());
        assert_eq!(tr(Language::En, "tray.quit"), "Quit");
    }

    fn key(band: &str) -> CellKey {
        CellKey {
            rat: router_core::Rat::Lte,
            pci: None,
            cell_id: None,
            band: band.into(),
        }
    }

    #[test]
    fn alert_rules() {
        let rev = Alert::SafeApply {
            event: SafeApplyEvent::Reverted {
                reason: RevertReason::NoReattach,
                fallback: false,
            },
        };
        assert_eq!(
            alert_text(&rev, false, Language::En).as_deref(),
            Some(tr(Language::En, "notify.reverted"))
        );
        // A revert that ended on Auto doesn't claim the previous bands came back.
        let auto = Alert::SafeApply {
            event: SafeApplyEvent::Reverted {
                reason: RevertReason::NoReattach,
                fallback: true,
            },
        };
        let text = alert_text(&auto, false, Language::En).unwrap();
        assert_eq!(text, tr(Language::En, "notify.revertedToAuto"));
        assert!(text.contains("Switched to Auto (all bands)"), "{text}");
        assert_eq!(
            alert_text(&auto, false, Language::Ar).as_deref(),
            Some(tr(Language::Ar, "notify.revertedToAuto"))
        );
        let applied = Alert::SafeApply {
            event: SafeApplyEvent::Applied,
        };
        assert!(alert_text(&applied, true, Language::En).is_none());
        let cell = Alert::CellChanged {
            from: key("B3"),
            to: key("B1"),
        };
        assert!(alert_text(&cell, false, Language::En).is_none());
        assert_eq!(
            alert_text(&cell, true, Language::En).unwrap(),
            "Cell changed: B3 \u{2192} B1"
        );
    }

    fn snap(phase: Phase, session: SessionView) -> AppSnapshot {
        AppSnapshot {
            session,
            device: None,
            status: None,
            signal: None,
            config: None,
            capabilities: None,
            safe_apply: SafeApplyView {
                phase,
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

    #[test]
    fn automation_alerts() {
        use router_core::SequenceOrigin;
        let text =
            |event: AutomationEvent, lang| alert_text(&Alert::Automation { event }, false, lang);
        let widen = |ended_on| AutomationEvent::WidenFailed {
            primary: 3,
            origin: SequenceOrigin::User,
            ended_on,
        };
        assert_eq!(
            text(widen(WidenFallback::PrimaryOnly), Language::En).unwrap(),
            "Adding the other bands after B3 did not work. The router stays on B3 alone, which has a connection."
        );
        let ar = text(widen(WidenFallback::Original), Language::Ar).unwrap();
        assert!(ar.contains("B3") && !ar.contains("{band}"), "{ar}");
        assert!(text(widen(WidenFallback::Auto), Language::En).is_none());
        let started = AutomationEvent::RelockStarted {
            primary: 3,
            serving_band: 20,
        };
        assert_eq!(
            text(started, Language::En).unwrap(),
            "The router moved off B3. Locking it back through safe apply."
        );
        let gave_up = AutomationEvent::RelockGaveUp {
            primary: 3,
            attempts: 3,
        };
        assert!(text(gave_up, Language::En).unwrap().contains("B3"));
        let lost = AutomationEvent::LossIgnored {
            reason: RevertReason::LossOfService,
        };
        assert_eq!(
            text(lost, Language::En).as_deref(),
            Some(tr(Language::En, "notify.lossIgnored"))
        );
        let step1 = AutomationEvent::PrimaryLockFailed {
            primary: 3,
            origin: SequenceOrigin::Relock,
        };
        assert!(text(step1, Language::En).is_none());
        let resumed = AutomationEvent::SequenceResumed {
            primary: 3,
            origin: SequenceOrigin::User,
            step: 2,
        };
        assert_eq!(
            text(resumed.clone(), Language::En).unwrap(),
            "The app was closed during a two-step change to B3. It is continuing through safe apply."
        );
        let ar = text(resumed, Language::Ar).unwrap();
        assert!(ar.contains("B3") && !ar.contains("{band}"), "{ar}");
        assert_eq!(
            text(AutomationEvent::SequenceDiscarded, Language::En).as_deref(),
            Some(tr(Language::En, "notify.sequenceDiscarded"))
        );
    }

    #[test]
    fn revert_pending_while_taken_over_fires_once() {
        let pending = Phase::RevertPending {
            reason: RevertReason::NoReattach,
        };
        let mut w = PendingWatch::default();
        assert!(!w.observe(&snap(Phase::Probation, SessionView::LoggedIn)));
        assert!(w.observe(&snap(pending.clone(), SessionView::TakenOver)));
        assert!(!w.observe(&snap(pending.clone(), SessionView::TakenOver)));
        assert!(!w.observe(&snap(Phase::Idle, SessionView::LoggedIn)));
        // A new occurrence fires again; with a session it does not.
        assert!(!w.observe(&snap(pending.clone(), SessionView::LoggedIn)));
        w.observe(&snap(Phase::Idle, SessionView::LoggedIn));
        // The takeover may only show after the change went pending: it still fires, once.
        assert!(!w.observe(&snap(
            pending.clone(),
            SessionView::Reconnecting {
                reason: "session lost".into(),
                retry_in_s: 5
            }
        )));
        assert!(w.observe(&snap(pending.clone(), SessionView::TakenOver)));
        assert!(!w.observe(&snap(pending, SessionView::TakenOver)));
    }

    /// C2: "close the router's web page" is only right when the web page holds the login.
    #[test]
    fn close_web_ui_never_for_unreachable_or_not_configured() {
        let pending = Phase::RevertPending {
            reason: RevertReason::ResumedWithoutService,
        };
        for session in [
            SessionView::NotConfigured,
            SessionView::Connecting,
            SessionView::Reconnecting {
                reason: "router unreachable".into(),
                retry_in_s: 4,
            },
            SessionView::Reconnecting {
                reason: "request timed out".into(),
                retry_in_s: 8,
            },
            SessionView::BadCredentials,
        ] {
            let mut w = PendingWatch::default();
            assert!(!w.observe(&snap(Phase::Idle, session.clone())));
            assert!(
                !w.observe(&snap(pending.clone(), session.clone())),
                "{session:?}"
            );
            assert!(
                !w.observe(&snap(pending.clone(), session.clone())),
                "{session:?}"
            );
        }
    }

    #[test]
    fn attempt_record_shape() {
        let ok = attempt_record(7, "notify.reverted", &Ok(()), true);
        assert_eq!(ok.kind, "notify");
        assert_eq!(ok.at_ms, 7);
        assert_eq!(ok.detail["key"], "notify.reverted");
        assert_eq!(ok.detail["result"], "queued");
        assert_eq!(ok.detail["appId"], true);
        assert!(ok.detail.get("error").is_none());

        // Only the key is logged, never the body text.
        let cell = Alert::CellChanged {
            from: key("B3"),
            to: key("B1"),
        };
        let (k, body) = alert_notice(&cell, true, Language::En).unwrap();
        assert!(body.contains("B3"), "{body}");
        let line = serde_json::to_string(&attempt_record(0, k, &Ok(()), false).detail).unwrap();
        assert!(!line.contains("B3"), "{line}");
        let line = serde_json::to_string(&ok.detail).unwrap();
        assert!(!line.contains("Your previous bands"), "{line}");

        let err = attempt_record(0, "notify.trayNotice", &Err("x".repeat(500)), false);
        assert_eq!(err.kind, "notify");
        assert_eq!(err.detail["result"], "error");
        assert_eq!(err.detail["appId"], false);
        assert_eq!(err.detail["error"].as_str().unwrap().chars().count(), 200);
    }

    #[test]
    fn app_id_rules() {
        #[cfg(windows)]
        {
            assert!(!app_id_applies(Path::new(r"C:\x\target\debug")));
            assert!(!app_id_applies(Path::new(r"C:\x\target\release")));
            assert!(app_id_applies(Path::new(
                r"C:\Program Files\5G Router Band Tuner"
            )));
        }
        #[cfg(not(windows))]
        {
            assert!(!app_id_applies(Path::new("/x/target/debug")));
            assert!(!app_id_applies(Path::new("/x/target/release")));
            assert!(app_id_applies(Path::new("/opt/5G Router Band Tuner")));
        }
    }

    #[test]
    fn every_alert_notice_key_is_in_the_table() {
        let reattach = |outcome| Alert::IpReattachEnded {
            result: router_core::IpReattachResult {
                outcome,
                from: Some("10.45".into()),
                to: Some("10.62".into()),
                restore: None,
            },
        };
        let refresh_end = |method, outcome| Alert::IpRefresh {
            event: IpRefreshAlert::Ended {
                result: router_core::IpRefreshResult {
                    outcome,
                    tries: 1,
                    target: None,
                    from: Some("10.45".into()),
                    to: None,
                    method,
                    refusal: None,
                    bands_changed: false,
                    bands_before: None,
                },
            },
        };
        let reverted = |fallback| Alert::SafeApply {
            event: SafeApplyEvent::Reverted {
                reason: RevertReason::NoReattach,
                fallback,
            },
        };
        let auto = |event| Alert::Automation { event };
        let widen = |ended_on| {
            auto(AutomationEvent::WidenFailed {
                primary: 3,
                origin: SequenceOrigin::User,
                ended_on,
            })
        };
        let alerts = [
            reverted(false),
            reverted(true),
            Alert::SafeApply {
                event: SafeApplyEvent::FellBackToAuto,
            },
            Alert::SafeApply {
                event: SafeApplyEvent::RevertFailed,
            },
            Alert::CellChanged {
                from: key("B3"),
                to: key("B1"),
            },
            Alert::Session {
                view: SessionView::TakenOver,
            },
            Alert::Session {
                view: SessionView::ConfirmRouter { first_time: true },
            },
            widen(WidenFallback::Original),
            widen(WidenFallback::PrimaryOnly),
            auto(AutomationEvent::RelockStarted {
                primary: 3,
                serving_band: 20,
            }),
            auto(AutomationEvent::RelockGaveUp {
                primary: 3,
                attempts: 3,
            }),
            auto(AutomationEvent::LossIgnored {
                reason: RevertReason::LossOfService,
            }),
            auto(AutomationEvent::SequenceResumed {
                primary: 3,
                origin: SequenceOrigin::User,
                step: 2,
            }),
            auto(AutomationEvent::SequenceDiscarded),
            Alert::Apn {
                event: SafeApplyEvent::Reverted {
                    reason: RevertReason::NoReattach,
                    fallback: false,
                },
            },
            Alert::Apn {
                event: SafeApplyEvent::RevertFailed,
            },
            Alert::ApnUnexpectedSwitch { reverting: true },
            Alert::ApnUnexpectedSwitch { reverting: false },
            auto(AutomationEvent::WidenFailed {
                primary: 1,
                origin: SequenceOrigin::IpReattach,
                ended_on: WidenFallback::PrimaryOnly,
            }),
            auto(AutomationEvent::SequenceResumed {
                primary: 1,
                origin: SequenceOrigin::IpReattach,
                step: 1,
            }),
            reattach(IpReattachOutcome::Changed),
            reattach(IpReattachOutcome::Unchanged),
            reattach(IpReattachOutcome::Unknown),
            refresh_end(IpMethod::NetworkSearch, IpRefreshOutcome::OnlineTimeout),
        ];
        // Data was confirmed back on after a data switch, so its online wait stays in the app.
        assert_eq!(
            alert_text(
                &refresh_end(IpMethod::DataSwitch, IpRefreshOutcome::OnlineTimeout),
                true,
                Language::En
            ),
            None
        );
        assert_eq!(
            alert_text(
                &refresh_end(IpMethod::NetworkSearch, IpRefreshOutcome::Unchanged),
                true,
                Language::En
            ),
            None
        );
        // A failed step is announced by its revert or `WidenFailed`; the user's own revert and a
        // replaced switch stay in the app.
        for quiet in [
            IpReattachOutcome::Step1Failed,
            IpReattachOutcome::Step2Failed,
            IpReattachOutcome::Reverted,
            IpReattachOutcome::Cancelled,
        ] {
            assert_eq!(alert_text(&reattach(quiet), true, Language::En), None);
        }
        // An APN switch the user undid, or one that went well, stays in the app.
        for quiet in [
            SafeApplyEvent::Reverted {
                reason: RevertReason::UserRequested,
                fallback: false,
            },
            SafeApplyEvent::Committed,
            SafeApplyEvent::Started,
        ] {
            assert_eq!(
                alert_text(&Alert::Apn { event: quiet }, true, Language::En),
                None
            );
        }
        for lang in [Language::En, Language::Ar] {
            for alert in &alerts {
                let (k, text) = alert_notice(alert, true, lang).unwrap();
                assert!(NOTIFY_KEYS.contains(&k), "{k}");
                assert_eq!(alert_text(alert, true, lang), Some(text.clone()));
                let raw = tr(lang, k);
                if !raw.contains("{band}") && k != "notify.cellChanged" {
                    assert_eq!(text, raw, "{k}");
                }
            }
        }
    }

    /// Logging out is the user's own action: no OS notification.
    #[test]
    fn logging_out_is_silent() {
        let alert = Alert::Session {
            view: SessionView::LoggedOut,
        };
        for lang in [Language::En, Language::Ar] {
            assert_eq!(alert_text(&alert, true, lang), None);
        }
    }

    #[test]
    fn taken_over_once_per_occurrence() {
        let mut w = PendingWatch::default();
        assert!(w.taken_over_first());
        assert!(!w.taken_over_first());
        w.observe(&snap(Phase::Idle, SessionView::TakenOver));
        assert!(!w.taken_over_first());
        w.observe(&snap(Phase::Idle, SessionView::LoggedIn));
        assert!(w.taken_over_first());
    }
}
