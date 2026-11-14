//! The app was called "CPE Band Tuner" up to v0.1.0. Its data folder and keychain service
//! carried that name, so both are picked up once under the new name.

use std::path::Path;

/// Data folder name (Tauri identifier) before the rename.
pub const LEGACY_APP_ID: &str = "io.github.hjbiki.cpe-band-tuner";
/// Keychain service before the rename.
pub const LEGACY_KEYCHAIN_SERVICE: &str = "cpe-band-tuner";

/// Moves the old data folder (next to `data_dir`) to `data_dir` if only the old one exists.
/// A pending-change journal must not be left behind: it is what reverts an unconfirmed change.
/// Returns whether a folder was moved.
pub fn migrate_data_dir(data_dir: &Path) -> std::io::Result<bool> {
    let legacy = data_dir.with_file_name(LEGACY_APP_ID);
    if data_dir.exists() || !legacy.is_dir() || legacy == data_dir {
        return Ok(false);
    }
    std::fs::rename(&legacy, data_dir)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("rbt-legacy-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn moves_old_folder_once() {
        let root = tmp("move");
        let old = root.join(LEGACY_APP_ID);
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("pending-change.json"), "{}").unwrap();
        let new = root.join("io.github.hjbiki.router-band-tuner");

        assert!(migrate_data_dir(&new).unwrap());
        assert!(new.join("pending-change.json").exists());
        assert!(!old.exists());
        assert!(!migrate_data_dir(&new).unwrap());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn leaves_both_alone_when_new_exists() {
        let root = tmp("keep");
        let old = root.join(LEGACY_APP_ID);
        let new = root.join("io.github.hjbiki.router-band-tuner");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();

        assert!(!migrate_data_dir(&new).unwrap());
        assert!(old.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
