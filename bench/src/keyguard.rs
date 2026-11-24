//! The key guard (AC 247): the harness refuses to start if a key-shaped
//! value appears in any tracked file under `bench/`.
//!
//! "Tracked" means present in the repository index. "Key-shaped" means
//! any value of a secret this process holds (exact match), a vendor key
//! prefix with a long body, a `Bearer` token or a `session=` cookie (the
//! scrubber's matchers). A tracked `bench/.env.local` is refused outright.
//!
//! A hit can be allowed only by listing the SHA-256 of the matched text,
//! with its path and a reason, in `bench/keyguard-allow.toml`. The guard
//! never prints a matched value: a hit names the path, the line and the
//! kind of match.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::scrub;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub path: String,
    pub line: usize,
    pub kind: &'static str,
    /// SHA-256 of the matched text, for an allowlist entry. Not the value.
    pub digest: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct Allowlist {
    #[serde(default)]
    pub allow: Vec<AllowEntry>,
}

#[derive(Debug, Deserialize)]
pub struct AllowEntry {
    pub path: String,
    pub sha256: String,
    pub reason: String,
}

#[derive(thiserror::Error, Debug)]
pub enum GuardError {
    #[error("git: {0}")]
    Git(#[from] git2::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("keyguard-allow.toml: {0}")]
    Allowlist(String),
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let d = Sha256::digest(bytes);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

fn line_of(text: &str, at: usize) -> usize {
    text[..at].bytes().filter(|&b| b == b'\n').count() + 1
}

/// Every key-shaped match in one file's text.
pub fn scan_text(path: &str, text: &str, known_values: &[String]) -> Vec<Hit> {
    let mut hits = Vec::new();
    for v in known_values.iter().filter(|v| v.len() >= 8) {
        let mut from = 0;
        while let Some(rel) = text[from..].find(v.as_str()) {
            let at = from + rel;
            hits.push(Hit {
                path: path.into(),
                line: line_of(text, at),
                kind: "known secret value",
                digest: sha256_hex(v.as_bytes()),
            });
            from = at + v.len();
        }
    }
    for (s, e) in scrub::find_prefixed_tokens(text) {
        hits.push(Hit {
            path: path.into(),
            line: line_of(text, s),
            kind: "key-shaped token",
            digest: sha256_hex(&text.as_bytes()[s..e]),
        });
    }
    for (s, e) in scrub::find_session_cookies(text) {
        hits.push(Hit {
            path: path.into(),
            line: line_of(text, s),
            kind: "session cookie",
            digest: sha256_hex(&text.as_bytes()[s..e]),
        });
    }
    hits
}

/// Paths under `bench/` (forward slashes, repo-relative) in the index of
/// the repository that contains `repo_root`.
pub fn tracked_bench_files(repo_root: &Path) -> Result<Vec<String>, GuardError> {
    let repo = git2::Repository::open(repo_root)?;
    let index = repo.index()?;
    let mut out = Vec::new();
    for e in index.iter() {
        let p = String::from_utf8_lossy(&e.path).to_string();
        if p.starts_with("bench/") {
            out.push(p);
        }
    }
    Ok(out)
}

pub fn load_allowlist(path: &Path) -> Result<Allowlist, GuardError> {
    match std::fs::read_to_string(path) {
        Ok(t) => toml::from_str(&t).map_err(|e| GuardError::Allowlist(e.to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Allowlist::default()),
        Err(e) => Err(GuardError::Io(e)),
    }
}

/// Scan every tracked file under `bench/`. Returns the hits the allowlist
/// does not cover; an empty result means the harness may start.
pub fn check_repo(repo_root: &Path, known_values: &[String]) -> Result<Vec<Hit>, GuardError> {
    let allow = load_allowlist(&repo_root.join("bench").join("keyguard-allow.toml"))?;
    let mut hits = Vec::new();
    for rel in tracked_bench_files(repo_root)? {
        if rel == "bench/.env.local" {
            hits.push(Hit {
                path: rel.clone(),
                line: 0,
                kind: "bench/.env.local is tracked",
                digest: String::new(),
            });
            continue;
        }
        let full: PathBuf = repo_root.join(&rel);
        let bytes = match std::fs::read(&full) {
            Ok(b) => b,
            // Deleted in the working tree but still in the index: nothing to read.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(GuardError::Io(e)),
        };
        let text = String::from_utf8_lossy(&bytes);
        for h in scan_text(&rel, &text, known_values) {
            let allowed = allow
                .allow
                .iter()
                .any(|a| a.path == h.path && a.sha256 == h.digest && !a.reason.trim().is_empty());
            if !allowed {
                hits.push(h);
            }
        }
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn fake_key() -> String {
        format!("sk-{}", "Zq7".repeat(12))
    }

    fn repo_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let td = tempfile::TempDir::new().unwrap();
        let repo = git2::Repository::init(td.path()).unwrap();
        let mut idx = repo.index().unwrap();
        for (p, body) in files {
            let full = td.path().join(p);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, body).unwrap();
            idx.add_path(Path::new(p)).unwrap();
        }
        idx.write().unwrap();
        td
    }

    #[test]
    fn negative_control_a_planted_key_is_found() {
        let key = fake_key();
        let td = repo_with(&[(
            "bench/cases/x/diff.patch",
            &format!("+let k = \"{key}\";\n"),
        )]);
        let hits = check_repo(td.path(), &[]).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "bench/cases/x/diff.patch");
        assert_eq!(hits[0].line, 1);
        // The hit never carries the value itself.
        assert!(!format!("{hits:?}").contains(&key));
    }

    #[test]
    fn a_known_secret_value_is_found_even_without_a_key_shape() {
        let td = repo_with(&[("bench/notes.md", "cookie is plainvalue12345 ok\n")]);
        let hits = check_repo(td.path(), &["plainvalue12345".to_string()]).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, "known secret value");
    }

