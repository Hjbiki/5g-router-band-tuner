//! The history database: one writer thread that owns the write connection, and one read
//! connection behind a mutex for queries. Everything is sent to the writer through a bounded
//! channel, so a caller never waits on the disk (events are dropped and counted when it's full).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Duration;

use router_core::event_text::kinds;
use router_core::{sanitize_detail, EventLog, EventRecord};
use rusqlite::{params, Connection, ErrorCode, OpenFlags, Transaction};

use super::sampler::{upsert_minute, MinuteRow};
use super::schema::{self, Opened};
use crate::speedtest::{clean_ip_range, SpeedTestResult, SpeedTestSource};

pub const FILE: &str = "history.db";
/// The writer's queue. A full queue drops events (counted) rather than block the caller.
const QUEUE: usize = 1024;
/// Messages applied in one transaction.
const BATCH: usize = 64;

pub const ERR_NEWER: &str = "history was made by a newer version of the app";
pub const ERR_UNAVAILABLE: &str = "history is not available";

/// Work for the writer thread.
pub enum WriteMsg {
    /// A copy of an `events.jsonl` record, already sanitized.
    Event(EventRecord),
    /// A finished speed test, and the router (host, driver) it ran on when known.
    Speed {
        result: Box<SpeedTestResult>,
        router: Option<(String, String)>,
    },
    /// A finished signal minute of the router (host, driver); merged with a row already there.
    Minute {
        row: Box<MinuteRow>,
        router: (String, String),
    },
    /// Answered once everything queued before it is committed.
    Flush(SyncSender<()>),
    /// Runs on the write connection outside any transaction, after everything queued before it
    /// is committed: pruning, clearing, VACUUM.
    Exec(Job),
    /// Commit what's queued and stop.
    Shutdown,
}

/// A job for [`WriteMsg::Exec`].
pub type Job = Box<dyn FnOnce(&mut Connection) + Send>;

pub enum HistoryStore {
    Ready(Ready),
    /// The file couldn't be opened; the reason is shown on the History page.
    Disabled(String),
}

pub struct Ready {
    /// `None` when the file was made by a newer app and is read-only.
    tx: Option<SyncSender<WriteMsg>>,
    read: Mutex<Connection>,
    dropped: AtomicU64,
    writer: Mutex<Option<JoinHandle<()>>>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn db_path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

/// The file isn't a database, or SQLite found it damaged.
fn is_corrupt(e: &rusqlite::Error) -> bool {
    matches!(
        e.sqlite_error_code(),
        Some(ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt)
    )
}

fn open_write(path: &Path) -> rusqlite::Result<(Connection, Opened)> {
    let mut conn = Connection::open(path)?;
    // A newer app's file is left exactly as it is.
    let have = schema::user_version(&conn)?;
    if have > schema::VERSION {
        return Ok((conn, Opened::Newer(have)));
    }
    schema::apply_pragmas(&conn)?;
    let opened = schema::migrate(&mut conn)?;
    Ok((conn, opened))
}

fn open_read(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_millis(2000))?;
    Ok(conn)
}

/// Renames the damaged file (and its WAL and shared-memory files) out of the way. Returns the
/// new name of the main file.
fn move_aside(path: &Path) -> std::io::Result<String> {
    let stamp = now_ms();
    let name = format!("{FILE}.corrupt-{stamp}");
    std::fs::rename(path, path.with_file_name(&name))?;
    for ext in ["-wal", "-shm"] {
        let side = path.with_file_name(format!("{FILE}{ext}"));
        if side.exists() {
            let _ = std::fs::rename(&side, path.with_file_name(format!("{name}{ext}")));
        }
    }
    Ok(name)
}

/// Upserts the router and returns its row id.
pub fn router_id(conn: &Connection, host: &str, driver: &str) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO routers (host, driver) VALUES (?1, ?2) ON CONFLICT (host, driver) DO NOTHING",
        params![host, driver],
    )?;
    conn.query_row(
        "SELECT id FROM routers WHERE host = ?1 AND driver = ?2",
        params![host, driver],
        |r| r.get(0),
    )
}

