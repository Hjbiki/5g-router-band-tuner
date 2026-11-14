//! The `password-rejected` marker.
//!
//! Written when the router rejects the saved password or locks the login. Every further try with
//! that password counts toward a lockout, so neither the launch auto-connect nor the Connect
//! button resends it until the password is saved again (or forgotten).
//!
//! The marker is written by the actor before the session view is published, so no reader of
//! alerts or snapshots can miss it, and an immediate Connect is already refused.

use std::fs;
use std::io;
use std::path::Path;

use crate::actor::SessionView;
use crate::journal::retry_transient;

/// File name in the app's data directory.
pub const MARKER_FILE: &str = "password-rejected";

/// The account a marker is for: `{host}/{user}`.
pub fn account_key(host: &str, user: &str) -> String {
    format!("{host}/{user}")
}

/// Records that `user` on `host` had its password rejected.
pub fn mark(dir: &Path, host: &str, user: &str) -> io::Result<()> {
    let key = account_key(host, user);
    retry_transient(|| fs::write(dir.join(MARKER_FILE), &key))
}

/// Whether the marker names this account.
pub fn is_marked(dir: &Path, host: &str, user: &str) -> bool {
    fs::read_to_string(dir.join(MARKER_FILE)).is_ok_and(|a| a == account_key(host, user))
}

/// Removes the marker. Best effort: a missing file is the goal anyway.
pub fn clear(dir: &Path) {
    let _ = retry_transient(|| match fs::remove_file(dir.join(MARKER_FILE)) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        r => r,
    });
}

/// Session views that mean "don't send this password again". A login refused with a code the app
/// doesn't recognise counts, except 125001 to 125003: those refuse the CSRF token, before any
/// password check, and a fresh process really does get them.
pub fn rejects_password(view: &SessionView) -> bool {
    match view {
        SessionView::BadCredentials | SessionView::LockedOut { .. } => true,
        SessionView::LoginRejected { code } => !(125_001..=125_003).contains(code),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_is_per_account() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        assert!(!is_marked(dir, "192.168.8.1", "admin"));
        mark(dir, "192.168.8.1", "admin").unwrap();
        assert!(is_marked(dir, "192.168.8.1", "admin"));
        // Only the rejected account is blocked.
        assert!(!is_marked(dir, "192.168.8.1", "user"));
        clear(dir);
        assert!(!is_marked(dir, "192.168.8.1", "admin"));
        // Clearing twice is harmless.
        clear(dir);
    }

    #[test]
    fn which_views_reject_the_password() {
        assert!(rejects_password(&SessionView::BadCredentials));
        assert!(rejects_password(&SessionView::LockedOut { wait_s: 60 }));
        // An unknown refusal might be about the password, so it isn't resent.
        assert!(rejects_password(&SessionView::LoginRejected {
            code: 108009
        }));
        // A refused token says nothing about the password.
        for code in [125001, 125002, 125003] {
            assert!(!rejects_password(&SessionView::LoginRejected { code }));
        }
        assert!(!rejects_password(&SessionView::TakenOver));
        assert!(!rejects_password(&SessionView::Connecting));
        assert!(!rejects_password(&SessionView::AlgorithmMismatch));
        // Nothing was sent, so there's no verdict on the password.
        assert!(!rejects_password(&SessionView::ConfirmRouter {
            first_time: false
        }));
    }
}
