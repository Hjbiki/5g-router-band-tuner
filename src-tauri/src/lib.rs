pub mod autoscan;
pub mod band_history;
#[cfg(windows)]
pub mod capture_window;
pub mod commands;
pub mod credentials;
pub mod history;
pub mod notify;
pub mod privacy_check;
pub mod router_probe;
pub mod settings;
pub mod speedtest;
pub mod tray;
pub mod updates;

use std::sync::Arc;

use tauri::{Manager, RunEvent, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

use commands::AppState;
use credentials::KeyringStore;
use settings::Settings;

/// Enable or disable launch at login; failures are not fatal.
pub(crate) fn sync_autostart(app: &tauri::AppHandle, enabled: bool) {
    let launcher = app.autolaunch();
    if launcher.is_enabled().unwrap_or(!enabled) != enabled {
        let _ = if enabled {
            launcher.enable()
        } else {
            launcher.disable()
        };
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main(app);
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    tray::hide_to_tray(window.app_handle());
                }
            }
        })
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            // Fail rather than start empty: the old folder may hold an unconfirmed change.
            router_core::legacy::migrate_data_dir(&data_dir)?;
            std::fs::create_dir_all(&data_dir)?;
            let settings = Settings::load(&data_dir);
            // Before anything can log: every event from here on is mirrored into history.db.
            let history = history::install(&data_dir);
            app.manage(history);
            history::log_event(
                &data_dir,
                router_core::event_text::kinds::APP_STARTED,
                serde_json::json!({ "version": app.package_info().version.to_string() }),
            );
            sync_autostart(app.handle(), settings.start_at_login);
            let state = tauri::async_runtime::block_on(async {
                AppState::start(
                    app.handle().clone(),
                    settings,
                    data_dir,
                    Arc::new(KeyringStore),
                )
            })?;
            app.manage(state);
            app.manage(speedtest::SpeedTestState::default());
            app.manage(speedtest::schedule::AutoTestState::default());
            app.manage(autoscan::ScanState::default());
            autoscan::on_start(app.handle());
            updates::start(app.handle());
            app.manage(router_probe::ProbeState::default());
            router_probe::start(app.handle());
            history::start(app.handle());
            speedtest::schedule::start(app.handle());
            tray::init(app.handle())?;
            // Connect by itself once the actor and tray are up (C2): a band change resumed
            // from the journal must be watched even if the window is never opened.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Some(state) = handle.try_state::<AppState>() {
                    state.auto_connect().await;
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::save_credentials,
            commands::forget_credentials,
            commands::connect,
            commands::connect_trusting,
            commands::logout,
            commands::ip_refresh_start,
            commands::ip_refresh_stop,
            commands::ip_reattach_start,
            commands::data_switch_on,
            commands::apn_profiles,
            commands::apn_refresh,
            commands::apn_switch,
            commands::apn_preview,
            commands::apn_add,
            commands::apn_edit,
            commands::apn_delete,
            commands::credential_status,
            commands::get_snapshot,
            commands::preview_config,
            commands::apply_config,
            commands::keep_now,
            commands::revert_now,
            commands::request_reboot,
            commands::confirm_reboot,
            band_history::band_outcomes,
            band_history::scan_periods,
            autoscan::get_scan,
            autoscan::start_scan,
            autoscan::continue_scan,
            autoscan::stop_scan,
            autoscan::dismiss_scan,
            router_probe::router_probe_status,
            router_probe::prepare_router_report,
            router_probe::open_router_issue,
            updates::update_status,
            updates::check_for_updates,
            updates::dismiss_update,
            updates::open_release_page,
            commands::request_factory_reset,
            commands::confirm_factory_reset,
            commands::set_visible,
            commands::open_developer_page,
            commands::prepare_diagnostics,
            commands::save_diagnostics,
            commands::prepare_page_scripts,
            commands::save_page_scripts,
            privacy_check::privacy_check,
            privacy_check::dns_check,
            // Speed test
            speedtest::run_speed_test,
            speedtest::cancel_speed_test,
            speedtest::speed_test_running,
            speedtest::speed_test_history,
            speedtest::open_ookla_terms,
            speedtest::schedule::auto_speed_test_status,
            // History
            history::commands::history_overview,
            history::commands::history_signal,
            history::commands::history_speedtests,
            history::commands::history_events,
            history::commands::clear_history,
            history::commands::export_history_csv,
        ])
        .build(tauri::generate_context!())
        .expect("error while building 5G Router Band Tuner")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                if let Some(state) = app.try_state::<commands::AppState>() {
                    state.finish_ip_refresh_at_exit();
                }
                history::on_exit(app);
            }
        });
}
