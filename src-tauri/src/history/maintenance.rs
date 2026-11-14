//! Keeping history.db small: pruning by the retention setting, and Clear history.
//!
//! Both run on the writer thread (`HistoryStore::exec`), so they never race a write. Pruning
//! deletes in batches, each its own transaction, so the writer never holds a long lock.

use std::sync::Arc;
use std::time::Duration;

use router_core::event_text::kinds;
use rusqlite::Connection;
use tauri::{AppHandle, Emitter, Manager};

use super::import::IMPORTED_SUFFIX;
use super::{log_event, HistoryStore};
use crate::speedtest;

/// Clear history is refused with this while a scan or speed test runs.
pub const ERR_BUSY: &str = "wait for the auto-scan or speed test to finish";
/// The Tauri event sent after Clear history.
pub const CLEARED_EVENT: &str = "history-cleared";

const DAY_MS: u64 = 24 * 3_600_000;
/// Rows deleted per statement.
pub const PRUNE_BATCH: usize = 5000;
/// Free pages given back to the file system after a prune.
const VACUUM_PAGES: u32 = 2000;
const FIRST_PRUNE: Duration = Duration::from_secs(60);
const PRUNE_EVERY: Duration = Duration::from_secs(6 * 3600);

/// The tables pruned and cleared, with their time column. `meta` and `routers` are kept, so
/// the events import carries on from its mark.
const TABLES: [(&str, &str); 3] = [
    ("signal_minute", "minute_ms"),
    ("speedtest", "at_ms"),
    ("event", "at_ms"),
];

/// Records older than this are removed; `None` (Forever) removes nothing.
pub fn cutoff_ms(retention_days: Option<u16>, now_ms: u64) -> Option<u64> {
    retention_days.map(|d| now_ms.saturating_sub(u64::from(d) * DAY_MS))
}

/// A retention change that keeps less than before, which prunes right away.
pub fn lowered(old: Option<u16>, new: Option<u16>) -> bool {
    match (old, new) {
        (_, None) => false,
        (None, Some(_)) => true,
        (Some(o), Some(n)) => n < o,
    }
}

/// Runs a pragma and reads (and drops) any rows it returns.
fn pragma(c: &Connection, sql: &str) -> rusqlite::Result<()> {
    let mut st = c.prepare(sql)?;
    let mut rows = st.query([])?;
    while rows.next()?.is_some() {}
    Ok(())
}

/// Deletes every row older than `cutoff_ms`, `batch` rows at a time, then gives free pages back
/// and truncates the WAL. Returns the number of rows deleted.
pub fn prune(c: &mut Connection, cutoff_ms: u64, batch: usize) -> rusqlite::Result<u64> {
    let mut total = 0u64;
    for (table, col) in TABLES {
        // signal_minute has no rowid: delete by its key.
        let (key, cols) = if table == "signal_minute" {
            ("(minute_ms, router_id)", "minute_ms, router_id")
        } else {
            ("rowid", "rowid")
        };
        let sql = format!(
            "DELETE FROM {table} WHERE {key} IN
                 (SELECT {cols} FROM {table} WHERE {col} < ?1 LIMIT ?2)"
        );
        loop {
            let n = c.execute(&sql, rusqlite::params![cutoff_ms as i64, batch as i64])?;
            total += n as u64;
            if n < batch {
                break;
            }
        }
    }
    if total > 0 {
        pragma(c, &format!("PRAGMA incremental_vacuum({VACUUM_PAGES})"))?;
    }
    pragma(c, "PRAGMA wal_checkpoint(TRUNCATE)")?;
    Ok(total)
}

/// Deletes every signal minute, speed test and event, keeps `meta` and `routers`, and VACUUMs.
/// The events import is moved to `now_ms`, so the cleared events aren't read back from
/// `events.jsonl` at the next start.
pub fn clear(c: &mut Connection, now_ms: u64) -> rusqlite::Result<()> {
    {
        let tx = c.transaction()?;
        for (table, _) in TABLES {
            tx.execute(&format!("DELETE FROM {table}"), [])?;
        }
        super::import::set_events_mark(&tx, now_ms)?;
        tx.commit()?;
    }
    c.execute_batch("VACUUM")?;
    pragma(c, "PRAGMA wal_checkpoint(TRUNCATE)")
}

/// Why Clear history can't run now.
pub fn clear_blocked(scan_running: bool, test_running: bool) -> Option<&'static str> {
    (scan_running || test_running).then_some(ERR_BUSY)
}

