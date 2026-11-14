use keyring::Entry;

use router_core::legacy::LEGACY_KEYCHAIN_SERVICE;

const SERVICE: &str = "router-band-tuner";

pub trait CredentialStore: Send + Sync {
    fn get(&self, host: &str, user: &str) -> Result<Option<String>, String>;
    fn set(&self, host: &str, user: &str, pw: &str) -> Result<(), String>;
    fn delete(&self, host: &str, user: &str) -> Result<(), String>;
}

/// OS keychain (Windows Credential Manager etc.).
pub struct KeyringStore;

fn entry(host: &str, user: &str) -> Result<Entry, String> {
    entry_in(SERVICE, host, user)
}

fn entry_in(service: &str, host: &str, user: &str) -> Result<Entry, String> {
    Entry::new(service, &format!("{host}/{user}")).map_err(|e| e.to_string())
}

/// Reads `new`, else moves a password saved under the pre-rename service into `new`.
fn get_or_migrate(new: &Entry, legacy: &Entry) -> Result<Option<String>, String> {
    if let Some(pw) = entry_get(new)? {
        return Ok(Some(pw));
    }
    let Some(pw) = entry_get(legacy)? else {
        return Ok(None);
    };
    entry_set(new, &pw)?;
    entry_delete(legacy)?;
    Ok(Some(pw))
}

fn entry_get(e: &Entry) -> Result<Option<String>, String> {
    match e.get_password() {
        Ok(p) => Ok(Some(p)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(err.to_string()),
    }
}

fn entry_set(e: &Entry, pw: &str) -> Result<(), String> {
    e.set_password(pw).map_err(|e| e.to_string())
}

fn entry_delete(e: &Entry) -> Result<(), String> {
    match e.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

impl CredentialStore for KeyringStore {
    fn get(&self, host: &str, user: &str) -> Result<Option<String>, String> {
        get_or_migrate(
            &entry(host, user)?,
            &entry_in(LEGACY_KEYCHAIN_SERVICE, host, user)?,
        )
    }
    fn set(&self, host: &str, user: &str, pw: &str) -> Result<(), String> {
        entry_set(&entry(host, user)?, pw)
    }
    fn delete(&self, host: &str, user: &str) -> Result<(), String> {
        entry_delete(&entry_in(LEGACY_KEYCHAIN_SERVICE, host, user)?)?;
        entry_delete(&entry(host, user)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keychain_roundtrip_unicode() {
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
        // The keyring mock keeps its secret inside one Entry, so exercise the
        // exact helpers KeyringStore uses against a single entry.
        let e = entry("192.168.8.1", "admin").unwrap();
        assert_eq!(entry_get(&e).unwrap(), None);
        let pw = "كلمة-سر-٢٠٢٦";
        entry_set(&e, pw).unwrap();
        assert_eq!(entry_get(&e).unwrap().as_deref(), Some(pw));
        entry_delete(&e).unwrap();
        assert_eq!(entry_get(&e).unwrap(), None);
        // deleting again is not an error
        entry_delete(&e).unwrap();
    }

    #[test]
    fn legacy_password_moves_to_new_service() {
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
        let new = entry("192.168.8.1", "admin").unwrap();
        let old = entry_in(LEGACY_KEYCHAIN_SERVICE, "192.168.8.1", "admin").unwrap();
        assert_eq!(get_or_migrate(&new, &old).unwrap(), None);
        entry_set(&old, "pw").unwrap();
        assert_eq!(get_or_migrate(&new, &old).unwrap().as_deref(), Some("pw"));
        assert_eq!(entry_get(&new).unwrap().as_deref(), Some("pw"));
        assert_eq!(entry_get(&old).unwrap(), None);
    }
}
