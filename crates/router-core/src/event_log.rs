//! JSON Lines event log (§7.4): `<app_data>/events.jsonl`, rotated at 1 MiB to `.1` → `.2` →
//! `.3`; anything older is dropped. Callers must never put credentials, tokens or device
//! identifiers in `detail`.
//!
//! A process may install one event sink ([`set_event_sink`]). [`EventLog::append`] calls it
//! with every record after the file write, whether or not the write worked, so the app can
//! mirror events elsewhere (the history database). A sink must not block; it should run
//! [`sanitize_detail`] on its copy before storing it.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::journal::retry_transient;

const LOG_FILE: &str = "events.jsonl";
const MAX_BYTES: u64 = 1024 * 1024;
const KEEP_ROTATED: u32 = 3;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventRecord {
    pub at_ms: u64,
    pub kind: String,
    pub detail: serde_json::Value,
}

/// Receives every appended record. See the module docs.
pub type EventSink = Box<dyn Fn(&EventRecord) + Send + Sync>;

static SINK: OnceLock<EventSink> = OnceLock::new();

/// Installs the process-wide event sink. Returns `false`, and drops `f`, when one is already
/// set.
pub fn set_event_sink(f: EventSink) -> bool {
    SINK.set(f).is_ok()
}

fn notify_sink(sink: Option<&EventSink>, e: &EventRecord) {
    if let Some(f) = sink {
        f(e);
    }
}

/// Keys that name a device, a SIM or a specific cell site. They are removed at any depth,
/// compared case-insensitively. `pci` and `band` are kept.
const IDENTIFYING_KEYS: &[&str] = &[
    "cellid",
    "cell_id",
    "enbid",
    "enb_id",
    "sector",
    "tac",
    "imei",
    "imsi",
    "iccid",
    "serial",
    "mac",
    "ip",
    "wan_ip",
    "ipv4",
    "wanipaddress",
];

/// Removes identifying keys (cell, eNB and sector IDs, TAC, IMEI, IMSI, ICCID, serial, MAC,
/// IP, WAN address) from `v` at any depth. Matching is exact, so `ip_range` stays.
pub fn sanitize_detail(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            map.retain(|k, _| {
                !IDENTIFYING_KEYS
                    .iter()
                    .any(|bad| k.eq_ignore_ascii_case(bad))
            });
            for child in map.values_mut() {
                sanitize_detail(child);
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(sanitize_detail),
        _ => {}
    }
}

pub struct EventLog {
    dir: PathBuf,
}

impl EventLog {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Appends `e` to the log, then hands it to the event sink whatever the write's result.
    pub fn append(&self, e: &EventRecord) -> io::Result<()> {
        let result = self.write(e);
        notify_sink(SINK.get(), e);
        result
    }

    fn write(&self, e: &EventRecord) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let mut line = serde_json::to_vec(e).map_err(io::Error::other)?;
        line.push(b'\n');
        let path = self.dir.join(LOG_FILE);
        let len = match fs::metadata(&path) {
            Ok(m) => m.len(),
            Err(err) if err.kind() == io::ErrorKind::NotFound => 0,
            Err(err) => return Err(err),
        };
        if len > 0 && len + line.len() as u64 > MAX_BYTES {
            self.rotate()?;
        }
        let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
        f.write_all(&line)
    }

    fn rotate(&self) -> io::Result<()> {
        let rotated = |n: u32| self.dir.join(format!("{LOG_FILE}.{n}"));
        remove_if_exists(&rotated(KEEP_ROTATED))?;
        for n in (1..KEEP_ROTATED).rev() {
            rename_if_exists(&rotated(n), &rotated(n + 1))?;
        }
        rename_if_exists(&self.dir.join(LOG_FILE), &rotated(1))
    }
}

fn remove_if_exists(p: &std::path::Path) -> io::Result<()> {
    match retry_transient(|| fs::remove_file(p)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

fn rename_if_exists(from: &std::path::Path, to: &std::path::Path) -> io::Result<()> {
    match retry_transient(|| fs::rename(from, to)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn record(kind: &str) -> EventRecord {
        EventRecord {
            at_ms: 1,
            kind: kind.into(),
            detail: serde_json::json!({}),
        }
    }

    #[test]
    fn sanitize_removes_identifiers_at_any_depth() {
        let mut v = serde_json::json!({
            "from": {"rat": "lte", "pci": 101, "cell_id": 12345, "band": "B3"},
            "to": {"rat": "lte", "pci": 102, "cellId": 12346, "band": "B3"},
            "list": [{"enb_id": 48, "TAC": 7, "pci": 103}],
            "Imei": "000000000000000",
            "ip": "10.0.0.1",
            "kept": {"mac": "00:00:00:00:00:00", "band": 3},
        });
        sanitize_detail(&mut v);
        assert_eq!(
            v,
            serde_json::json!({
                "from": {"rat": "lte", "pci": 101, "band": "B3"},
                "to": {"rat": "lte", "pci": 102, "band": "B3"},
                "list": [{"pci": 103}],
                "kept": {"band": 3},
            })
        );
    }

    #[test]
    fn sanitize_removes_wan_addresses_but_keeps_the_range() {
        let mut v = serde_json::json!({
            "wan_ip": "10.45.1.2",
            "IPv4": "10.45.1.2",
            "WanIPAddress": "10.45.1.2",
            "ip_range": "10.45",
        });
        sanitize_detail(&mut v);
        assert_eq!(v, serde_json::json!({ "ip_range": "10.45" }));
    }

    #[test]
    fn sanitize_leaves_scalars_alone() {
        let mut v = serde_json::json!("tac");
        sanitize_detail(&mut v);
        assert_eq!(v, serde_json::json!("tac"));
    }

    #[test]
    fn sink_receives_the_record() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let sink: EventSink = Box::new(move |e| s2.lock().unwrap().push(e.kind.clone()));
        notify_sink(Some(&sink), &record("a.b"));
        notify_sink(None, &record("c.d"));
        assert_eq!(*seen.lock().unwrap(), vec!["a.b".to_string()]);
    }

    #[test]
    fn append_still_reports_a_failed_write() {
        // A file where the log directory should be makes create_dir_all fail.
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"x").unwrap();
        assert!(EventLog::new(blocker).append(&record("a.b")).is_err());
    }
}