/// A serde enum's plain name (`"cloudflare"`), or `""`.
fn enum_name<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Stores one result; a second copy (same time and provider) is ignored. `true` when stored.
/// A result without a source is stored as `fallback_source`. An IP range that is anything but
/// two numbers is dropped, so no address can reach the file this way.
pub(super) fn insert_speedtest(
    conn: &Connection,
    r: &SpeedTestResult,
    router_id: Option<i64>,
    fallback_source: SpeedTestSource,
) -> rusqlite::Result<bool> {
    let mut r = r.clone();
    let source = *r.source.get_or_insert(fallback_source);
    r.ip_range = clean_ip_range(r.ip_range.as_deref());
    let json = serde_json::to_string(&r)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let n = conn.execute(
        "INSERT OR IGNORE INTO speedtest (at_ms, router_id, provider, source, size,
             download_mbps, upload_mbps, idle_ping_ms, bytes_used, lte_band, nr_band,
             allowed_bands, valid, record_json, ip_range)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            r.at_ms as i64,
            router_id,
            enum_name(&r.provider),
            enum_name(&source),
            enum_name(&r.size),
            r.download_mbps,
            r.upload_mbps,
            r.idle_ping_ms,
            r.bytes_used as i64,
            r.serving.lte_band,
            r.serving.nr_band,
            r.allowed_bands.map(|b| b.to_huawei_hex()),
            r.invalid_reason.is_none(),
            json,
            r.ip_range,
        ],
    )?;
    Ok(n > 0)
}

pub(super) fn insert_event_on(conn: &Connection, e: &EventRecord) -> rusqlite::Result<bool> {
    let detail = serde_json::to_string(&e.detail).unwrap_or_else(|_| "null".into());
    let n = conn.execute(
        "INSERT OR IGNORE INTO event (at_ms, kind, detail_json) VALUES (?1, ?2, ?3)",
        params![e.at_ms as i64, e.kind, detail],
    )?;
    Ok(n > 0)
}

fn insert_event(tx: &Transaction<'_>, e: &EventRecord) -> rusqlite::Result<()> {
    insert_event_on(tx, e).map(|_| ())
}

/// Applies the writes of `batch` in one transaction. Flush replies and Shutdown are left to
/// the caller.
fn apply_batch(conn: &mut Connection, batch: &[WriteMsg]) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    for msg in batch {
        match msg {
            WriteMsg::Event(e) => insert_event(&tx, e)?,
            WriteMsg::Speed { result, router } => {
                let id = match router {
                    Some((host, driver)) => Some(router_id(&tx, host, driver)?),
                    None => None,
                };
                insert_speedtest(&tx, result, id, SpeedTestSource::Manual)?;
            }
            WriteMsg::Minute { row, router } => {
                let id = router_id(&tx, &router.0, &router.1)?;
                upsert_minute(&tx, id, row)?;
            }
            // Handled by `write_batch`; `writer_loop` never puts a job in a batch.
            WriteMsg::Flush(_) | WriteMsg::Shutdown | WriteMsg::Exec(_) => {}
        }
    }
    tx.commit()
}

/// Commits a batch. When its transaction fails, each write is tried again on its own, so one
/// bad row (or a busy moment) can't take the others with it. Returns the Flush replies to send
/// and whether Shutdown was asked for.
fn write_batch(conn: &mut Connection, batch: &[WriteMsg]) -> (Vec<SyncSender<()>>, bool) {
    let mut replies = Vec::new();
    let mut stop = false;
    for msg in batch {
        match msg {
            WriteMsg::Flush(reply) => replies.push(reply.clone()),
            WriteMsg::Shutdown => stop = true,
            _ => {}
        }
    }
    if let Err(e) = apply_batch(conn, batch) {
        eprintln!(
            "history: a batch of {} failed, writing one at a time: {e}",
            batch.len()
        );
        for msg in batch {
            if matches!(msg, WriteMsg::Flush(_) | WriteMsg::Shutdown) {
                continue;
            }
            if let Err(e) = apply_batch(conn, std::slice::from_ref(msg)) {
                eprintln!("history: a write failed: {e}");
            }
        }
    }
    (replies, stop)
}

