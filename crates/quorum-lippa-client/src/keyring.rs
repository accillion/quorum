//! Cookie storage: OS keychain (default) or file fallback (`--no-keyring`).
//!
//! Per-host account scoping (`lippa-session@<host>`) so one machine can hold
//! concurrent staging + prod sessions.

use crate::secret::Secret;
use std::path::PathBuf;

const SERVICE: &str = "quorum";

#[derive(thiserror::Error, Debug)]
pub enum KeyringError {
    #[error("invalid base url: {0}")]
    InvalidBaseUrl(String),
    #[error("keyring operation failed: {0}")]
    Keyring(String),
    #[error("file fallback i/o: {0}")]
    Io(String),
}

#[derive(Debug, Clone)]
pub enum Storage {
    OsKeyring,
    File(PathBuf),
}

fn account_for(base_url: &str) -> Result<String, KeyringError> {
    let parsed = url::Url::parse(base_url)
        .map_err(|_| KeyringError::InvalidBaseUrl(base_url.to_string()))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| KeyringError::InvalidBaseUrl(base_url.to_string()))?
        .to_string();
    Ok(format!("lippa-session@{host}"))
}

pub fn store_cookie(storage: &Storage, base_url: &str, value: &Secret) -> Result<(), KeyringError> {
    match storage {
        Storage::OsKeyring => {
            let account = account_for(base_url)?;
            let entry = keyring::Entry::new(SERVICE, &account)
                .map_err(|e| KeyringError::Keyring(e.to_string()))?;
            entry
                .set_password(value.expose())
                .map_err(|e| KeyringError::Keyring(e.to_string()))?;
            Ok(())
        }
        Storage::File(path) => {
            let path = file_path_for(path, base_url)?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| KeyringError::Io(e.to_string()))?;
            }
            std::fs::write(&path, value.expose()).map_err(|e| KeyringError::Io(e.to_string()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                    .map_err(|e| KeyringError::Io(e.to_string()))?;
            }
            Ok(())
        }
    }
}

pub fn load_cookie(storage: &Storage, base_url: &str) -> Result<Option<Secret>, KeyringError> {
    match storage {
        Storage::OsKeyring => {
            let account = account_for(base_url)?;
            let entry = keyring::Entry::new(SERVICE, &account)
                .map_err(|e| KeyringError::Keyring(e.to_string()))?;
            match entry.get_password() {
                Ok(s) => Ok(Some(Secret::new(s))),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(KeyringError::Keyring(e.to_string())),
            }
        }
        Storage::File(path) => {
            let p = file_path_for(path, base_url)?;
            if !p.exists() {
                return Ok(None);
            }
            let s = std::fs::read_to_string(&p).map_err(|e| KeyringError::Io(e.to_string()))?;
            Ok(Some(Secret::new(s)))
        }
    }
}

pub fn delete_cookie(storage: &Storage, base_url: &str) -> Result<(), KeyringError> {
    match storage {
        Storage::OsKeyring => {
            let account = account_for(base_url)?;
            let entry = keyring::Entry::new(SERVICE, &account)
                .map_err(|e| KeyringError::Keyring(e.to_string()))?;
            match entry.delete_credential() {
                Ok(()) => Ok(()),
                Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(KeyringError::Keyring(e.to_string())),
            }
        }
        Storage::File(path) => {
            let p = file_path_for(path, base_url)?;
            if p.exists() {
                std::fs::remove_file(&p).map_err(|e| KeyringError::Io(e.to_string()))?;
            }
            Ok(())
        }
    }
}

fn file_path_for(base: &std::path::Path, base_url: &str) -> Result<PathBuf, KeyringError> {
    let parsed = url::Url::parse(base_url)
        .map_err(|_| KeyringError::InvalidBaseUrl(base_url.to_string()))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| KeyringError::InvalidBaseUrl(base_url.to_string()))?;
    Ok(base.join(format!("{host}.session")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn file_storage_round_trip() {
        let dir = tempdir().unwrap();
        let storage = Storage::File(dir.path().to_path_buf());
        let url = "https://app.lippa.ai";
        assert!(load_cookie(&storage, url).unwrap().is_none());
        store_cookie(&storage, url, &Secret::new("abc".into())).unwrap();
        let got = load_cookie(&storage, url).unwrap().unwrap();
        assert_eq!(got.expose(), "abc");
        delete_cookie(&storage, url).unwrap();
        assert!(load_cookie(&storage, url).unwrap().is_none());
        // Idempotent delete.
        delete_cookie(&storage, url).unwrap();
    }

    #[test]
    fn account_includes_host_only() {
        assert_eq!(
            account_for("https://app.lippa.ai/foo").unwrap(),
            "lippa-session@app.lippa.ai"
        );
    }

    #[test]
    fn invalid_base_url_rejected() {
        assert!(matches!(
            account_for("nothttp"),
            Err(KeyringError::InvalidBaseUrl(_))
        ));
    }

    #[test]
    fn file_paths_are_per_host() {
        let dir = tempdir().unwrap();
        let storage = Storage::File(dir.path().to_path_buf());
        store_cookie(&storage, "https://app.lippa.ai", &Secret::new("p".into())).unwrap();
        store_cookie(
            &storage,
            "https://staging.lippa.ai",
            &Secret::new("s".into()),
        )
        .unwrap();
        assert_eq!(
            load_cookie(&storage, "https://app.lippa.ai")
                .unwrap()
                .unwrap()
                .expose(),
            "p"
        );
        assert_eq!(
            load_cookie(&storage, "https://staging.lippa.ai")
                .unwrap()
                .unwrap()
                .expose(),
            "s"
        );
    }
}
