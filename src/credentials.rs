use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::auth::{AuthError, Session, wipe};
use crate::dpapi;

pub const SESSION_FILE: &str = "session.bin";
pub const WEB_PROFILE_DIR: &str = "webview";

#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("could not access {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("could not decrypt {path} (saved by another Windows user?): {source}")]
    Decrypt { path: PathBuf, source: io::Error },
    #[error("saved session is corrupt; sign in again")]
    Corrupt,
    #[error(transparent)]
    Auth(#[from] AuthError),
}

pub fn save(dir: &Path, session: &Session) -> Result<PathBuf, CredentialError> {
    let path = dir.join(SESSION_FILE);
    let io_err = |source| CredentialError::Io {
        path: path.clone(),
        source,
    };
    let sealed = dpapi::protect(session.cookie().as_bytes()).map_err(io_err)?;
    fs::create_dir_all(dir).map_err(io_err)?;
    fs::write(&path, sealed).map_err(io_err)?;
    Ok(path)
}

pub fn load(dir: &Path) -> Result<Option<Session>, CredentialError> {
    let path = dir.join(SESSION_FILE);
    let sealed = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(CredentialError::Io { path, source }),
    };
    let plain =
        dpapi::unprotect(&sealed).map_err(|source| CredentialError::Decrypt { path, source })?;
    let mut cookie = String::from_utf8(plain).map_err(|_| CredentialError::Corrupt)?;
    let session = Session::from_cookie(&cookie);
    wipe(&mut cookie);
    Ok(Some(session?))
}

// The sign-in window's browser profile; it holds the Google login itself.
pub fn web_profile_dir(dir: &Path) -> PathBuf {
    dir.join(WEB_PROFILE_DIR)
}

pub fn remove(dir: &Path) -> Result<(), CredentialError> {
    let session = dir.join(SESSION_FILE);
    ignore_missing(fs::remove_file(&session), session)?;
    let profile = web_profile_dir(dir);
    ignore_missing(fs::remove_dir_all(&profile), profile)
}

fn ignore_missing(result: io::Result<()>, path: PathBuf) -> Result<(), CredentialError> {
    match result {
        Err(err) if err.kind() != io::ErrorKind::NotFound => {
            Err(CredentialError::Io { path, source: err })
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-creds")
            .join(name)
    }

    #[test]
    fn missing_session_means_signed_out() {
        assert!(load(&test_dir("does-not-exist")).unwrap().is_none());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = test_dir("round-trip");
        let session = Session::from_cookie("SAPISID=fake-test-value; HSID=h").unwrap();
        save(&dir, &session).unwrap();
        let loaded = load(&dir).unwrap().unwrap();
        assert_eq!(loaded.cookie(), session.cookie());
    }

    #[test]
    fn sign_out_removes_the_session_and_tolerates_a_missing_one() {
        let dir = test_dir("sign-out");
        let session = Session::from_cookie("SAPISID=fake-test-value").unwrap();
        save(&dir, &session).unwrap();
        let profile = web_profile_dir(&dir);
        fs::create_dir_all(profile.join("Default")).unwrap();
        fs::write(profile.join("Default").join("Cookies"), b"x").unwrap();
        remove(&dir).unwrap();
        assert!(load(&dir).unwrap().is_none());
        assert!(!profile.exists());
        remove(&dir).unwrap();
    }

    #[test]
    fn garbage_file_fails_to_decrypt() {
        let dir = test_dir("garbage");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(SESSION_FILE), b"not dpapi data").unwrap();
        assert!(matches!(load(&dir), Err(CredentialError::Decrypt { .. })));
    }
}
