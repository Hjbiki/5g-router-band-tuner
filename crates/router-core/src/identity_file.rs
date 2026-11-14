//! What the router looked like at the last login whose server signature checked out.
//!
//! `<app_data>/router-identity.json` lets the launch auto-connect tell the router from another
//! device at the same address before it sends a login proof. A device that doesn't know the
//! password can't get recorded, because the record is only written after the server signature
//! verifies. A file that can't be read counts as nothing recorded, which means asking first.
//!
//! The salt and iteration count are what any device at that address is told before a login, so
//! they aren't secrets. The file still stays out of diagnostics, logs and the UI.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::driver::RouterIdentity;
use crate::journal::write_atomic;

const IDENTITY_VERSION: u32 = 1;
const IDENTITY_FILE: &str = "router-identity.json";

#[derive(Debug, Serialize, Deserialize)]
struct IdentityRecord {
    version: u32,
    host: String,
    username: String,
    salt: String,
    iterations: u32,
}

/// The recorded identity of this account on this router, if there is one.
pub fn load(dir: &Path, host: &str, username: &str) -> Option<RouterIdentity> {
    let text = fs::read_to_string(dir.join(IDENTITY_FILE)).ok()?;
    let r: IdentityRecord = serde_json::from_str(&text).ok()?;
    (r.version == IDENTITY_VERSION && r.host == host && r.username == username)
        .then(|| RouterIdentity::new(&r.salt, r.iterations))
}

pub fn save(dir: &Path, host: &str, username: &str, identity: &RouterIdentity) -> io::Result<()> {
    let record = IdentityRecord {
        version: IDENTITY_VERSION,
        host: host.to_string(),
        username: username.to_string(),
        salt: identity.salt.clone(),
        iterations: identity.iterations,
    };
    let bytes = serde_json::to_vec_pretty(&record).map_err(io::Error::other)?;
    write_atomic(dir, IDENTITY_FILE, &bytes)
}

/// Forgets the record, so the next Connect the user presses counts as a first login.
pub fn remove(dir: &Path) {
    let _ = fs::remove_file(dir.join(IDENTITY_FILE));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_for_the_same_account_only() {
        let dir = tempfile::tempdir().unwrap();
        let id = RouterIdentity::new("AB12", 1000);
        assert_eq!(load(dir.path(), "192.168.8.1", "admin"), None);
        save(dir.path(), "192.168.8.1", "admin", &id).unwrap();
        assert_eq!(load(dir.path(), "192.168.8.1", "admin"), Some(id));
        assert_eq!(load(dir.path(), "192.168.1.1", "admin"), None);
        assert_eq!(load(dir.path(), "192.168.8.1", "user"), None);
        remove(dir.path());
        assert_eq!(load(dir.path(), "192.168.8.1", "admin"), None);
    }

    #[test]
    fn an_unreadable_file_counts_as_nothing_recorded() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(IDENTITY_FILE), "not json").unwrap();
        assert_eq!(load(dir.path(), "192.168.8.1", "admin"), None);
    }
}
