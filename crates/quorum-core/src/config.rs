//! `.quorum/config.toml` — written by `quorum link`, read by `quorum review`.
//!
//! Schema:
//! ```toml
//! project_id = "<lippa project id>"
//! base_url = "https://app.lippa.ai"
//! remote_url = true   # optional; default true. If `false`, archive omits remote_url.
//!
//! [memory]                          # Phase 1C — all keys optional
//! candidate_threshold = 3           # 2..=100; auto-promote N
//! local_convention_bundle_cap = 500 # 100..=2048; per-entry bundle bytes
//! candidate_expire_days = 90        # 0..=3650; 0 disables auto-expire
//! ```

use std::path::Path;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct QuorumConfig {
    pub project_id: String,
    pub base_url: String,
    #[serde(default = "default_true")]
    pub remote_url: bool,
    /// Phase 1C — `[memory]` section. Absent in `config.toml` falls back
    /// to [`MemoryConfig::default`] (no warning, per spec §3.4).
    #[serde(default)]
    pub memory: MemoryConfig,
}

fn default_true() -> bool {
    true
}

/// Phase 1C — `[memory]` section keys. All keys carry hard ranges per
/// spec §3.4 / §5.4; values outside the range surface as
/// [`ConfigError::OutOfRange`] (exit 2 at the CLI).
#[derive(Debug, Clone, Copy, serde::Deserialize, serde::Serialize)]
pub struct MemoryConfig {
    #[serde(default = "default_candidate_threshold")]
    pub candidate_threshold: u32,
    #[serde(default = "default_local_convention_bundle_cap")]
    pub local_convention_bundle_cap: u32,
    #[serde(default = "default_candidate_expire_days")]
    pub candidate_expire_days: u32,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        MemoryConfig {
            candidate_threshold: default_candidate_threshold(),
            local_convention_bundle_cap: default_local_convention_bundle_cap(),
            candidate_expire_days: default_candidate_expire_days(),
        }
    }
}

const fn default_candidate_threshold() -> u32 {
    3
}
const fn default_local_convention_bundle_cap() -> u32 {
    500
}
const fn default_candidate_expire_days() -> u32 {
    90
}

/// Inclusive hard ranges from spec §5.4. Validation runs after TOML parse;
/// out-of-range values fail rather than clamping (user-visible error).
const CANDIDATE_THRESHOLD_RANGE: (u32, u32) = (2, 100);
const LOCAL_CONVENTION_BUNDLE_CAP_RANGE: (u32, u32) = (100, 2048);
const CANDIDATE_EXPIRE_DAYS_RANGE: (u32, u32) = (0, 3650);

impl MemoryConfig {
    /// Returns `Err` with a human-readable message for the first key whose
    /// value falls outside its hard range. Called from [`read`] and from
    /// `quorum link` so the user sees the error at the write site too.
    pub fn validate(&self) -> Result<(), String> {
        check_range(
            "candidate_threshold",
            self.candidate_threshold,
            CANDIDATE_THRESHOLD_RANGE,
        )?;
        check_range(
            "local_convention_bundle_cap",
            self.local_convention_bundle_cap,
            LOCAL_CONVENTION_BUNDLE_CAP_RANGE,
        )?;
        check_range(
            "candidate_expire_days",
            self.candidate_expire_days,
            CANDIDATE_EXPIRE_DAYS_RANGE,
        )?;
        Ok(())
    }
}