    #[test]
    fn untracked_and_outside_bench_files_are_not_scanned() {
        let key = fake_key();
        let td = repo_with(&[("README.md", &key)]);
        std::fs::create_dir_all(td.path().join("bench")).unwrap();
        std::fs::write(td.path().join("bench/untracked.txt"), &key).unwrap();
        assert!(check_repo(td.path(), &[]).unwrap().is_empty());
    }

    #[test]
    fn a_tracked_env_local_is_refused() {
        let td = repo_with(&[("bench/.env.local", "X=1\n")]);
        let hits = check_repo(td.path(), &[]).unwrap();
        assert_eq!(hits[0].kind, "bench/.env.local is tracked");
    }

    #[test]
    fn the_allowlist_needs_path_digest_and_reason() {
        let key = fake_key();
        let digest = sha256_hex(key.as_bytes());
        let allow_ok = format!(
            "[[allow]]\npath = \"bench/cases/x/diff.patch\"\nsha256 = \"{digest}\"\nreason = \"dummy key in upstream example code\"\n"
        );
        let td = repo_with(&[
            ("bench/cases/x/diff.patch", &key),
            ("bench/keyguard-allow.toml", &allow_ok),
        ]);
        assert!(check_repo(td.path(), &[]).unwrap().is_empty());

        let allow_no_reason = format!(
            "[[allow]]\npath = \"bench/cases/x/diff.patch\"\nsha256 = \"{digest}\"\nreason = \"\"\n"
        );
        let td = repo_with(&[
            ("bench/cases/x/diff.patch", &key),
            ("bench/keyguard-allow.toml", &allow_no_reason),
        ]);
        assert_eq!(check_repo(td.path(), &[]).unwrap().len(), 1);
    }

    /// The real repository: every tracked file under bench/ is clean.
    #[test]
    fn this_repository_has_no_key_shaped_values_under_bench() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let hits = check_repo(root, &[]).unwrap();
        assert!(
            hits.is_empty(),
            "key-shaped values in tracked bench files: {hits:?}"
        );
    }
}
