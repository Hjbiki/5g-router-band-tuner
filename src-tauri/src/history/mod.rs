//! M2 history: signal minutes, speed tests and a sanitized copy of every event, in one local
//! SQLite file (`<app data>/history.db`). `events.jsonl` stays the safety audit log; this is the
//! copy the History page and the CSV export read. Nothing here leaves the PC.
//!
//! The file never holds a cell ID, eNB ID, sector, TAC, IMEI, IMSI, ICCID, IP or MAC: events
//! go through `router_core::sanitize_detail` on the way in.

pub mod commands;
pub mod csv;
pub mod import;
pub mod maintenance;
pub mod query;
pub mod sampler;
pub mod schema;
pub mod store;

use std::sync::Arc;
use std::time::Duration;

use router_core::event_text::kinds;
use router_core::{EventLog, EventRecord};
use tauri::{AppHandle, Manager};

use crate::speedtest::{self, SpeedTestResult};

pub use store::{HistoryStore, WriteMsg};

/// How long the app waits at exit for queued writes.
const EXIT_FLUSH: Duration = Duration::from_secs(2);
/// How long a read of the speed tests waits for queued writes, so the page that reloads sees them.
const SPEED_FLUSH: Duration = Duration::from_secs(2);
/// How long a finished speed test waits for its row before it goes to the fallback file instead.
const SPEED_WRITE: Duration = Duration::from_secs(10);
/// `speed_test_history` returns at least every result of this many days (Auto-scan's window).
const RECENT_DAYS: u64 = 7;
/// How often dropped events are reported.
const DROPPED_EVERY: Duration = Duration::from_secs(60);

fn now_ms() -> u64 {
    crate::notify::now_ms()
}

/// Appends one app event to `events.jsonl` (and so, through the sink, to the database).
pub(crate) fn log_event(data_dir: &std::path::Path, kind: &str, detail: serde_json::Value) {
    let rec = EventRecord {
        at_ms: now_ms(),
        kind: kind.into(),
        detail,
    };
    let _ = EventLog::new(data_dir.to_path_buf()).append(&rec);
}

/// Opens the store and routes every event-log record into it. Returns the store for
/// `app.manage`.
pub fn install(data_dir: &std::path::Path) -> Arc<HistoryStore> {
    let store = Arc::new(HistoryStore::open(data_dir));
    if !store.is_disabled() {
        let sink = store.clone();
        router_core::set_event_sink(Box::new(move |e| sink.mirror_event(e)));
    }
    store
}

/// Reports dropped events once a minute, as `history.dropped {count}`, and starts pruning.
pub fn start(app: &AppHandle) {
    maintenance::start(app);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(DROPPED_EVERY).await;
            let (Some(store), Some(state)) = (
                app.try_state::<Arc<HistoryStore>>(),
                app.try_state::<crate::commands::AppState>(),
            ) else {
                continue;
            };
            let count = store.take_dropped();
            if count > 0 {
                let dir = state.data_dir().to_path_buf();
                let _ = tokio::task::spawn_blocking(move || {
                    log_event(
                        &dir,
                        kinds::HISTORY_DROPPED,
                        serde_json::json!({ "count": count }),
                    )
                })
                .await;
            }
        }
    });
}

/// At exit: log `app.stopped` and give the writer a moment to commit.
pub fn on_exit(app: &AppHandle) {
    if let Some(state) = app.try_state::<crate::commands::AppState>() {
        log_event(state.data_dir(), kinds::APP_STOPPED, serde_json::json!({}));
    }
    if let Some(store) = app.try_state::<Arc<HistoryStore>>() {
        store.flush(EXIT_FLUSH);
    }
}

/// Stores a finished speed test. While the database can't take it (unavailable, read-only, or
/// the write failed or didn't finish in time), the result goes to the old `speedtests.jsonl`
/// instead, which the next start imports. Blocking: call off the runtime.
pub fn record_speedtest(app: &AppHandle, r: &SpeedTestResult) {
    let Some(state) = app.try_state::<crate::commands::AppState>() else {
        return;
    };
    let (host, driver, _) = state.scan_identity();
    let stored = app
        .try_state::<Arc<HistoryStore>>()
        .is_some_and(|store| store_speedtest(&store, r, Some((host, driver))).is_ok());
    if !stored {
        let _ = speedtest::append_history(state.data_dir(), r);
    }
}

