//! Crash-safe record of a two-step (primary band, or band switch for a new IP) apply, next to
//! the journal.
//!
//! `<app_data>/pending-sequence.json` says which step of a primary-band apply is running or due,
//! so a quit, crash or restart between the steps doesn't silently leave the router on the
//! primary band alone. It's written with the journal's atomic write (temp file, fsync, rename)
//! and removed when the sequence ends. A file that can't be read, or that was written for another
//! host or driver, is never acted on: it's moved aside to `pending-sequence.ignored-<ms>.json`.
//!
//! The record holds network configs only: no credentials, tokens or device identifiers.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::journal::{retry_transient, sync_dir, write_atomic};
use crate::safe_apply::SequenceOrigin;
use crate::{LteBandSet, NetConfig};

pub const SEQUENCE_VERSION: u32 = 1;
const SEQUENCE_FILE: &str = "pending-sequence.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SequenceRecord {
    pub version: u32,
    pub driver: String,
    pub host: String,
    pub origin: SequenceOrigin,
    pub primary: u8,
    /// 1 (the primary band alone) or 2 (`final_config`).
    pub step: u8,
    pub final_config: NetConfig,
    /// The config from before step 1, once step 1's snapshot is known.
    pub original: Option<NetConfig>,
    /// Change id (journal id) of the step; `None` until that step's journal is written.
    pub change: Option<String>,
    /// Wall clock. Audit only.
    pub created_at_ms: u64,
    /// Step 1's config when it isn't `final_config` with the primary band alone (a band switch
    /// for a new IP). Left out of the file otherwise, so the shape stays as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step1: Option<NetConfig>,
}

impl SequenceRecord {
    /// The record describes a two-step apply the actor could have started: step 1 or 2, a valid
    /// primary band, and a final LTE set that holds it and at least one other band. A band
    /// switch for a new IP instead needs its own step 1, which holds the primary band (its first
    /// band) and differs from the final config; other origins never carry one.
    pub fn is_well_formed(&self) -> bool {
        let Ok(only) = LteBandSet::from_bands(&[self.primary]) else {
            return false;
        };
        if !matches!(self.step, 1 | 2) || !(1..=128).contains(&self.primary) {
            return false;
        }
        match (self.origin, &self.step1) {
            (SequenceOrigin::IpReattach, Some(step1)) => {
                step1.lte_bands.contains(self.primary)
                    && *step1 != self.final_config
                    && step1.network_mode == self.final_config.network_mode
            }
            (SequenceOrigin::IpReattach, None) | (_, Some(_)) => false,
            (_, None) => {
                self.final_config.lte_bands.contains(self.primary)
                    && self.final_config.lte_bands != only
            }
        }
    }
}

pub struct SequenceStore {
    dir: PathBuf,
}

impl SequenceStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(SEQUENCE_FILE)
    }

    /// `Ok(None)` when no sequence is on file. A corrupt file is an `InvalidData` error.
    pub fn load(&self) -> io::Result<Option<SequenceRecord>> {
        match fs::read(self.path()) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn write(&self, r: &SequenceRecord) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let bytes = serde_json::to_vec_pretty(r).map_err(io::Error::other)?;
        write_atomic(&self.dir, SEQUENCE_FILE, &bytes)
    }

    /// Removes the file; a missing file is fine.
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

    /// Moves the file to `pending-sequence.ignored-<at_ms>.json`, out of the way of new
    /// sequences but kept for the user to look at. Returns the new path.
    pub fn set_aside(&self, at_ms: u64) -> io::Result<PathBuf> {
        let target = aside_path(&self.dir, at_ms);
        retry_transient(|| fs::rename(self.path(), &target))?;
        sync_dir(&self.dir);
        Ok(target)
    }
}

fn aside_path(dir: &Path, at_ms: u64) -> PathBuf {
    dir.join(format!("pending-sequence.ignored-{at_ms}.json"))
}