/// Prunes by `retention_days` and logs `history.pruned` when anything went. Blocking.
pub fn prune_store(
    store: &HistoryStore,
    data_dir: &std::path::Path,
    retention_days: Option<u16>,
    now_ms: u64,
) -> Result<u64, String> {
    let Some(cutoff) = cutoff_ms(retention_days, now_ms) else {
        return Ok(0);
    };
    if store.is_disabled() || store.is_read_only() {
        return Ok(0);
    }
    let rows = store.exec(move |c| prune(c, cutoff, PRUNE_BATCH))?;
    if rows > 0 {
        log_event(
            data_dir,
            kinds::HISTORY_PRUNED,
            serde_json::json!({ "rows": rows, "cutoffMs": cutoff }),
        );
    }
    Ok(rows)
}

/// Clears the database and deletes `speedtests.jsonl.imported`, and `speedtests.jsonl` too: it
/// was imported at open (or holds fallback results written since), and a copy left behind by a
/// failed rename would bring the cleared tests back at the next start. Never touches
/// `events.jsonl`, `journal-archive`, `pending-change.json` or `scan.json`. Blocking.
pub fn clear_store(store: &HistoryStore, data_dir: &std::path::Path) -> Result<(), String> {
    let now = super::now_ms();
    store.exec(move |c| clear(c, now))?;
    let file = speedtest::history_path(data_dir);
    let mut imported = file.clone().into_os_string();
    imported.push(IMPORTED_SUFFIX);
    for name in [file.into_os_string(), imported] {
        match std::fs::remove_file(&name) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("history: couldn't delete {name:?}: {e}"),
        }
    }
    log_event(data_dir, kinds::HISTORY_CLEARED, serde_json::json!({}));
    Ok(())
}

fn prune_with(app: &AppHandle) {
    let (Some(store), Some(state)) = (
        app.try_state::<Arc<HistoryStore>>(),
        app.try_state::<crate::commands::AppState>(),
    ) else {
        return;
    };
    let days = state.history_settings().retention_days;
    if let Err(e) = prune_store(&store, state.data_dir(), days, super::now_ms()) {
        eprintln!("history: pruning failed: {e}");
    }
}

/// Prunes now, off the async runtime: after a save that lowered the retention.
pub fn prune_soon(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || prune_with(&app));
}

/// Prunes 60 s after start, then every 6 hours.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_PRUNE).await;
        loop {
            let a = app.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || prune_with(&a)).await;
            tokio::time::sleep(PRUNE_EVERY).await;
        }
    });
}