/// Writes one result in its own transaction and waits for the real answer.
fn store_speedtest(
    store: &HistoryStore,
    r: &SpeedTestResult,
    router: Option<(String, String)>,
) -> Result<(), String> {
    let r = r.clone();
    store.exec_within(SPEED_WRITE, move |c| {
        let tx = c.transaction()?;
        let id = match &router {
            Some((host, driver)) => Some(store::router_id(&tx, host, driver)?),
            None => None,
        };
        store::insert_speedtest(&tx, &r, id, speedtest::SpeedTestSource::Manual)?;
        tx.commit()
    })
}

/// Adds the fallback file's results to what the database gave, without doubles (same time and
/// provider). Used while the database is read-only, when new results only reach the file.
fn merge_fallback(
    mut rows: Vec<SpeedTestResult>,
    file: Vec<SpeedTestResult>,
) -> Vec<SpeedTestResult> {
    for r in file {
        if !rows
            .iter()
            .any(|x| x.at_ms == r.at_ms && x.provider == r.provider)
        {
            rows.push(r);
        }
    }
    rows
}

/// Speed tests in `from_ms..=to_ms`, oldest first, for the History page. While the database is
/// unavailable they come from the fallback file, and while it's read-only the file is added.
pub fn speedtests_in_range(
    store: &HistoryStore,
    data_dir: &std::path::Path,
    range: &query::HistoryRange,
) -> Result<Vec<SpeedTestResult>, String> {
    let in_range = |r: &SpeedTestResult| r.at_ms >= range.from_ms && r.at_ms <= range.to_ms;
    if store.is_disabled() {
        return Ok(speedtest::load_history(data_dir)
            .into_iter()
            .filter(in_range)
            .collect());
    }
    let rows = store.with_read(|c| query::speedtests(c, range))?;
    if !store.is_read_only() {
        return Ok(rows);
    }
    let file = speedtest::load_history(data_dir)
        .into_iter()
        .filter(in_range)
        .collect();
    let mut all = merge_fallback(rows, file);
    all.sort_by_key(|r| r.at_ms);
    Ok(all)
}

/// What `speed_test_history` returns, newest first: the newest `speedtest::HISTORY_CAP` results,
/// or every result of the last `RECENT_DAYS` days when that's more, so Auto-scan's 7-day averages
/// see the whole window. Falls back to `speedtests.jsonl` while the database is unavailable, and
/// adds it while the database is read-only.
pub fn recent_speedtests(
    store: &HistoryStore,
    data_dir: &std::path::Path,
) -> Result<Vec<SpeedTestResult>, String> {
    recent_speedtests_at(store, data_dir, now_ms())
}

