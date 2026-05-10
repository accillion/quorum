//! Hardcoded deny-list of secret-bearing filenames.
//!
//! Glob semantics (preflight + spec §4.3.2):
//!   - Patterns without `/` are basename-globbed at any depth.
//!     `.env` matches `infra/staging/.env` and `.env.production`.
//!   - Patterns with `/` are suffix-globbed: `.aws/credentials` matches
//!     `.aws/credentials` at any depth (`home/user/.aws/credentials`).
//!   - Patterns are case-sensitive (Linux/macOS authoritative). Windows
//!     filesystems are case-insensitive, but the *content* of the bundle
//!     contains repo-relative paths exactly as git2 reports them.

const PATTERNS: &[&str] = &[
    ".env",
    ".env.*",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    ".aws/credentials",
    ".npmrc",
    ".netrc",
    ".pgpass",
    "secrets.yml",
    "secrets.yaml",
    "secrets.json",
];

/// `true` if `path` (repo-relative, forward-slashed) matches the deny-list.
/// Always returns deterministic results — no globals, no I/O.
pub fn is_denied(path: &str) -> bool {
    let path = path.replace('\\', "/");
    let basename = path.rsplit('/').next().unwrap_or(&path);
    for pat in PATTERNS {
        if pat.contains('/') {
            // Suffix match: full path ends with `<sep?>pattern`.
            if path == *pat || path.ends_with(&format!("/{pat}")) {
                return true;
            }
        } else if basename_matches(basename, pat) {
            return true;
        }
    }
    false
}

/// Tiny glob matcher for the subset we actually use: literal text plus
/// optional `*` wildcards. Matches the entire `name`. Avoids pulling in a
/// glob crate for ~17 patterns.
fn basename_matches(name: &str, pattern: &str) -> bool {
    fn inner(name: &[u8], pat: &[u8]) -> bool {
        match (pat.first(), name.first()) {
            (None, None) => true,
            (None, Some(_)) => false,
            (Some(b'*'), _) => {
                // Try consuming 0..=name.len() chars.
                if inner(name, &pat[1..]) {
                    return true;
                }
                if let Some(rest) = name.get(1..) {
                    inner(rest, pat)
                } else {
                    false
                }
            }
            (Some(p), Some(n)) if p == n => inner(&name[1..], &pat[1..]),
            _ => false,
        }
    }
    inner(name.as_bytes(), pattern.as_bytes())
}

/// Returns the first matching pattern (for the bundle's exclusion marker).
pub fn reason(path: &str) -> Option<&'static str> {
    let path = path.replace('\\', "/");
    let basename = path.rsplit('/').next().unwrap_or(&path).to_string();
    for pat in PATTERNS {
        if pat.contains('/') {
            if path == *pat || path.ends_with(&format!("/{pat}")) {
                return Some(pat);
            }
        } else if basename_matches(&basename, pat) {
            return Some(pat);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basename_patterns_match_at_any_depth() {
        assert!(is_denied(".env"));
        assert!(is_denied("infra/staging/.env"));
        assert!(is_denied("services/api/.env.production"));
        assert!(is_denied("services/api/secrets.yml"));
        assert!(is_denied("a/b/c/id_rsa"));
        assert!(is_denied("certs/server.pem"));
        assert!(is_denied("vendor/license.key"));
    }

    #[test]
    fn path_patterns_match_at_root_and_nested() {
        assert!(is_denied(".aws/credentials"));
        assert!(is_denied("home/user/.aws/credentials"));
    }

    #[test]
    fn unrelated_paths_are_not_denied() {
        assert!(!is_denied("src/main.rs"));
        assert!(!is_denied("README.md"));
        assert!(!is_denied("config.toml"));
        assert!(!is_denied("environment.rs"));
    }

    #[test]
    fn windows_path_separators_normalized() {
        assert!(is_denied("infra\\staging\\.env"));
        assert!(is_denied("home\\user\\.aws\\credentials"));
    }

    #[test]
    fn reason_returns_matching_pattern() {
        assert_eq!(reason(".env"), Some(".env"));
        assert_eq!(reason("infra/staging/.env"), Some(".env"));
        assert_eq!(reason(".aws/credentials"), Some(".aws/credentials"));
        assert_eq!(reason("src/main.rs"), None);
    }
}
