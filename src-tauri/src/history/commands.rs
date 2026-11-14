//! The History page's commands. Every query runs off the async runtime on the read connection.

use std::sync::Arc;

use router_core::SessionView;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use ts_rs::TS;

use super::csv::{self, CsvLabels, ExportDataset, ExportOptions, ExportOutcome, Target};
use super::query::{self, Bucket, EventPage, EventQuery, HistoryRange, SignalSeries};
use super::store::{self, HistoryStore};
use crate::commands::{AppState, CmdError};
use crate::speedtest::SpeedTestResult;

type CmdResult<T> = Result<T, CmdError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistoryOverview {
    /// The database could be opened. When `false`, `unavailableReason` says why.
    pub available: bool,
    pub unavailable_reason: Option<String>,
    /// Made by a newer version of the app: shown, never written.
    pub read_only: bool,
    #[ts(type = "number | null")]
    pub first_ms: Option<u64>,
    #[ts(type = "number | null")]
    pub last_ms: Option<u64>,
    /// Signal minutes stored.
    #[ts(type = "number")]
    pub minutes: u64,
    #[ts(type = "number")]
    pub speedtests: u64,
    #[ts(type = "number")]
    pub events: u64,
    /// The file and its WAL, in bytes.
    #[ts(type = "number")]
    pub db_bytes: u64,
    pub retention_days: Option<u16>,
    /// Signal is being recorded now: the app is logged in to the router.
    pub recording: bool,
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> CmdResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| CmdError::Message(e.to_string()))?
        .map_err(CmdError::Message)
}

fn db_bytes(data_dir: &std::path::Path) -> u64 {
    let main = store::db_path(data_dir);
    let wal = main.with_file_name(format!("{}-wal", store::FILE));
    [main, wal]
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum()
}

pub fn overview(
    store: &HistoryStore,
    data_dir: &std::path::Path,
    retention_days: Option<u16>,
    recording: bool,
) -> HistoryOverview {
    let mut o = HistoryOverview {
        available: false,
        unavailable_reason: store.unavailable_reason().map(str::to_owned),
        read_only: store.is_read_only(),
        first_ms: None,
        last_ms: None,
        minutes: 0,
        speedtests: 0,
        events: 0,
        db_bytes: 0,
        retention_days,
        recording,
    };
    if store.is_disabled() {
        return o;
    }
    match store.with_read(query::counts) {
        Ok(n) => {
            o.available = true;
            o.first_ms = n.first_ms;
            o.last_ms = n.last_ms;
            o.minutes = n.minutes;
            o.speedtests = n.speedtests;
            o.events = n.events;
            o.db_bytes = db_bytes(data_dir);
        }
        Err(e) => o.unavailable_reason = Some(e),
    }
    o
}

#[tauri::command]
pub async fn history_overview(
    state: State<'_, AppState>,
    history: State<'_, Arc<HistoryStore>>,
) -> CmdResult<HistoryOverview> {
    let store = history.inner().clone();
    let dir = state.data_dir().to_path_buf();
    let days = state.history_settings().retention_days;
    let recording = matches!(state.snapshot().session, SessionView::LoggedIn);
    blocking(move || Ok(overview(&store, &dir, days, recording))).await
}

#[tauri::command]
pub async fn history_signal(
    history: State<'_, Arc<HistoryStore>>,
    range: HistoryRange,
    bucket: Bucket,
) -> CmdResult<SignalSeries> {
    let store = history.inner().clone();
    blocking(move || store.with_read(|c| query::signal(c, &range, bucket))).await
}

/// Speed tests in the range, oldest first. While the database is unavailable or read-only, the
/// fallback file is read too.
#[tauri::command]
pub async fn history_speedtests(
    state: State<'_, AppState>,
    history: State<'_, Arc<HistoryStore>>,
    range: HistoryRange,
) -> CmdResult<Vec<SpeedTestResult>> {
    let store = history.inner().clone();
    let dir = state.data_dir().to_path_buf();
    blocking(move || super::speedtests_in_range(&store, &dir, &range)).await
}

#[tauri::command]
pub async fn history_events(
    history: State<'_, Arc<HistoryStore>>,
    q: EventQuery,
) -> CmdResult<EventPage> {
    let store = history.inner().clone();
    blocking(move || store.with_read(|c| query::events(c, &q))).await
}

/// Deletes the stored signal, speed tests and events. Refused while a scan or speed test runs.
#[tauri::command]
pub async fn clear_history(app: AppHandle) -> CmdResult<()> {
    blocking(move || super::maintenance::clear_with(&app)).await
}

/// Builds the CSV in memory, runs the privacy check, then asks where to save it: a file for one
/// dataset, a folder for all four. Nothing is written when the check finds something.
#[tauri::command]
pub async fn export_history_csv(
    app: AppHandle,
    state: State<'_, AppState>,
    history: State<'_, Arc<HistoryStore>>,
    opts: ExportOptions,
    labels: CsvLabels,
) -> CmdResult<ExportOutcome> {
    use tauri_plugin_dialog::DialogExt;
    let store = history.inner().clone();
    let dir = state.data_dir().to_path_buf();
    blocking(move || {
        let fmt = csv::locale_format();
        let files = store.with_read(|c| csv::build(c, &opts, &labels, fmt))?;
        csv::privacy_check(&files)?;
        let now = super::now_ms();
        let offset = opts.range.utc_offset_min;
        let target = if opts.dataset == ExportDataset::All {
            app.dialog()
                .file()
                .blocking_pick_folder()
                .map(|p| p.into_path().map(Target::Folder))
        } else {
            app.dialog()
                .file()
                .set_file_name(csv::file_name(opts.dataset, now, offset))
                .add_filter("CSV", &["csv"])
                .blocking_save_file()
                .map(|p| p.into_path().map(Target::File))
        };
        let Some(target) = target else {
            return Ok(ExportOutcome::Cancelled);
        };
        let target = target.map_err(|e| e.to_string())?;
        let names = csv::save(&files, &target, now, offset)?;
        let rows: usize = files.iter().map(|f| f.rows).sum();
        super::log_event(
            &dir,
            router_core::event_text::kinds::HISTORY_EXPORTED,
            serde_json::json!({
                "dataset": opts.dataset,
                "rows": rows,
                "cellNumbers": opts.include_cell_numbers,
                "ipRange": opts.include_ip_range,
            }),
        );
        Ok(ExportOutcome::Saved { files: names })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disabled_store_reports_why() {
        let dir = tempfile::tempdir().unwrap();
        let o = overview(
            &HistoryStore::Disabled("broken".into()),
            dir.path(),
            Some(90),
            true,
        );
        assert!(!o.available);
        assert_eq!(o.unavailable_reason.as_deref(), Some("broken"));
        assert_eq!(o.retention_days, Some(90));
        assert!(o.recording);
    }

    #[test]
    fn an_open_store_counts_its_rows_and_size() {
        let dir = tempfile::tempdir().unwrap();
        let s = HistoryStore::open(dir.path());
        s.mirror_event(&router_core::EventRecord {
            at_ms: 42,
            kind: "app.started".into(),
            detail: serde_json::json!({}),
        });
        assert!(s.flush(std::time::Duration::from_secs(5)));
        let o = overview(&s, dir.path(), None, false);
        assert!(o.available && !o.read_only);
        assert_eq!(o.events, 1);
        assert_eq!((o.first_ms, o.last_ms), (Some(42), Some(42)));
        assert!(o.db_bytes > 0);
        s.shutdown();
    }
}