fn check_range(key: &str, value: u32, (lo, hi): (u32, u32)) -> Result<(), String> {
    if value < lo || value > hi {
        Err(format!(
            "[memory] {key} = {value} is outside the allowed range {lo}..={hi}"
        ))
    } else {
        Ok(())
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ConfigError {
    #[error("config not found at {0}; run `quorum link --project <id>`")]
    NotFound(std::path::PathBuf),
    #[error("config read failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("config parse failed: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("config serialize failed: {0}")]
    Serialize(#[from] toml::ser::Error),
    /// Phase 1C — `[memory]` range violation (AC 164). The message
    /// includes the offending key, its value, and the allowed range.
    #[error("config out of range: {0}")]
    OutOfRange(String),
}

pub fn read(repo_root: &Path) -> Result<QuorumConfig, ConfigError> {
    let path = repo_root.join(".quorum").join("config.toml");
    if !path.exists() {
        return Err(ConfigError::NotFound(path));
    }
    let text = std::fs::read_to_string(&path)?;
    let cfg = toml::from_str::<QuorumConfig>(&text)?;
    cfg.memory.validate().map_err(ConfigError::OutOfRange)?;
    Ok(cfg)
}

pub fn write(repo_root: &Path, cfg: &QuorumConfig) -> Result<std::path::PathBuf, ConfigError> {
    let dir = repo_root.join(".quorum");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("config.toml");
    let text = toml::to_string_pretty(cfg)?;
    std::fs::write(&path, text)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn round_trip() {
        let dir = tempdir().unwrap();
        let cfg = QuorumConfig {
            project_id: "p_1".into(),
            base_url: "https://app.lippa.ai".into(),
            remote_url: true,
            memory: MemoryConfig::default(),
        };
        let path = write(dir.path(), &cfg).unwrap();
        assert!(path.exists());
        let got = read(dir.path()).unwrap();
        assert_eq!(got.project_id, "p_1");
        assert!(got.remote_url);
    }

    #[test]
    fn missing_config_errors_with_helpful_message() {
        let dir = tempdir().unwrap();
        match read(dir.path()) {
            Err(ConfigError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    fn write_raw(dir: &std::path::Path, body: &str) {
        let qd = dir.join(".quorum");
        std::fs::create_dir_all(&qd).unwrap();
        std::fs::write(qd.join("config.toml"), body).unwrap();
    }

    #[test]
    fn memory_defaults_when_section_absent() {
        // AC 165: default config (no [memory] section) yields the
        // documented defaults: threshold=3, cap=500, expire=90.
        let dir = tempdir().unwrap();
        write_raw(
            dir.path(),
            "project_id = \"p_1\"\nbase_url = \"https://app.lippa.ai\"\n",
        );
        let cfg = read(dir.path()).unwrap();
        assert_eq!(cfg.memory.candidate_threshold, 3);
        assert_eq!(cfg.memory.local_convention_bundle_cap, 500);
        assert_eq!(cfg.memory.candidate_expire_days, 90);
    }

    #[test]
    fn memory_explicit_values_parsed() {
        let dir = tempdir().unwrap();
        write_raw(
            dir.path(),
            "project_id = \"p_1\"\nbase_url = \"https://app.lippa.ai\"\n\
             [memory]\n\
             candidate_threshold = 5\n\
             local_convention_bundle_cap = 1024\n\
             candidate_expire_days = 30\n",
        );
        let cfg = read(dir.path()).unwrap();
        assert_eq!(cfg.memory.candidate_threshold, 5);
        assert_eq!(cfg.memory.local_convention_bundle_cap, 1024);
        assert_eq!(cfg.memory.candidate_expire_days, 30);
    }

    #[test]
    fn memory_threshold_out_of_range_rejected() {
        // AC 164: candidate_threshold = 0 / 1 / 101 — all rejected.
        for bad in [0u32, 1, 101] {
            let dir = tempdir().unwrap();
            write_raw(
                dir.path(),
                &format!(
                    "project_id = \"p\"\nbase_url = \"u\"\n\
                     [memory]\ncandidate_threshold = {bad}\n"
                ),
            );
            match read(dir.path()) {
                Err(ConfigError::OutOfRange(m)) => {
                    assert!(
                        m.contains("candidate_threshold"),
                        "error message must name the key, got: {m}"
                    );
                }
                other => panic!("expected OutOfRange for threshold={bad}, got {other:?}"),
            }
        }
        // Boundary OK: 2 and 100 pass.
        for ok in [2u32, 100] {
            let dir = tempdir().unwrap();
            write_raw(
                dir.path(),
                &format!(
                    "project_id = \"p\"\nbase_url = \"u\"\n\
                     [memory]\ncandidate_threshold = {ok}\n"
                ),
            );
            read(dir.path()).expect("boundary value must pass");
        }
    }

    #[test]
    fn memory_bundle_cap_out_of_range_rejected() {
        // AC 164: local_convention_bundle_cap range 100..=2048.
        for bad in [0u32, 99, 2049, 10_000] {
            let dir = tempdir().unwrap();
            write_raw(
                dir.path(),
                &format!(
                    "project_id = \"p\"\nbase_url = \"u\"\n\
                     [memory]\nlocal_convention_bundle_cap = {bad}\n"
                ),
            );
            match read(dir.path()) {
                Err(ConfigError::OutOfRange(m)) => {
                    assert!(m.contains("local_convention_bundle_cap"));
                }
                other => panic!("expected OutOfRange for cap={bad}, got {other:?}"),
            }
        }
    }

    #[test]
    fn memory_expire_days_out_of_range_rejected() {
        // AC 164: candidate_expire_days range 0..=3650 (0 disables).
        let dir = tempdir().unwrap();
        write_raw(
            dir.path(),
            "project_id = \"p\"\nbase_url = \"u\"\n\
             [memory]\ncandidate_expire_days = 3651\n",
        );
        match read(dir.path()) {
            Err(ConfigError::OutOfRange(m)) => {
                assert!(m.contains("candidate_expire_days"));
            }
            other => panic!("expected OutOfRange, got {other:?}"),
        }
        // 0 is explicitly allowed (disables auto-expire per spec §3.4).
        let dir = tempdir().unwrap();
        write_raw(
            dir.path(),
            "project_id = \"p\"\nbase_url = \"u\"\n\
             [memory]\ncandidate_expire_days = 0\n",
        );
        read(dir.path()).expect("0 is the disable sentinel and must be allowed");
    }
}