fn writer_loop(mut conn: Connection, data_dir: PathBuf, rx: Receiver<WriteMsg>) {
    // Anything queued meanwhile waits, so a Flush sent after `open` also waits for the import.
    if let Err(e) = super::import::run(&mut conn, &data_dir) {
        eprintln!("history: the import failed: {e}");
    }
    // A job met while batching waits here until the batch before it is committed.
    let mut held: Option<Job> = None;
    loop {
        if let Some(job) = held.take() {
            job(&mut conn);
            continue;
        }
        let first = match rx.recv() {
            Ok(WriteMsg::Exec(job)) => {
                job(&mut conn);
                continue;
            }
            Ok(m) => m,
            Err(_) => break,
        };
        let mut batch = vec![first];
        while batch.len() < BATCH {
            match rx.try_recv() {
                Ok(WriteMsg::Exec(job)) => {
                    held = Some(job);
                    break;
                }
                Ok(m) => batch.push(m),
                Err(_) => break,
            }
        }
        let (replies, stop) = write_batch(&mut conn, &batch);
        for r in replies {
            let _ = r.send(());
        }
        if stop {
            break;
        }
    }
}

impl HistoryStore {
    /// Opens or creates `<data_dir>/history.db`. A damaged file is moved aside and a fresh one is
    /// created (logged as `history.reset`); if that fails too, history is disabled.
    pub fn open(data_dir: &Path) -> Self {
        let path = db_path(data_dir);
        match Self::try_open(data_dir) {
            Ok(s) => s,
            Err(e) if is_corrupt(&e) && path.exists() => {
                let moved = match move_aside(&path) {
                    Ok(name) => name,
                    Err(io) => return Self::Disabled(format!("{e}; {io}")),
                };
                let rec = EventRecord {
                    at_ms: now_ms(),
                    kind: kinds::HISTORY_RESET.into(),
                    detail: serde_json::json!({ "movedTo": moved, "error": e.to_string() }),
                };
                let _ = EventLog::new(data_dir.to_path_buf()).append(&rec);
                match Self::try_open(data_dir) {
                    Ok(s) => {
                        s.mirror_event(&rec);
                        s
                    }
                    Err(e) => Self::Disabled(e.to_string()),
                }
            }
            Err(e) => Self::Disabled(e.to_string()),
        }
    }

    fn try_open(data_dir: &Path) -> rusqlite::Result<Self> {
        let path = db_path(data_dir);
        let (conn, opened) = open_write(&path)?;
        let read = open_read(&path)?;
        let data_dir = data_dir.to_path_buf();
        let (tx, writer) = match opened {
            Opened::Current => {
                let (tx, rx) = mpsc::sync_channel(QUEUE);
                let handle = std::thread::Builder::new()
                    .name("history-writer".into())
                    .spawn(move || writer_loop(conn, data_dir, rx))
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                (Some(tx), Some(handle))
            }
            Opened::Newer(_) => (None, None),
        };
        Ok(Self::Ready(Ready {
            tx,
            read: Mutex::new(read),
            dropped: AtomicU64::new(0),
            writer: Mutex::new(writer),
        }))
    }

    /// Why history isn't available, or `None` when it is.
    pub fn unavailable_reason(&self) -> Option<&str> {
        match self {
            Self::Ready(_) => None,
            Self::Disabled(why) => Some(why),
        }
    }

    pub fn is_disabled(&self) -> bool {
        matches!(self, Self::Disabled(_))
    }

    fn sender(&self) -> Result<&SyncSender<WriteMsg>, String> {
        match self {
            Self::Ready(r) => r.tx.as_ref().ok_or_else(|| ERR_NEWER.to_string()),
            Self::Disabled(_) => Err(ERR_UNAVAILABLE.into()),
        }
    }

    /// Queues a write, waiting while the queue is full. For rare, important writes only.
    pub fn send(&self, msg: WriteMsg) -> Result<(), String> {
        self.sender()?
            .send(msg)
            .map_err(|_| "the history writer stopped".to_string())
    }

