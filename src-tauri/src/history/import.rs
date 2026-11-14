//! Brings older files into the database, in the writer thread right after it opens:
//!
//! 1. `speedtests.jsonl`, every start: the pre-M2 log, or results written as the fallback while
//!    the database was unavailable. Afterwards it's renamed to `speedtests.jsonl.imported`.
//! 2. `events.jsonl` and its rotated copies, every start, sanitized: the lines from the newest
//!    time the last import saw onwards (`meta` `import.events.mark`). That brings in events the
//!    live copy missed: logged while the database was unavailable, dropped from a full queue, or
//!    written by band-runner.
//!
//! Both use INSERT OR IGNORE, so running it again stores nothing twice. Unreadable lines are
//! skipped.

use std::path::Path;

use router_core::event_text::kinds;
use router_core::{sanitize_detail, EventLog, EventRecord};
use rusqlite::{params, Connection, OptionalExtension};

use super::store::{insert_event_on, insert_speedtest};
use crate::speedtest::{self, SpeedTestResult, SpeedTestSource};

pub const IMPORTED_SUFFIX: &str = ".imported";
/// Set by the first app versions, which imported `events.jsonl` once.
const EVENTS_DONE_KEY: &str = "import.events";
/// Lines older than this time were imported already.
const EVENTS_MARK_KEY: &str = "import.events.mark";
const EVENT_FILES: [&str; 4] = [
    "events.jsonl.3",
    "events.jsonl.2",
    "events.jsonl.1",
    "events.jsonl",
];

/// What one run stored.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Imported {
    pub speedtests: u64,
    pub events: u64,
}

/// A result's source when the record doesn't say: Scan when it ran during an auto-scan.
pub fn infer_source(at_ms: u64, scan_periods: &[(u64, u64)]) -> SpeedTestSource {
    if scan_periods
        .iter()
        .any(|&(from, to)| at_ms >= from && at_ms <= to)
    {
        SpeedTestSource::Scan
    } else {
        SpeedTestSource::Manual
    }
}

fn import_speedtests(conn: &mut Connection, data_dir: &Path) -> rusqlite::Result<u64> {
    let path = speedtest::history_path(data_dir);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(0);
    };
    let periods = crate::band_history::load_scan_periods(data_dir);
    let tx = conn.transaction()?;
    let mut n = 0;
    for r in text
        .lines()
        .filter_map(|l| serde_json::from_str::<SpeedTestResult>(l).ok())
    {
        let source = infer_source(r.at_ms, &periods);
        if insert_speedtest(&tx, &r, None, source)? {
            n += 1;
        }
    }
    tx.commit()?;
    let mut done = path.into_os_string();
    done.push(IMPORTED_SUFFIX);
    if let Err(e) = std::fs::rename(speedtest::history_path(data_dir), &done) {
        eprintln!("history: couldn't rename the imported speed tests: {e}");
    }
    Ok(n)
}

fn meta(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .optional()
}

/// Lines of `events.jsonl` older than `at_ms` aren't imported again: after Clear history, so the
/// cleared events don't come back at the next start.
pub fn set_events_mark(conn: &Connection, at_ms: u64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
        params![EVENTS_MARK_KEY, at_ms.to_string()],
    )?;
    Ok(())
}

fn import_events(conn: &mut Connection, data_dir: &Path) -> rusqlite::Result<u64> {
    let mark = match meta(conn, EVENTS_MARK_KEY)?.and_then(|v| v.parse::<u64>().ok()) {
        Some(m) => Some(m),
        // Imported once by an earlier version: carry on from the newest event stored.
        None if meta(conn, EVENTS_DONE_KEY)?.is_some() => Some(conn.query_row(
            "SELECT COALESCE(MAX(at_ms), 0) FROM event",
            [],
            |r| r.get::<_, i64>(0),
        )? as u64),
        None => None,
    };
    let tx = conn.transaction()?;
    let mut n = 0;
    let mut newest = mark.unwrap_or(0);
    for name in EVENT_FILES {
        let Ok(text) = std::fs::read_to_string(data_dir.join(name)) else {
            continue;
        };
        for mut e in text
            .lines()
            .filter_map(|l| serde_json::from_str::<EventRecord>(l).ok())
            .filter(|e| mark.is_none_or(|m| e.at_ms >= m))
        {
            newest = newest.max(e.at_ms);
            sanitize_detail(&mut e.detail);
            if insert_event_on(&tx, &e)? {
                n += 1;
            }
        }
    }
    set_events_mark(&tx, newest)?;
    tx.commit()?;
    Ok(n)
}