fn recent_speedtests_at(
    store: &HistoryStore,
    data_dir: &std::path::Path,
    now_ms: u64,
) -> Result<Vec<SpeedTestResult>, String> {
    let floor = now_ms.saturating_sub(RECENT_DAYS * 24 * 3_600_000);
    let newest_first = |mut h: Vec<SpeedTestResult>| {
        h.sort_by(|a, b| b.at_ms.cmp(&a.at_ms));
        let keep = h
            .iter()
            .filter(|r| r.at_ms >= floor)
            .count()
            .max(speedtest::HISTORY_CAP);
        h.truncate(keep);
        h
    };
    if store.is_disabled() {
        return Ok(newest_first(speedtest::load_history(data_dir)));
    }
    // Waits for the start-up import and any queued result.
    store.flush(SPEED_FLUSH);
    let rows: Vec<String> = store.with_read(|c| {
        let mut st = c.prepare(
            "SELECT record_json FROM speedtest
             WHERE at_ms >= ?2
                OR id IN (SELECT id FROM speedtest ORDER BY at_ms DESC, id DESC LIMIT ?1)
             ORDER BY at_ms DESC, id DESC",
        )?;
        let rows = st.query_map(
            rusqlite::params![speedtest::HISTORY_CAP as i64, floor as i64],
            |r| r.get(0),
        )?;
        rows.collect()
    })?;
    let rows: Vec<SpeedTestResult> = rows
        .iter()
        .filter_map(|j| serde_json::from_str(j).ok())
        .collect();
    if !store.is_read_only() {
        return Ok(rows);
    }
    Ok(newest_first(merge_fallback(
        rows,
        speedtest::load_history(data_dir),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speedtest::SpeedTestSource;

    fn result(at: u64) -> SpeedTestResult {
        serde_json::from_value(serde_json::json!({
            "atMs": at,
            "provider": "cloudflare",
            "requested": "cloudflare",
            "fallback": null,
            "fallbackDetail": null,
            "size": "light",
            "serving": {"lteBand": 3, "ltePci": 103, "nrBand": 41},
            "idlePingMs": 40.0,
            "downloadMbps": 100.0,
            "uploadMbps": 10.0,
            "pingDownMs": 300.0,
            "pingUpMs": null,
            "bytesUsed": 1000,
            "server": "Cloudflare DXB",
            "sizeApplies": true,
            "allowedBands": "4",
        }))
        .unwrap()
    }

    #[test]
    fn an_older_record_without_a_source_still_reads() {
        let r = result(1);
        assert_eq!(r.source, None);
        let v = serde_json::to_value(&r).unwrap();
        assert!(v.get("source").is_some(), "always written, as null");
    }

    #[test]
    fn history_from_the_database_keeps_its_shape_and_cap() {
        let dir = tempfile::tempdir().unwrap();
        let s = HistoryStore::open(dir.path());
        for i in 0..205 {
            let mut r = result(1_000 + i);
            r.source = Some(SpeedTestSource::Manual);
            s.send(WriteMsg::Speed {
                result: Box::new(r),
                router: Some(("192.168.8.1".into(), "mock".into())),
            })
            .unwrap();
        }
        let got = recent_speedtests(&s, dir.path()).unwrap();
        assert_eq!(got.len(), speedtest::HISTORY_CAP);
        assert_eq!(got[0].at_ms, 1_204, "newest first");
        assert_eq!(got.last().unwrap().at_ms, 1_005);
        // Exactly the JSON the file-based history gave.
        let mut want = result(1_204);
        want.source = Some(SpeedTestSource::Manual);
        assert_eq!(
            serde_json::to_value(&got[0]).unwrap(),
            serde_json::to_value(&want).unwrap()
        );
        s.shutdown();
    }

    #[test]
    fn the_last_seven_days_are_returned_even_past_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let s = HistoryStore::open(dir.path());
        let now = 100 * 24 * 3_600_000;
        let day = 24 * 3_600_000;
        // 10 older than the window, then 250 inside it.
        for i in 0..10 {
            s.send(WriteMsg::Speed {
                result: Box::new(result(now - 9 * day + i)),
                router: None,
            })
            .unwrap();
        }
        for i in 0..250 {
            s.send(WriteMsg::Speed {
                result: Box::new(result(now - 6 * day + i)),
                router: None,
            })
            .unwrap();
        }
        let got = recent_speedtests_at(&s, dir.path(), now).unwrap();
        assert_eq!(got.len(), 250);
        assert!(got.iter().all(|r| r.at_ms >= now - 7 * day));
        s.shutdown();
    }

    #[test]
    fn a_read_only_store_adds_the_fallback_file() {
        let dir = tempfile::tempdir().unwrap();
        let s = HistoryStore::open(dir.path());
        s.send(WriteMsg::Speed {
            result: Box::new(result(10)),
            router: None,
        })
        .unwrap();
        s.shutdown();
        drop(s);
        {
            let c = rusqlite::Connection::open(store::db_path(dir.path())).unwrap();
            c.pragma_update(None, "user_version", 99).unwrap();
        }
        let s = HistoryStore::open(dir.path());
        assert!(s.is_read_only());
        // A result run after the downgrade only reaches the file; one copy of an old one too.
        speedtest::append_history(dir.path(), &result(10)).unwrap();
        speedtest::append_history(dir.path(), &result(20)).unwrap();
        let got = recent_speedtests_at(&s, dir.path(), 30).unwrap();
        assert_eq!(
            got.iter().map(|r| r.at_ms).collect::<Vec<_>>(),
            vec![20, 10]
        );
        let range = query::HistoryRange {
            from_ms: 0,
            to_ms: 15,
            utc_offset_min: 0,
        };
        let got = speedtests_in_range(&s, dir.path(), &range).unwrap();
        assert_eq!(got.iter().map(|r| r.at_ms).collect::<Vec<_>>(), vec![10]);
        let range = query::HistoryRange { to_ms: 30, ..range };
        let got = speedtests_in_range(&s, dir.path(), &range).unwrap();
        assert_eq!(
            got.iter().map(|r| r.at_ms).collect::<Vec<_>>(),
            vec![10, 20]
        );
    }

    #[test]
    fn a_disabled_store_reads_the_fallback_file() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..3 {
            speedtest::append_history(dir.path(), &result(i)).unwrap();
        }
        let s = HistoryStore::Disabled("broken".into());
        let got = recent_speedtests(&s, dir.path()).unwrap();
        assert_eq!(
            got.iter().map(|r| r.at_ms).collect::<Vec<_>>(),
            vec![2, 1, 0]
        );
    }
}