    /// The event sink: a sanitized copy of `e` is queued, or dropped and counted when the queue
    /// is full. Never blocks.
    pub fn mirror_event(&self, e: &EventRecord) {
        let Self::Ready(r) = self else { return };
        let Some(tx) = &r.tx else { return };
        let mut e = e.clone();
        sanitize_detail(&mut e.detail);
        if let Err(TrySendError::Full(_)) = tx.try_send(WriteMsg::Event(e)) {
            r.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Events dropped since the last call.
    pub fn take_dropped(&self) -> u64 {
        match self {
            Self::Ready(r) => r.dropped.swap(0, Ordering::Relaxed),
            Self::Disabled(_) => 0,
        }
    }

    /// Waits up to `timeout` for everything queued so far to be committed.
    pub fn flush(&self, timeout: Duration) -> bool {
        let Ok(tx) = self.sender() else { return false };
        let (reply, done) = mpsc::sync_channel(1);
        if tx.send(WriteMsg::Flush(reply)).is_err() {
            return false;
        }
        done.recv_timeout(timeout).is_ok()
    }

    /// Runs `f` on the write connection, after everything queued before it, and waits for its
    /// answer. Blocking: call off the async runtime.
    pub fn exec<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> rusqlite::Result<T> + Send + 'static,
    ) -> Result<T, String> {
        let tx = self.sender()?;
        let (reply, done) = mpsc::sync_channel(1);
        let job: Job = Box::new(move |c| {
            let _ = reply.send(f(c).map_err(|e| e.to_string()));
        });
        tx.send(WriteMsg::Exec(job))
            .map_err(|_| "the history writer stopped".to_string())?;
        done.recv()
            .map_err(|_| "the history writer stopped".to_string())?
    }

    /// Like `exec`, but gives up waiting after `timeout`. The job may still run later.
    pub fn exec_within<T: Send + 'static>(
        &self,
        timeout: Duration,
        f: impl FnOnce(&mut Connection) -> rusqlite::Result<T> + Send + 'static,
    ) -> Result<T, String> {
        let tx = self.sender()?;
        let (reply, done) = mpsc::sync_channel(1);
        let job: Job = Box::new(move |c| {
            let _ = reply.send(f(c).map_err(|e| e.to_string()));
        });
        tx.send(WriteMsg::Exec(job))
            .map_err(|_| "the history writer stopped".to_string())?;
        done.recv_timeout(timeout)
            .map_err(|_| "the history write didn't finish in time".to_string())?
    }

    /// `true` when the file was made by a newer app: readable, never written.
    pub fn is_read_only(&self) -> bool {
        matches!(self, Self::Ready(r) if r.tx.is_none())
    }