/// Runs both imports and logs `history.imported` when anything was stored.
pub fn run(conn: &mut Connection, data_dir: &Path) -> rusqlite::Result<Imported> {
    let done = Imported {
        speedtests: import_speedtests(conn, data_dir)?,
        events: import_events(conn, data_dir)?,
    };
    if done != Imported::default() {
        let rec = EventRecord {
            at_ms: crate::notify::now_ms(),
            kind: kinds::HISTORY_IMPORTED.into(),
            detail: serde_json::json!({ "speedtests": done.speedtests, "events": done.events }),
        };
        let _ = EventLog::new(data_dir.to_path_buf()).append(&rec);
        // The event sink sends the same record; the second copy is ignored.
        insert_event_on(conn, &rec)?;
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{schema, HistoryStore};
    use std::time::Duration;

    fn result(at: u64, source: Option<SpeedTestSource>) -> SpeedTestResult {
        serde_json::from_value(serde_json::json!({
            "atMs": at,
            "provider": "cloudflare",
            "requested": "cloudflare",
            "fallback": null,
            "fallbackDetail": null,
            "size": "light",
            "serving": {"lteBand": 3, "ltePci": 103},
            "idlePingMs": 40.0,
            "downloadMbps": 100.0,
            "uploadMbps": 10.0,
            "pingDownMs": null,
            "pingUpMs": null,
            "bytesUsed": 1000,
            "server": null,
            "source": source,
        }))
        .unwrap()
    }

    fn mem() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        schema::migrate(&mut c).unwrap();
        c
    }

    fn write_lines(path: &Path, lines: &[String]) {
        std::fs::write(path, lines.join("\n") + "\n").unwrap();
    }

    fn sources(c: &Connection) -> Vec<(i64, String)> {
        let mut st = c
            .prepare("SELECT at_ms, source FROM speedtest ORDER BY at_ms")
            .unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn older_lines_import_without_an_ip_range() {
        let dir = tempfile::tempdir().unwrap();
        let line = serde_json::to_string(&result(500, None)).unwrap();
        assert!(!line.contains("ipRange\":\""));
        write_lines(&speedtest::history_path(dir.path()), &[line]);
        let mut c = mem();
        assert_eq!(run(&mut c, dir.path()).unwrap().speedtests, 1);
        let range: Option<String> = c
            .query_row("SELECT ip_range FROM speedtest", [], |r| r.get(0))
            .unwrap();
        assert_eq!(range, None);
    }

    #[test]
    fn the_source_is_inferred_from_the_scan_periods() {
        let p = [(100, 200), (500, u64::MAX)];
        assert_eq!(infer_source(150, &p), SpeedTestSource::Scan);
        assert_eq!(infer_source(300, &p), SpeedTestSource::Manual);
        assert_eq!(infer_source(9_999, &p), SpeedTestSource::Scan);
        assert_eq!(infer_source(1, &[]), SpeedTestSource::Manual);
    }

    #[test]
    fn speed_tests_are_imported_once_with_their_source_and_the_file_renamed() {
        let dir = tempfile::tempdir().unwrap();
        write_lines(
            &dir.path().join("events.jsonl"),
            &[
                r#"{"at_ms":1000,"kind":"scan.active","detail":{"active":true}}"#.into(),
                r#"{"at_ms":2000,"kind":"scan.active","detail":{"active":false}}"#.into(),
            ],
        );
        let line = |r: &SpeedTestResult| serde_json::to_string(r).unwrap();
        write_lines(
            &speedtest::history_path(dir.path()),
            &[
                line(&result(500, None)),
                "not json".into(),
                line(&result(1500, None)),
                line(&result(1600, Some(SpeedTestSource::Manual))),
                line(&result(500, None)),
            ],
        );
        let mut c = mem();
        let got = run(&mut c, dir.path()).unwrap();
        assert_eq!(got.speedtests, 3);
        assert_eq!(
            sources(&c),
            vec![
                (500, "manual".into()),
                (1500, "scan".into()),
                (1600, "manual".into())
            ]
        );
        assert!(!speedtest::history_path(dir.path()).exists());
        assert!(dir.path().join("speedtests.jsonl.imported").exists());
        // Nothing new the second time.
        assert_eq!(run(&mut c, dir.path()).unwrap(), Imported::default());
        // The stored JSON carries the inferred source.
        let json: String = c
            .query_row(
                "SELECT record_json FROM speedtest WHERE at_ms = 1500",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let r: SpeedTestResult = serde_json::from_str(&json).unwrap();
        assert_eq!(r.source, Some(SpeedTestSource::Scan));
    }

    #[test]
    fn events_are_imported_sanitized_deduplicated_and_from_the_mark_on() {
        let dir = tempfile::tempdir().unwrap();
        let cell =
            r#"{"at_ms":10,"kind":"cell.changed","detail":{"to":{"pci":101,"cell_id":12345}}}"#;
        write_lines(&dir.path().join("events.jsonl.1"), &[cell.into()]);
        write_lines(
            &dir.path().join("events.jsonl"),
            &[
                cell.into(),
                "{broken".into(),
                r#"{"at_ms":20,"kind":"session","detail":{"state":"loggedIn"}}"#.into(),
            ],
        );
        let mut c = mem();
        let got = run(&mut c, dir.path()).unwrap();
        assert_eq!(got.events, 2);
        let detail: String = c
            .query_row(
                "SELECT detail_json FROM event WHERE kind = 'cell.changed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(detail, r#"{"to":{"pci":101}}"#);
        // history.imported was logged and stored.
        let logged: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM event WHERE kind = 'history.imported'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(logged, 1);
        // The next start brings in only what's new since the last import (an event the live
        // copy missed), not lines older than that (a cleared or pruned one).
        write_lines(
            &dir.path().join("events.jsonl"),
            &[
                r#"{"at_ms":15,"kind":"session","detail":{"old":true}}"#.into(),
                r#"{"at_ms":30,"kind":"session","detail":{}}"#.into(),
            ],
        );
        assert_eq!(run(&mut c, dir.path()).unwrap().events, 1);
        assert_eq!(run(&mut c, dir.path()).unwrap().events, 0);
        // After Clear history, nothing older than the clear comes back.
        set_events_mark(&c, 100).unwrap();
        assert_eq!(run(&mut c, dir.path()).unwrap().events, 0);
    }

    #[test]
    fn a_fallback_write_is_imported_at_the_next_start() {
        let dir = tempfile::tempdir().unwrap();
        speedtest::append_history(dir.path(), &result(42, Some(SpeedTestSource::Auto))).unwrap();
        let s = HistoryStore::open(dir.path());
        assert!(s.flush(Duration::from_secs(5)));
        let got = crate::history::recent_speedtests(&s, dir.path()).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].source, Some(SpeedTestSource::Auto));
        assert!(!speedtest::history_path(dir.path()).exists());
        s.shutdown();
    }
}
