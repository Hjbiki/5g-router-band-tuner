//! The history database's pragmas and migrations. `user_version` counts the migrations applied;
//! a file from a newer app (a higher version) is opened for reading only.

use rusqlite::Connection;

/// The schema this build writes.
pub const VERSION: u32 = 2;

/// One entry per schema version, applied in order inside a transaction.
const MIGRATIONS: &[&str] = &[
    // v1
    "CREATE TABLE meta (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    CREATE TABLE routers (
        id     INTEGER PRIMARY KEY,
        host   TEXT NOT NULL,
        driver TEXT NOT NULL,
        UNIQUE (host, driver)
    );
    -- One row per UTC minute and router. Signal values are tenths of a dB, from in-service
    -- samples only. Never holds a cell ID, eNB ID, sector or TAC.
    CREATE TABLE signal_minute (
        minute_ms       INTEGER NOT NULL,
        router_id       INTEGER NOT NULL REFERENCES routers (id),
        alive_ms        INTEGER NOT NULL,
        observed_ms     INTEGER NOT NULL,
        service_ms      INTEGER NOT NULL,
        nr_ms           INTEGER NOT NULL,
        lte_ca_ms       INTEGER NOT NULL,
        cell_changes    INTEGER NOT NULL,
        flags           INTEGER NOT NULL,
        lte_rsrp_min    INTEGER, lte_rsrp_avg INTEGER, lte_rsrp_max INTEGER,
        lte_rsrq_min    INTEGER, lte_rsrq_avg INTEGER, lte_rsrq_max INTEGER,
        lte_sinr_min    INTEGER, lte_sinr_avg INTEGER, lte_sinr_max INTEGER,
        nr_rsrp_min     INTEGER, nr_rsrp_avg  INTEGER, nr_rsrp_max  INTEGER,
        nr_rsrq_min     INTEGER, nr_rsrq_avg  INTEGER, nr_rsrq_max  INTEGER,
        nr_sinr_min     INTEGER, nr_sinr_avg  INTEGER, nr_sinr_max  INTEGER,
        net_mode        TEXT,
        nr_option       TEXT,
        allowed_bands   TEXT,
        network_type    INTEGER,
        bars            INTEGER,
        endc            INTEGER,
        endc_restricted INTEGER,
        rat             TEXT,
        lte_band        INTEGER,
        nr_band         INTEGER,
        lte_pci         INTEGER,
        lte_earfcn      INTEGER,
        nr_pci          INTEGER,
        nr_arfcn        INTEGER,
        operator        TEXT,
        PRIMARY KEY (minute_ms, router_id)
    ) WITHOUT ROWID;
    -- Every speed test; record_json is the full SpeedTestResult.
    CREATE TABLE speedtest (
        id            INTEGER PRIMARY KEY,
        at_ms         INTEGER NOT NULL,
        router_id     INTEGER REFERENCES routers (id),
        provider      TEXT NOT NULL,
        source        TEXT NOT NULL,
        size          TEXT NOT NULL,
        download_mbps REAL,
        upload_mbps   REAL,
        idle_ping_ms  REAL,
        bytes_used    INTEGER NOT NULL,
        lte_band      INTEGER,
        nr_band       INTEGER,
        allowed_bands TEXT,
        valid         INTEGER NOT NULL,
        record_json   TEXT NOT NULL,
        UNIQUE (at_ms, provider)
    );
    CREATE INDEX speedtest_at ON speedtest (at_ms);
    -- A sanitized copy of every events.jsonl record.
    CREATE TABLE event (
        id          INTEGER PRIMARY KEY,
        at_ms       INTEGER NOT NULL,
        kind        TEXT NOT NULL,
        detail_json TEXT NOT NULL,
        UNIQUE (at_ms, kind, detail_json)
    );
    CREATE INDEX event_at ON event (at_ms);
    CREATE INDEX event_kind_at ON event (kind, at_ms);",
    // v2: the internet address's range ("a.b", never more) with each speed test and minute.
    // Rows from before stay NULL.
    "ALTER TABLE speedtest ADD COLUMN ip_range TEXT;
    ALTER TABLE signal_minute ADD COLUMN ip_range TEXT;
    CREATE INDEX speedtest_range ON speedtest (ip_range, at_ms);",
];

/// Set on every connection. `auto_vacuum` only takes effect before the first table exists, so it
/// comes before anything that creates one.
pub fn apply_pragmas(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(std::time::Duration::from_millis(2000))?;
    conn.execute_batch(
        "PRAGMA page_size = 4096;
         PRAGMA auto_vacuum = INCREMENTAL;",
    )?;
    // journal_mode returns a row, so it's read rather than executed.
    let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    conn.execute_batch(
        "PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;",
    )
}

pub fn user_version(conn: &Connection) -> rusqlite::Result<u32> {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
}

/// What the file holds after `migrate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opened {
    /// At this build's version and writable.
    Current,
    /// Made by a newer app: read it, never write it.
    Newer(u32),
}

/// Brings the file up to `VERSION`. Reopening a current file changes nothing.
pub fn migrate(conn: &mut Connection) -> rusqlite::Result<Opened> {
    let have = user_version(conn)?;
    if have > VERSION {
        return Ok(Opened::Newer(have));
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(have as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as u32)?;
        tx.commit()?;
    }
    Ok(Opened::Current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_is_one_migration_per_version() {
        assert_eq!(MIGRATIONS.len(), VERSION as usize);
    }

    #[test]
    fn a_v1_file_moves_to_v2_and_keeps_its_rows() {
        let mut c = Connection::open_in_memory().unwrap();
        c.execute_batch(MIGRATIONS[0]).unwrap();
        c.pragma_update(None, "user_version", 1).unwrap();
        c.execute_batch(
            "INSERT INTO routers (host, driver) VALUES ('192.168.8.1', 'mock');
             INSERT INTO speedtest (at_ms, provider, source, size, bytes_used, valid, record_json)
                 VALUES (1, 'cloudflare', 'manual', 'light', 0, 1, '{}');
             INSERT INTO signal_minute (minute_ms, router_id, alive_ms, observed_ms, service_ms,
                 nr_ms, lte_ca_ms, cell_changes, flags) VALUES (0, 1, 1, 1, 1, 0, 0, 0, 0);",
        )
        .unwrap();
        assert_eq!(migrate(&mut c).unwrap(), Opened::Current);
        assert_eq!(user_version(&c).unwrap(), 2);
        let range = |table: &str| -> Option<String> {
            c.query_row(&format!("SELECT ip_range FROM {table}"), [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(range("speedtest"), None, "old rows stay unknown");
        assert_eq!(range("signal_minute"), None);
        let index: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'speedtest_range'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(index, 1);
        // Migrating again changes nothing.
        assert_eq!(migrate(&mut c).unwrap(), Opened::Current);
        assert_eq!(user_version(&c).unwrap(), 2);
    }
}