    /// Commits what's queued, stops the writer and waits for it.
    pub fn shutdown(&self) {
        let Self::Ready(r) = self else { return };
        if let Some(tx) = &r.tx {
            let _ = tx.send(WriteMsg::Shutdown);
        }
        if let Some(h) = r.writer.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = h.join();
        }
    }

    /// Runs a query on the read connection.
    pub fn with_read<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, String> {
        match self {
            Self::Ready(r) => {
                let conn = r.read.lock().unwrap_or_else(|p| p.into_inner());
                f(&conn).map_err(|e| e.to_string())
            }
            Self::Disabled(_) => Err(ERR_UNAVAILABLE.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(at: u64, kind: &str, detail: serde_json::Value) -> EventRecord {
        EventRecord {
            at_ms: at,
            kind: kind.into(),
            detail,
        }
    }

    fn count(s: &HistoryStore, table: &str) -> i64 {
        s.with_read(|c| c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)))
            .unwrap()
    }

    #[test]
    fn a_fresh_file_reaches_the_current_version_with_its_pragmas() {
        let dir = tempfile::tempdir().unwrap();
        let s = HistoryStore::open(dir.path());
        assert!(s.unavailable_reason().is_none());
        s.shutdown();
        let c = Connection::open(db_path(dir.path())).unwrap();
        assert_eq!(schema::user_version(&c).unwrap(), schema::VERSION);
        let pragma = |p: &str| -> String {
            c.query_row(&format!("PRAGMA {p}"), [], |r| {
                r.get::<_, rusqlite::types::Value>(0)
            })
            .map(|v| match v {
                rusqlite::types::Value::Integer(i) => i.to_string(),
                rusqlite::types::Value::Text(t) => t,
                other => format!("{other:?}"),
            })
            .unwrap()
        };
        assert_eq!(pragma("journal_mode"), "wal");
        assert_eq!(pragma("auto_vacuum"), "2", "incremental");
        assert_eq!(pragma("page_size"), "4096");
    }

    #[test]
    fn reopening_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let s = HistoryStore::open(dir.path());
        s.mirror_event(&event(1, "app.started", serde_json::json!({})));
        assert!(s.flush(Duration::from_secs(5)));
        s.shutdown();
        drop(s);
        let s = HistoryStore::open(dir.path());
        assert!(s.unavailable_reason().is_none());
        assert_eq!(count(&s, "event"), 1);
        s.shutdown();
    }

    #[test]
    fn a_file_from_a_newer_app_is_read_only() {
        let dir = tempfile::tempdir().unwrap();
        {
            let c = Connection::open(db_path(dir.path())).unwrap();
            c.pragma_update(None, "user_version", 99).unwrap();
        }
        let s = HistoryStore::open(dir.path());
        assert!(s.unavailable_reason().is_none(), "still readable");
        assert_eq!(
            s.send(WriteMsg::Event(event(1, "a", serde_json::json!({})))),
            Err(ERR_NEWER.to_string())
        );
        assert!(!s.flush(Duration::from_millis(100)));
        // The event sink quietly drops it.
        s.mirror_event(&event(1, "a", serde_json::json!({})));
        assert_eq!(s.take_dropped(), 0);
        assert_eq!(s.with_read(schema::user_version).unwrap(), 99);
    }

    #[test]
    fn a_damaged_file_is_moved_aside_and_logged() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(db_path(dir.path()), vec![0x5Au8; 8192]).unwrap();
        let s = HistoryStore::open(dir.path());
        assert!(s.unavailable_reason().is_none());
        assert!(s.flush(Duration::from_secs(5)));
        let moved: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("history.db.corrupt-"))
            .collect();
        assert_eq!(moved.len(), 1, "{moved:?}");
        let log = std::fs::read_to_string(dir.path().join("events.jsonl")).unwrap();
        assert!(log.contains(kinds::HISTORY_RESET));
        // Stored once, although both the import and the direct copy saw it.
        let n: i64 = s
            .with_read(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM event WHERE kind = ?1",
                    [kinds::HISTORY_RESET],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(n, 1);
        s.shutdown();
    }

    #[test]
    fn events_are_written_once_and_sanitized() {
        let dir = tempfile::tempdir().unwrap();
        let s = HistoryStore::open(dir.path());
        let e = event(
            5,
            kinds::CELL_CHANGED,
            serde_json::json!({"to": {"pci": 101, "cell_id": 12345, "band": "B3"}}),
        );
        s.mirror_event(&e);
        s.mirror_event(&e);
        s.mirror_event(&event(
            5,
            kinds::CELL_CHANGED,
            serde_json::json!({"other": 1}),
        ));
        assert!(s.flush(Duration::from_secs(5)));
        assert_eq!(count(&s, "event"), 2);
        let details: Vec<String> = s
            .with_read(|c| {
                let mut st = c.prepare("SELECT detail_json FROM event ORDER BY id")?;
                let rows = st.query_map([], |r| r.get(0))?;
                rows.collect()
            })
            .unwrap();
        assert_eq!(details[0], r#"{"to":{"band":"B3","pci":101}}"#);
        s.shutdown();
    }

    #[test]
    fn one_failing_write_does_not_take_the_rest_of_its_batch() {
        let dir = tempfile::tempdir().unwrap();
        let (mut c, _) = open_write(&db_path(dir.path())).unwrap();
        // Any insert of this kind fails, like a row SQLite refuses.
        c.execute_batch(
            "CREATE TRIGGER refuse BEFORE INSERT ON event WHEN NEW.kind = 'bad'
             BEGIN SELECT RAISE(ABORT, 'refused'); END;",
        )
        .unwrap();
        let (reply, done) = mpsc::sync_channel(1);
        let batch = vec![
            WriteMsg::Event(event(1, "good", serde_json::json!({}))),
            WriteMsg::Event(event(2, "bad", serde_json::json!({}))),
            WriteMsg::Event(event(3, "good", serde_json::json!({}))),
            WriteMsg::Flush(reply),
        ];
        let (replies, stop) = write_batch(&mut c, &batch);
        assert!(!stop);
        assert_eq!(replies.len(), 1);
        replies[0].send(()).unwrap();
        assert!(done.try_recv().is_ok());
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM event", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }

    fn speed(at: u64, ip_range: Option<&str>) -> SpeedTestResult {
        serde_json::from_value(serde_json::json!({
            "atMs": at,
            "provider": "cloudflare",
            "requested": "cloudflare",
            "fallback": null,
            "fallbackDetail": null,
            "size": "light",
            "serving": {"lteBand": 3},
            "idlePingMs": 40.0,
            "downloadMbps": 100.0,
            "uploadMbps": 10.0,
            "pingDownMs": null,
            "pingUpMs": null,
            "bytesUsed": 1,
            "server": null,
            "ipRange": ip_range,
        }))
        .unwrap()
    }

    #[test]
    fn a_speed_test_keeps_its_range_and_never_an_address() {
        let mut c = Connection::open_in_memory().unwrap();
        schema::migrate(&mut c).unwrap();
        insert_speedtest(&c, &speed(1, Some("10.45")), None, SpeedTestSource::Manual).unwrap();
        insert_speedtest(
            &c,
            &speed(2, Some("10.45.37.142")),
            None,
            SpeedTestSource::Manual,
        )
        .unwrap();
        insert_speedtest(&c, &speed(3, None), None, SpeedTestSource::Manual).unwrap();
        let mut st = c
            .prepare("SELECT ip_range, record_json FROM speedtest ORDER BY at_ms")
            .unwrap();
        let rows: Vec<(Option<String>, String)> = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(rows[0].0.as_deref(), Some("10.45"));
        assert!(rows[0].1.contains(r#""ipRange":"10.45""#), "{}", rows[0].1);
        assert_eq!(rows[1].0, None, "a full address is dropped");
        assert!(!rows[1].1.contains("10.45"), "{}", rows[1].1);
        assert_eq!(rows[2].0, None);
        let back = super::super::query::speedtests(
            &c,
            &super::super::query::HistoryRange {
                from_ms: 0,
                to_ms: 10,
                utc_offset_min: 0,
            },
        )
        .unwrap();
        assert_eq!(back[0].ip_range.as_deref(), Some("10.45"));
    }

    /// What an older build sees: a file at a higher version than its own opens read-only, and
    /// what's in it can still be read.
    #[test]
    fn a_file_from_a_newer_schema_stays_readable() {
        let dir = tempfile::tempdir().unwrap();
        let s = HistoryStore::open(dir.path());
        s.send(WriteMsg::Speed {
            result: Box::new(speed(5, Some("10.62"))),
            router: None,
        })
        .unwrap();
        assert!(s.flush(Duration::from_secs(5)));
        s.shutdown();
        drop(s);
        {
            let c = Connection::open(db_path(dir.path())).unwrap();
            c.pragma_update(None, "user_version", schema::VERSION + 1)
                .unwrap();
        }
        let s = HistoryStore::open(dir.path());
        assert!(s.is_read_only());
        let range: Option<String> = s
            .with_read(|c| c.query_row("SELECT ip_range FROM speedtest", [], |r| r.get(0)))
            .unwrap();
        assert_eq!(range.as_deref(), Some("10.62"));
        assert_eq!(
            s.with_read(schema::user_version).unwrap(),
            schema::VERSION + 1,
            "left as it was"
        );
    }

    #[test]
    fn routers_are_upserted() {
        let c = Connection::open_in_memory().unwrap();
        let mut c = c;
        schema::migrate(&mut c).unwrap();
        let a = router_id(&c, "192.168.8.1", "huaweiH122").unwrap();
        let b = router_id(&c, "192.168.8.1", "mock").unwrap();
        assert_ne!(a, b);
        assert_eq!(router_id(&c, "192.168.8.1", "huaweiH122").unwrap(), a);
    }

    #[test]
    fn a_disabled_store_refuses_everything() {
        let s = HistoryStore::Disabled("broken".into());
        assert_eq!(s.unavailable_reason(), Some("broken"));
        assert!(s.with_read(|_| Ok(())).is_err());
        assert!(s.send(WriteMsg::Shutdown).is_err());
        assert!(!s.flush(Duration::from_millis(10)));
    }
}
