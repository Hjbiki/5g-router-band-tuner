//! Crash-safe record of mobile data turned off by Refresh IP.
//!
//! `<app_data>/pending-data-switch.json` is written (temp file, fsync, rename) before every "off"
//! POST, and a failed write sends nothing. It is moved to `journal-archive/` once a read of the
//! switch says 1. At launch an open record for this router means data may still be off: the
//! actor turns it back on after the first login. A record for another router, or one that can't
//! be read, is shown and never acted on.
//!
//! The record holds no address, credential or device identifier.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::journal::{retry_transient, sync_dir, write_atomic};

pub const DATA_SWITCH_VERSION: u32 = 1;
const FILE: &str = "pending-data-switch.json";
const ARCHIVE_DIR: &str = "journal-archive";
const ARCHIVE_PREFIX: &str = "data-switch-";
const ARCHIVE_KEEP: usize = 20;

/// Where the switch was when the record was last written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataSwitchPhase {
    /// Written before the "off" POST.
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataSwitchRecord {
    pub version: u32,
    pub id: String,
    pub host: String,
    pub driver: String,
    /// Wall clock. Audit only.
    pub created_at_ms: u64,
    pub phase: DataSwitchPhase,
}

impl DataSwitchRecord {
    /// Written by this version of the app, for this router and driver.
    pub fn is_ours(&self, host: &str, driver: &str) -> bool {
        self.version == DATA_SWITCH_VERSION && self.host == host && self.driver == driver
    }
}

pub struct DataSwitchStore {
    dir: PathBuf,
}

impl DataSwitchStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(FILE)
    }

    /// `Ok(None)` when nothing is on file. A corrupt file is an `InvalidData` error, never
    /// silently treated as "data is on".
    pub fn load(&self) -> io::Result<Option<DataSwitchRecord>> {
        match fs::read(self.path()) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Written and fsynced before the "off" POST.
    pub fn write(&self, r: &DataSwitchRecord) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let bytes = serde_json::to_vec_pretty(r).map_err(io::Error::other)?;
        write_atomic(&self.dir, FILE, &bytes)
    }

    /// Moves the record to `journal-archive/data-switch-<created_at_ms>-<id>.json`, removes the
    /// pending file and keeps the newest 20 of these archives.
    pub fn archive(&self, r: &DataSwitchRecord) -> io::Result<()> {
        let archive = self.dir.join(ARCHIVE_DIR);
        fs::create_dir_all(&archive)?;
        let name = format!(
            "{ARCHIVE_PREFIX}{}-{}.json",
            r.created_at_ms,
            safe_id(&r.id)
        );
        let bytes = serde_json::to_vec_pretty(r).map_err(io::Error::other)?;
        write_atomic(&archive, &name, &bytes)?;
        self.clear()?;
        prune(&archive)
    }

    /// Removes the pending file; a missing file is fine.
    pub fn clear(&self) -> io::Result<()> {
        match retry_transient(|| fs::remove_file(self.path())) {
            Ok(()) => {
                sync_dir(&self.dir);
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// The id ends up in a file name: keep it to characters that can't leave the archive dir.
fn safe_id(id: &str) -> String {
    let s: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(64)
        .collect();
    if s.is_empty() {
        "unknown".to_string()
    } else {
        s
    }
}

fn prune(archive: &Path) -> io::Result<()> {
    let mut entries: Vec<(u64, String)> = fs::read_dir(archive)?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|name| {
            let rest = name.strip_prefix(ARCHIVE_PREFIX)?;
            let ms = rest.split('-').next()?.parse().ok()?;
            Some((ms, name))
        })
        .collect();
    entries.sort_unstable_by(|a, b| b.cmp(a));
    for (_, name) in entries.into_iter().skip(ARCHIVE_KEEP) {
        retry_transient(|| fs::remove_file(archive.join(&name)))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, at: u64) -> DataSwitchRecord {
        DataSwitchRecord {
            version: DATA_SWITCH_VERSION,
            id: id.to_string(),
            host: "192.168.8.1".to_string(),
            driver: "mock".to_string(),
            created_at_ms: at,
            phase: DataSwitchPhase::Off,
        }
    }

    #[test]
    fn write_load_archive() {
        let dir = tempfile::tempdir().unwrap();
        let store = DataSwitchStore::new(dir.path().to_path_buf());
        assert_eq!(store.load().unwrap(), None);
        let r = rec("a-1", 5);
        store.write(&r).unwrap();
        assert_eq!(store.load().unwrap(), Some(r.clone()));
        assert!(r.is_ours("192.168.8.1", "mock"));
        assert!(!r.is_ours("192.168.1.1", "mock"));
        store.archive(&r).unwrap();
        assert_eq!(store.load().unwrap(), None);
        assert!(dir
            .path()
            .join("journal-archive")
            .join("data-switch-5-a-1.json")
            .exists());
    }

    #[test]
    fn a_corrupt_record_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(FILE), "{").unwrap();
        let err = DataSwitchStore::new(dir.path().to_path_buf())
            .load()
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn the_archive_keeps_the_newest_20() {
        let dir = tempfile::tempdir().unwrap();
        let store = DataSwitchStore::new(dir.path().to_path_buf());
        for i in 0..25u64 {
            store.archive(&rec(&format!("id{i}"), 100 + i)).unwrap();
        }
        let names: Vec<String> = fs::read_dir(dir.path().join("journal-archive"))
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .collect();
        assert_eq!(names.len(), 20);
        assert!(!names.iter().any(|n| n.contains("-id0.json")));
    }
}
