//! Secrets: vendor and judge API keys and the Lippa cookie.
//!
//! Keys come only from environment variables or the gitignored
//! `bench/.env.local` (AC 247); the Lippa cookie only from
//! `QUORUM_LIPPA_SESSION` (Gate A ruling 5). A [`Secret`] never prints its
//! value: `Debug` and `Display` both show `<redacted>`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

/// Environment variables that hold secrets. The scrubber and the key guard
/// also match the *values* of whichever of these are set.
pub const SECRET_ENV_VARS: &[&str] = &[
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "GEMINI_API_KEY",
    "MISTRAL_API_KEY",
    "XAI_API_KEY",
    "QUORUM_LIPPA_SESSION",
];

/// The variable the Lippa cookie is read from. Never from `.env.local`.
pub const LIPPA_SESSION_ENV: &str = "QUORUM_LIPPA_SESSION";

#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    /// The raw value, for an HTTP header or a child process environment.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Parse `KEY=VALUE` lines. Blank lines and `#` comments are skipped; one
/// pair of surrounding quotes is removed. Unknown keys are kept, so the
/// caller decides what to use.
pub fn parse_env_file(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim();
            let v = v
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .or_else(|| v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
                .unwrap_or(v);
            out.insert(k.trim().to_string(), v.to_string());
        }
    }
    out
}

/// Where keys come from: the process environment first, then
/// `bench/.env.local`. The Lippa cookie is environment-only.
#[derive(Default)]
pub struct KeySource {
    file: BTreeMap<String, String>,
    env: BTreeMap<String, String>,
}

impl KeySource {
    /// Read the real environment and `<bench_dir>/.env.local` if present.
    pub fn load(bench_dir: &Path) -> std::io::Result<Self> {
        let path = bench_dir.join(".env.local");
        let file = match std::fs::read_to_string(&path) {
            Ok(t) => parse_env_file(&t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e),
        };
        let env = SECRET_ENV_VARS
            .iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
            .collect();
        Ok(Self { file, env })
    }

    /// For tests: explicit maps, no process environment.
    pub fn from_maps(env: BTreeMap<String, String>, file: BTreeMap<String, String>) -> Self {
        Self { file, env }
    }

    pub fn get(&self, name: &str) -> Option<Secret> {
        if let Some(v) = self.env.get(name).filter(|v| !v.is_empty()) {
            return Some(Secret::new(v.clone()));
        }
        if name == LIPPA_SESSION_ENV {
            return None;
        }
        self.file
            .get(name)
            .filter(|v| !v.is_empty())
            .map(|v| Secret::new(v.clone()))
    }

    /// Every secret value known to this source, for exact-value scrubbing.
    /// Values shorter than 8 bytes are left out: they would match ordinary
    /// text and they are not real keys.
    pub fn known_values(&self) -> Vec<String> {
        let mut v: Vec<String> = SECRET_ENV_VARS
            .iter()
            .filter_map(|k| self.get(k))
            .map(|s| s.expose().to_string())
            .filter(|s| s.len() >= 8)
            .collect();
        v.sort();
        v.dedup();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_never_formats_its_value() {
        let s = Secret::new("value-that-must-not-print");
        assert_eq!(format!("{s}"), "<redacted>");
        assert_eq!(format!("{s:?}"), "Secret(<redacted>)");
    }

    #[test]
    fn env_beats_file_and_cookie_is_env_only() {
        let mut env = BTreeMap::new();
        env.insert("XAI_API_KEY".into(), "from-env-xxxxxxxx".into());
        let mut file = BTreeMap::new();
        file.insert("XAI_API_KEY".into(), "from-file-xxxxxxx".into());
        file.insert("MISTRAL_API_KEY".into(), "mistral-file-xxxx".into());
        file.insert(LIPPA_SESSION_ENV.into(), "cookie-in-file-xx".into());
        let src = KeySource::from_maps(env, file);
        assert_eq!(
            src.get("XAI_API_KEY").unwrap().expose(),
            "from-env-xxxxxxxx"
        );
        assert_eq!(
            src.get("MISTRAL_API_KEY").unwrap().expose(),
            "mistral-file-xxxx"
        );
        assert!(
            src.get(LIPPA_SESSION_ENV).is_none(),
            "cookie must come from the environment only"
        );
    }

    #[test]
    fn env_file_parsing() {
        let m = parse_env_file("# c\n\nexport A=1\nB=\"two\"\nC='3'\nD = four \n");
        assert_eq!(m["A"], "1");
        assert_eq!(m["B"], "two");
        assert_eq!(m["C"], "3");
        assert_eq!(m["D"], "four");
    }
}
