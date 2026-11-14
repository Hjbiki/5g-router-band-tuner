use std::io;
use std::path::Path;

use router_core::NetConfig;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

const FILE: &str = "settings.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DriverId {
    HuaweiH122,
    Mock,
}

/// The languages Rust has its own strings for (tray, notifications). The UI may offer more;
/// anything else falls back to English here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    En,
    Ar,
}

impl Language {
    /// From a stored language code (`"system"`, `"en"`, `"ar-SA"`, ...).
    pub fn resolve(code: &str) -> Self {
        let code = if code == SYSTEM_LANGUAGE {
            sys_locale::get_locale().unwrap_or_default()
        } else {
            code.to_string()
        };
        if code.to_ascii_lowercase().starts_with("ar") {
            Language::Ar
        } else {
            Language::En
        }
    }
}

/// Stored `language` value meaning "follow Windows".
pub const SYSTEM_LANGUAGE: &str = "system";

fn valid_language(code: &str) -> bool {
    if code == SYSTEM_LANGUAGE {
        return true;
    }
    let mut parts = code.split('-');
    let primary = parts.next().unwrap_or("");
    (2..=3).contains(&primary.len())
        && primary.bytes().all(|b| b.is_ascii_lowercase())
        && parts.all(|p| (2..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ThemeMode {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Palette {
    Carbon,
    Cobalt,
    Sandstone,
}

/// Which layout of the Signal and Bands pages to show. Each is a full design, so people can
/// compare them in the app; the data and every action are the same in all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Design {
    /// The cards and band chips the app has always had.
    #[default]
    Classic,
    /// The answer first, a picture of the link, and the bands on a map of the spectrum.
    Spectrum,
}

/// What the app may do by itself. Every change it makes still goes through safe apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct AutomationSettings {
    /// The starred LTE band (1-128), if any.
    pub primary_band: Option<u8>,
    /// Lock back to the primary band when the router drifts off it.
    pub relock_primary: bool,
    /// Revert a change whose probation loses service. Off means the change stays even
    /// without internet; the journal, verify and "Revert now" still work.
    pub revert_on_loss: bool,
}

impl Default for AutomationSettings {
    fn default() -> Self {
        Self {
            primary_band: None,
            relock_primary: false,
            revert_on_loss: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SpeedTestProvider {
    /// Built in, the default: speed.cloudflare.com.
    Cloudflare,
    /// The official Ookla Speedtest CLI, downloaded only after the user accepts its terms.
    Ookla,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SpeedTestSize {
    /// About 40 MB per test.
    Light,
    /// About 200 MB per test.
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct SpeedTestSettings {
    pub provider: SpeedTestProvider,
    pub size: SpeedTestSize,
    /// The user accepted Ookla's licence and privacy terms in the app.
    pub ookla_terms_accepted: bool,
}

impl Default for SpeedTestSettings {
    fn default() -> Self {
        Self {
            provider: SpeedTestProvider::Cloudflare,
            size: SpeedTestSize::Light,
            ookla_terms_accepted: false,
        }
    }
}

/// Speed tests the app runs by itself, for the History page. Off by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct AutoSpeedTestSettings {
    pub enabled: bool,
    /// Hours between tests, `AUTO_TEST_EVERY_H_MIN` to `AUTO_TEST_EVERY_H_MAX`.
    pub every_h: u16,
    /// Ookla only once its terms are accepted; otherwise Cloudflare.
    pub provider: SpeedTestProvider,
    pub size: SpeedTestSize,
}

impl Default for AutoSpeedTestSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            every_h: 3,
            provider: SpeedTestProvider::Cloudflare,
            size: SpeedTestSize::Light,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct HistorySettings {
    /// Days of history to keep, `RETENTION_DAYS_MIN` to `RETENTION_DAYS_MAX`; `None` keeps
    /// everything.
    pub retention_days: Option<u16>,
    pub auto_speed_test: AutoSpeedTestSettings,
}

impl Default for HistorySettings {
    fn default() -> Self {
        Self {
            retention_days: Some(90),
            auto_speed_test: AutoSpeedTestSettings::default(),
        }
    }
}

/// What the Refresh IP dialog starts with: the last address start aimed for, and the tries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct IpRefreshSettings {
    /// One or two numbers ("10" or "10.45"); never a whole address.
    pub last_target: Option<String>,
    /// 1 to 5.
    pub max_tries: u8,
}

impl Default for IpRefreshSettings {
    fn default() -> Self {
        Self {
            last_target: None,
            max_tries: router_core::MAX_TRIES,
        }
    }
}

impl IpRefreshSettings {
    fn validate(&self) -> Result<(), String> {
        if !(1..=router_core::MAX_TRIES).contains(&self.max_tries) {
            return Err(router_core::IP_TRIES_INVALID.into());
        }
        if let Some(t) = &self.last_target {
            router_core::IpTarget::parse(t)
                .map_err(|_| router_core::IP_TARGET_INVALID.to_string())?;
        }
        Ok(())
    }
}

/// The 7-day floor keeps the Auto-scan's 7-day averages.
pub const RETENTION_DAYS_MIN: u16 = 7;
pub const RETENTION_DAYS_MAX: u16 = 3650;
pub const AUTO_TEST_EVERY_H_MIN: u16 = 1;
pub const AUTO_TEST_EVERY_H_MAX: u16 = 168;

fn valid_retention(days: Option<u16>) -> bool {
    days.is_none_or(|d| (RETENTION_DAYS_MIN..=RETENTION_DAYS_MAX).contains(&d))
}

fn valid_every_h(h: u16) -> bool {
    (AUTO_TEST_EVERY_H_MIN..=AUTO_TEST_EVERY_H_MAX).contains(&h)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Digits {
    Western,
    ArabicIndic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct TimerSettings {
    pub grace_s: u32,
    pub loss_s: u32,
    pub probation_s: u32,
}

impl Default for TimerSettings {
    fn default() -> Self {
        Self {
            grace_s: 90,
            loss_s: 60,
            probation_s: 180,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Preset {
    pub name: String,
    pub config: NetConfig,
}

/// User settings. Never contains a password (those live in the OS keychain).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Settings {
    pub host: String,
    pub driver: DriverId,
    pub username: String,
    pub timers: TimerSettings,
    pub presets: Vec<Preset>,
    /// `"system"` (follow Windows) or a language code from the UI's language files.
    pub language: String,
    pub digits: Digits,
    pub theme: ThemeMode,
    pub palette: Palette,
    pub design: Design,
    /// Size of the whole interface in percent, from `UI_SCALE_MIN` to `UI_SCALE_MAX`.
    pub ui_scale: u16,
    pub cell_alerts: bool,
    pub start_at_login: bool,
    pub tray_notice_shown: bool,
    /// The first-run guide was finished or skipped.
    pub onboarding_done: bool,
    pub automation: AutomationSettings,
    pub speed_test: SpeedTestSettings,
    /// Check GitHub for a newer release. `None` until the user has been asked.
    pub update_check: Option<bool>,
    /// Retention and automatic speed tests.
    #[serde(default)]
    pub history: HistorySettings,
    #[serde(default)]
    pub ip_refresh: IpRefreshSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            host: "192.168.8.1".into(),
            driver: DriverId::HuaweiH122,
            username: "admin".into(),
            timers: TimerSettings::default(),
            presets: Vec::new(),
            language: SYSTEM_LANGUAGE.into(),
            digits: Digits::Western,
            theme: ThemeMode::System,
            palette: Palette::Carbon,
            design: Design::Classic,
            ui_scale: 100,
            cell_alerts: true,
            start_at_login: false,
            tray_notice_shown: false,
            onboarding_done: false,
            automation: AutomationSettings::default(),
            speed_test: SpeedTestSettings::default(),
            update_check: None,
            history: HistorySettings::default(),
            ip_refresh: IpRefreshSettings::default(),
        }
    }
}

pub const UI_SCALE_MIN: u16 = 90;
pub const UI_SCALE_MAX: u16 = 150;

fn valid_host(h: &str) -> bool {
    !h.is_empty() && h.trim() == h && !h.contains(['/', '\\', ' ', '@'])
}

impl Settings {
    /// Defaults when the file is missing or corrupt; individually invalid fields
    /// (host, username, timers) fall back to their defaults. Never fails.
    pub fn load(dir: &Path) -> Self {
        let mut s: Self = std::fs::read_to_string(dir.join(FILE))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        s.sanitize();
        s
    }

    fn sanitize(&mut self) {
        let d = Self::default();
        if !valid_host(&self.host) {
            self.host = d.host;
        }
        if self.username.trim().is_empty() {
            self.username = d.username;
        }
        if !valid_language(&self.language) {
            self.language = d.language;
        }
        if self
            .automation
            .primary_band
            .is_some_and(|b| !(1..=128).contains(&b))
        {
            self.automation.primary_band = None;
        }
        if !(UI_SCALE_MIN..=UI_SCALE_MAX).contains(&self.ui_scale) {
            self.ui_scale = d.ui_scale;
        }
        if !valid_retention(self.history.retention_days) {
            self.history.retention_days = d.history.retention_days;
        }
        if !valid_every_h(self.history.auto_speed_test.every_h) {
            self.history.auto_speed_test.every_h = d.history.auto_speed_test.every_h;
        }
        if self.ip_refresh.validate().is_err() {
            self.ip_refresh = d.ip_refresh;
        }
        self.normalize();
        let t = &mut self.timers;
        if !(30..=300).contains(&t.grace_s) {
            t.grace_s = d.timers.grace_s;
        }
        if !(20..=300).contains(&t.loss_s) {
            t.loss_s = d.timers.loss_s;
        }
        if !(60..=600).contains(&t.probation_s) {
            t.probation_s = d.timers.probation_s;
        }
    }

    /// Rules between fields, applied on load and before every save: with no primary band
    /// there is nothing to re-lock to, so the watchdog can't stay armed; and automatic tests
    /// fall back to Cloudflare when Ookla's terms aren't (or are no longer) accepted.
    pub fn normalize(&mut self) {
        if self.automation.primary_band.is_none() {
            self.automation.relock_primary = false;
        }
        let auto = &mut self.history.auto_speed_test;
        if auto.provider == SpeedTestProvider::Ookla && !self.speed_test.ookla_terms_accepted {
            auto.provider = SpeedTestProvider::Cloudflare;
        }
    }

    pub fn save(&self, dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        let tmp = dir.join("settings.json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, dir.join(FILE))
    }

    pub fn validate(&self) -> Result<(), String> {
        if !valid_host(&self.host) {
            return Err("invalid host".into());
        }
        if self.username.trim().is_empty() {
            return Err("username must not be empty".into());
        }
        let t = &self.timers;
        if !(30..=300).contains(&t.grace_s) {
            return Err("re-attach grace must be 30-300 s".into());
        }
        if !(20..=300).contains(&t.loss_s) {
            return Err("loss threshold must be 20-300 s".into());
        }
        if !(60..=600).contains(&t.probation_s) {
            return Err("probation must be 60-600 s".into());
        }
        if !valid_language(&self.language) {
            return Err("invalid language code".into());
        }
        if !(UI_SCALE_MIN..=UI_SCALE_MAX).contains(&self.ui_scale) {
            return Err("UI size must be 90-150%".into());
        }
        if self
            .automation
            .primary_band
            .is_some_and(|b| !(1..=128).contains(&b))
        {
            return Err("primary band must be 1-128".into());
        }
        if !valid_retention(self.history.retention_days) {
            return Err("history retention must be 7-3650 days".into());
        }
        let auto = &self.history.auto_speed_test;
        if !valid_every_h(auto.every_h) {
            return Err("automatic test interval must be 1-168 hours".into());
        }
        if auto.provider == SpeedTestProvider::Ookla && !self.speed_test.ookla_terms_accepted {
            return Err("accept the Ookla terms before choosing it for automatic tests".into());
        }
        self.ip_refresh.validate()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let s = Settings::default();
        assert_eq!(s.timers.grace_s, 90);
        assert_eq!(s.timers.loss_s, 60);
        assert_eq!(s.timers.probation_s, 180);
        assert_eq!(s.host, "192.168.8.1");
        assert_eq!(s.username, "admin");
        assert_eq!(s.language, "system");
        assert_eq!(s.palette, Palette::Carbon);
        assert_eq!(s.theme, ThemeMode::System);
        assert!(s.automation.revert_on_loss);
        assert!(!s.automation.relock_primary);
        assert_eq!(s.speed_test.provider, SpeedTestProvider::Cloudflare);
        assert_eq!(s.speed_test.size, SpeedTestSize::Light);
        assert!(s.validate().is_ok());
    }

    #[test]
    fn ip_refresh_settings_are_checked() {
        let d = Settings::default();
        assert_eq!(d.ip_refresh.max_tries, 5);
        assert_eq!(d.ip_refresh.last_target, None);
        let with = |tries: u8, target: Option<&str>| {
            let s = Settings {
                ip_refresh: IpRefreshSettings {
                    last_target: target.map(str::to_string),
                    max_tries: tries,
                },
                ..Settings::default()
            };
            s.validate()
        };
        assert!(with(1, Some("10")).is_ok());
        assert!(with(5, Some("10.45")).is_ok());
        assert!(with(0, None).is_err());
        assert!(with(6, None).is_err());
        assert!(with(3, Some("10.45.12")).is_err());
        assert!(with(3, Some("256")).is_err());

        // A bad value on disk falls back to the defaults.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"ipRefresh":{"lastTarget":"10.45.12.1","maxTries":9}}"#,
        )
        .unwrap();
        assert_eq!(
            Settings::load(dir.path()).ip_refresh,
            IpRefreshSettings::default()
        );
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"ipRefresh":{"lastTarget":"100.70","maxTries":3}}"#,
        )
        .unwrap();
        let s = Settings::load(dir.path());
        assert_eq!(s.ip_refresh.last_target.as_deref(), Some("100.70"));
        assert_eq!(s.ip_refresh.max_tries, 3);
    }

    #[test]
    fn language_codes() {
        for ok in ["system", "en", "ar", "fil", "pt-BR", "zh-Hant"] {
            assert!(valid_language(ok), "{ok}");
        }
        for bad in ["", "EN", "e", "english", "en_US", "../x", "en-"] {
            assert!(!valid_language(bad), "{bad}");
        }
        assert_eq!(Language::resolve("ar-SA"), Language::Ar);
        assert_eq!(Language::resolve("fr"), Language::En);
    }

    #[test]
    fn older_settings_files_keep_their_values() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("settings.json"),
            r#"{"language":"ar","cellAlerts":false,"onboardingDone":true}"#,
        )
        .unwrap();
        let s = Settings::load(d.path());
        assert_eq!(s.language, "ar");
        assert!(!s.cell_alerts);
        assert_eq!(s.automation, AutomationSettings::default());
        assert_eq!(s.palette, Palette::Carbon);
    }

    #[test]
    fn validate_ranges() {
        let with = |f: fn(&mut Settings)| {
            let mut s = Settings::default();
            f(&mut s);
            s.validate()
        };
        assert!(with(|s| s.timers.grace_s = 29).is_err());
        assert!(with(|s| s.timers.grace_s = 301).is_err());
        assert!(with(|s| s.timers.loss_s = 19).is_err());
        assert!(with(|s| s.timers.loss_s = 301).is_err());
        assert!(with(|s| s.timers.probation_s = 59).is_err());
        assert!(with(|s| s.timers.probation_s = 601).is_err());
        assert!(with(|s| s.timers.grace_s = 30).is_ok());
        assert!(with(|s| s.timers.probation_s = 600).is_ok());
    }

    #[test]
    fn corrupt_file_loads_defaults() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("settings.json"), "{ not json").unwrap();
        assert_eq!(Settings::load(d.path()), Settings::default());
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(Settings::load(empty.path()), Settings::default());
    }

    #[test]
    fn ui_size_stays_in_range() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE), r#"{"uiScale": 400}"#).unwrap();
        assert_eq!(Settings::load(dir.path()).ui_scale, 100);
        std::fs::write(dir.path().join(FILE), r#"{"uiScale": 125}"#).unwrap();
        assert_eq!(Settings::load(dir.path()).ui_scale, 125);
        // A settings file from before this field exists loads with the default.
        std::fs::write(dir.path().join(FILE), "{}").unwrap();
        assert_eq!(Settings::load(dir.path()).ui_scale, 100);

        let s = Settings {
            ui_scale: 89,
            ..Settings::default()
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn invalid_fields_fall_back_individually() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("settings.json"),
            r#"{"host":"http://x/y","timers":{"graceS":5},"language":"ar"}"#,
        )
        .unwrap();
        let s = Settings::load(d.path());
        assert_eq!(s.host, "192.168.8.1");
        assert_eq!(s.timers.grace_s, 90);
        assert_eq!(s.language, "ar");
        assert!(s.validate().is_ok());
    }

    #[test]
    fn save_load_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let s = Settings {
            language: "ar".into(),
            digits: Digits::ArabicIndic,
            palette: Palette::Sandstone,
            driver: DriverId::Mock,
            ..Settings::default()
        };
        s.save(d.path()).unwrap();
        assert_eq!(Settings::load(d.path()), s);
    }

    #[test]
    fn no_primary_band_means_no_relock() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("settings.json"),
            r#"{"automation":{"primaryBand":null,"relockPrimary":true}}"#,
        )
        .unwrap();
        assert!(!Settings::load(d.path()).automation.relock_primary);
        // An out-of-range band is dropped, and the re-lock with it.
        std::fs::write(
            d.path().join("settings.json"),
            r#"{"automation":{"primaryBand":200,"relockPrimary":true}}"#,
        )
        .unwrap();
        let s = Settings::load(d.path());
        assert_eq!(s.automation.primary_band, None);
        assert!(!s.automation.relock_primary);
        // What the UI sends when the star is cleared.
        let mut s = Settings::default();
        s.automation.relock_primary = true;
        s.normalize();
        assert!(!s.automation.relock_primary);
        s.automation.primary_band = Some(3);
        s.automation.relock_primary = true;
        s.normalize();
        assert!(s.automation.relock_primary, "kept with a primary band");
    }

    #[test]
    fn older_settings_get_history_defaults() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join(FILE), r#"{"language":"ar"}"#).unwrap();
        let s = Settings::load(d.path());
        assert_eq!(s.history, HistorySettings::default());
        assert_eq!(s.history.retention_days, Some(90));
        let auto = s.history.auto_speed_test;
        assert!(!auto.enabled);
        assert_eq!(auto.every_h, 3);
        assert_eq!(auto.provider, SpeedTestProvider::Cloudflare);
        assert_eq!(auto.size, SpeedTestSize::Light);
        // A partial history object keeps what it has.
        std::fs::write(
            d.path().join(FILE),
            r#"{"history":{"retentionDays":null,"autoSpeedTest":{"enabled":true}}}"#,
        )
        .unwrap();
        let s = Settings::load(d.path());
        assert_eq!(s.history.retention_days, None, "Forever");
        assert!(s.history.auto_speed_test.enabled);
        assert_eq!(s.history.auto_speed_test.every_h, 3);
    }

    #[test]
    fn history_bounds() {
        let with = |f: fn(&mut Settings)| {
            let mut s = Settings::default();
            f(&mut s);
            s.validate()
        };
        assert!(with(|s| s.history.retention_days = Some(6)).is_err());
        assert!(with(|s| s.history.retention_days = Some(3651)).is_err());
        assert!(with(|s| s.history.retention_days = Some(7)).is_ok());
        assert!(with(|s| s.history.retention_days = Some(3650)).is_ok());
        assert!(with(|s| s.history.retention_days = None).is_ok());
        assert!(with(|s| s.history.auto_speed_test.every_h = 0).is_err());
        assert!(with(|s| s.history.auto_speed_test.every_h = 169).is_err());
        assert!(with(|s| s.history.auto_speed_test.every_h = 1).is_ok());
        assert!(with(|s| s.history.auto_speed_test.every_h = 168).is_ok());

        // Out of range on disk: back to the defaults, one field at a time.
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join(FILE),
            r#"{"history":{"retentionDays":2,"autoSpeedTest":{"everyH":500,"enabled":true}}}"#,
        )
        .unwrap();
        let s = Settings::load(d.path());
        assert_eq!(s.history.retention_days, Some(90));
        assert_eq!(s.history.auto_speed_test.every_h, 3);
        assert!(s.history.auto_speed_test.enabled);
    }

    #[test]
    fn ookla_for_automatic_tests_needs_its_terms() {
        let mut s = Settings::default();
        s.history.auto_speed_test.provider = SpeedTestProvider::Ookla;
        assert!(s.validate().is_err());
        s.speed_test.ookla_terms_accepted = true;
        assert!(s.validate().is_ok());

        // Terms revoked: normalize falls back to Cloudflare.
        s.speed_test.ookla_terms_accepted = false;
        s.normalize();
        assert_eq!(
            s.history.auto_speed_test.provider,
            SpeedTestProvider::Cloudflare
        );
        assert!(s.validate().is_ok());

        // The same on load.
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join(FILE),
            r#"{"history":{"autoSpeedTest":{"provider":"ookla"}}}"#,
        )
        .unwrap();
        assert_eq!(
            Settings::load(d.path()).history.auto_speed_test.provider,
            SpeedTestProvider::Cloudflare
        );
    }

    #[test]
    fn settings_never_store_password() {
        let json = serde_json::to_string(&Settings::default()).unwrap();
        assert!(!json.to_lowercase().contains("password"));
        assert!(json.contains("huawei_h122"));
    }
}
