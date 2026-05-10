//! `.quorum/config.toml` — written by `quorum link`, read by `quorum review`.
//!
//! Schema:
//! ```toml
//! project_id = "<lippa project id>"
//! base_url = "https://app.lippa.ai"
//! remote_url = true   # optional; default true. If `false`, archive omits remote_url.
//! ```

use std::path::Path;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct QuorumConfig {
    pub project_id: String,
    pub base_url: String,
    #[serde(default = "default_true")]
    pub remote_url: bool,
}

fn default_true() -> bool {
    true
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
}

pub fn read(repo_root: &Path) -> Result<QuorumConfig, ConfigError> {
    let path = repo_root.join(".quorum").join("config.toml");
    if !path.exists() {
        return Err(ConfigError::NotFound(path));
    }
    let text = std::fs::read_to_string(&path)?;
    Ok(toml::from_str::<QuorumConfig>(&text)?)
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
}