/// Clear history, for the command: refused while a scan or speed test runs.
pub fn clear_with(app: &AppHandle) -> Result<(), String> {
    let scan = app
        .try_state::<crate::autoscan::ScanState>()
        .is_some_and(|s| s.is_running());
    let test = app
        .try_state::<speedtest::SpeedTestState>()
        .is_some_and(|s| s.is_running());
    if let Some(why) = clear_blocked(scan, test) {
        return Err(why.into());
    }
    let (Some(store), Some(state)) = (
        app.try_state::<Arc<HistoryStore>>(),
        app.try_state::<crate::commands::AppState>(),
    ) else {
        return Err(super::store::ERR_UNAVAILABLE.into());
    };
    clear_store(&store, state.data_dir())?;
    let _ = app.emit(CLEARED_EVENT, ());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::sampler::{upsert_minute, MinuteRow};
    use crate::history::schema;
    use crate::history::store::{insert_event_on, router_id};
    use router_core::EventRecord;

    fn db() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        schema::migrate(&mut c).unwrap();
        c
    }

    fn count(c: &Connection, table: &str) -> i64 {
        c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    fn speed_row(c: &Connection, at: u64) {
        c.execute(
            "INSERT INTO speedtest (at_ms, provider, source, size, bytes_used, valid, record_json)
             VALUES (?1, 'cloudflare', 'manual', 'light', 0, 1, '{}')",
            [at as i64],
        )
        .unwrap();
    }

    fn seed(c: &Connection, times: &[u64]) {
        let id = router_id(c, "192.168.8.1", "mock").unwrap();
        for &t in times {
            upsert_minute(
                c,
                id,
                &MinuteRow {
                    minute_ms: t,
                    alive_ms: 60_000,
                    ..MinuteRow::default()
                },
            )
            .unwrap();
            speed_row(c, t);
            insert_event_on(
                c,
                &EventRecord {
                    at_ms: t,
                    kind: kinds::APP_STARTED.into(),
                    detail: serde_json::json!({}),
                },
            )
            .unwrap();
        }
    }

    #[test]
    fn the_cutoff_follows_the_retention() {
        let now = 100 * DAY_MS;
        assert_eq!(cutoff_ms(Some(90), now), Some(10 * DAY_MS));
        assert_eq!(cutoff_ms(None, now), None, "Forever");
        assert_eq!(cutoff_ms(Some(3650), now), Some(0));
    }

    #[test]
    fn lowering_the_retention_prunes_now() {
        assert!(lowered(Some(90), Some(30)));
        assert!(lowered(None, Some(365)));
        assert!(!lowered(Some(30), Some(90)));
        assert!(!lowered(Some(30), None));
        assert!(!lowered(Some(30), Some(30)));
    }

    #[test]
    fn prune_removes_old_rows_from_all_three_tables() {
        let mut c = db();
        seed(&c, &[60_000, 120_000, 180_000, 240_000]);
        let n = prune(&mut c, 180_000, PRUNE_BATCH).unwrap();
        assert_eq!(n, 6, "two of each");
        for t in ["signal_minute", "speedtest", "event"] {
            assert_eq!(count(&c, t), 2, "{t}");
        }
        let oldest: i64 = c
            .query_row("SELECT MIN(minute_ms) FROM signal_minute", [], |r| r.get(0))
            .unwrap();
        assert_eq!(oldest, 180_000, "the cutoff itself is kept");
        assert_eq!(prune(&mut c, 180_000, PRUNE_BATCH).unwrap(), 0);
    }

    #[test]
    fn prune_works_in_batches() {
        let mut c = db();
        let tx = c.transaction().unwrap();
        for i in 0..12_000u64 {
            speed_row(&tx, i);
        }
        tx.commit().unwrap();
        assert_eq!(prune(&mut c, 11_000, PRUNE_BATCH).unwrap(), 11_000);
        assert_eq!(count(&c, "speedtest"), 1000);
    }

    #[test]
    fn forever_never_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let store = HistoryStore::open(dir.path());
        assert!(store.flush(Duration::from_secs(5)));
        assert_eq!(
            prune_store(&store, dir.path(), None, u64::MAX / 2).unwrap(),
            0
        );
        store.shutdown();
    }

    #[test]
    fn a_prune_through_the_store_is_logged() {
        let dir = tempfile::tempdir().unwrap();
        let store = HistoryStore::open(dir.path());
        store
            .exec(|c| {
                seed(c, &[1_000, 2_000]);
                Ok(())
            })
            .unwrap();
        let now = 8 * DAY_MS;
        assert_eq!(prune_store(&store, dir.path(), Some(7), now).unwrap(), 6);
        let log = std::fs::read_to_string(dir.path().join("events.jsonl")).unwrap();
        assert!(log.contains(kinds::HISTORY_PRUNED));
        assert!(log.contains(&format!("\"cutoffMs\":{}", DAY_MS)));
        store.shutdown();
    }

    #[test]
    fn clear_is_refused_while_a_scan_or_test_runs() {
        assert_eq!(clear_blocked(true, false), Some(ERR_BUSY));
        assert_eq!(clear_blocked(false, true), Some(ERR_BUSY));
        assert_eq!(clear_blocked(false, false), None);
    }

    #[test]
    fn clear_empties_the_tables_and_keeps_meta() {
        let dir = tempfile::tempdir().unwrap();
        let store = HistoryStore::open(dir.path());
        let meta = store
            .exec(|c| {
                seed(c, &[1_000, 2_000]);
                c.execute(
                    "INSERT OR REPLACE INTO meta (key, value) VALUES ('import.events', '1')",
                    [],
                )?;
                Ok(count(c, "meta"))
            })
            .unwrap();
        assert!(meta >= 1);
        let imported = dir.path().join("speedtests.jsonl.imported");
        std::fs::write(&imported, "{}\n").unwrap();
        // Left behind by a rename that failed after the import.
        let leftover = speedtest::history_path(dir.path());
        std::fs::write(&leftover, "{}\n").unwrap();
        std::fs::write(dir.path().join("scan.json"), "{}").unwrap();
        clear_store(&store, dir.path()).unwrap();
        assert!(store.flush(Duration::from_secs(5)));
        let left = store
            .with_read(|c| {
                Ok((
                    count(c, "signal_minute"),
                    count(c, "speedtest"),
                    c.query_row(
                        "SELECT COUNT(*) FROM event WHERE kind != ?1",
                        [kinds::HISTORY_CLEARED],
                        |r| r.get::<_, i64>(0),
                    )?,
                    count(c, "meta"),
                    count(c, "routers"),
                ))
            })
            .unwrap();
        assert_eq!(left, (0, 0, 0, meta, 1));
        assert!(!imported.exists());
        assert!(!leftover.exists(), "it would bring the cleared tests back");
        assert!(dir.path().join("scan.json").exists(), "never touched");
        let log = std::fs::read_to_string(dir.path().join("events.jsonl")).unwrap();
        assert!(log.contains(kinds::HISTORY_CLEARED));
        store.shutdown();
    }
}
