//! The `logged-out` marker.
//!
//! Written when the user logs out, so the launch auto-connect doesn't log straight back in at
//! the next start. Pressing Connect clears it. A band change, sequence or foreign journal that
//! needs watching still connects (see `actor`), and leaves the marker in place.
//!
//! The marker names the account it was written for. One that can't be read, or that names
//! another host or username, counts as not marked: the worst case is an ordinary auto-connect.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::journal::{retry_transient, write_atomic};

/// File name in the app's data directory.
pub const MARKER_FILE: &str = "logged-out";

#[derive(Debug, Serialize, Deserialize)]
struct MarkerRecord {
    host: String,
    username: String,
    at_ms: u64,
}

/// Records that `username` on `host` logged out at `at_ms`.
pub fn mark(dir: &Path, host: &str, username: &str, at_ms: u64) -> io::Result<()> {
    let record = MarkerRecord {
        host: host.to_string(),
        username: username.to_string(),
        at_ms,
    };
    let bytes = serde_json::to_vec_pretty(&record).map_err(io::Error::other)?;
    write_atomic(dir, MARKER_FILE, &bytes)
}

/// Whether the marker names this account.
pub fn is_marked(dir: &Path, host: &str, username: &str) -> bool {
    fs::read_to_string(dir.join(MARKER_FILE))
        .ok()
        .and_then(|t| serde_json::from_str::<MarkerRecord>(&t).ok())
        .is_some_and(|r| r.host == host && r.username == username)
}

/// Removes the marker. Best effort: a missing file is the goal anyway.
pub fn clear(dir: &Path) {
    let _ = retry_transient(|| match fs::remove_file(dir.join(MARKER_FILE)) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        r => r,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_is_per_account() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        assert!(!is_marked(dir, "192.168.8.1", "admin"));
        mark(dir, "192.168.8.1", "admin", 1).unwrap();
        assert!(is_marked(dir, "192.168.8.1", "admin"));
        assert!(!is_marked(dir, "192.168.8.1", "user"));
        assert!(!is_marked(dir, "192.168.1.1", "admin"));
        clear(dir);
        assert!(!is_marked(dir, "192.168.8.1", "admin"));
        clear(dir);
    }

    #[test]
    fn an_unreadable_marker_counts_as_not_marked() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(MARKER_FILE), "192.168.8.1/admin").unwrap();
        assert!(!is_marked(dir.path(), "192.168.8.1", "admin"));
        fs::write(dir.path().join(MARKER_FILE), "{\"host\": 3}").unwrap();
        assert!(!is_marked(dir.path(), "192.168.8.1", "admin"));
    }
}
