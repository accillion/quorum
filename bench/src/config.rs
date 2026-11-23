//! `bench/bench.toml`: the frozen run settings (part of the design freeze;
//! its SHA-256 is in DESIGN.md and every run manifest).

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchConfig {
    pub lippa: LippaCfg,
    pub quorum: QuorumCfg,
    pub run: RunCfg,
    pub s_arms: SArmsCfg,
    pub judge: JudgeCfg,
    pub extraction: StepCfg,
    pub clustering: StepCfg,
    pub adjudication: AdjudicationCfg,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LippaCfg {
    pub base_url: String,
    /// The dedicated bench project. Rolf creates it and fills this in
    /// before run 1b; it is empty in the frozen design.
    pub project_id: String,
    /// Worst-case credits reserved per Consensus session.
    pub credits_per_session: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuorumCfg {
    /// The Quorum commit the binary under test is built from.
    pub commit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunCfg {
    pub repeats: u32,
    pub tiers: Vec<String>,
    pub review_timeout_seconds: u64,
    /// Seed for every random choice: human-check samples, queue order.
    pub seed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SArmsCfg {
    pub prompt: String,
    pub output_cap: u64,
    /// Keyed by the Lippa model id a `Q-high` session reports.
    pub models: BTreeMap<String, SModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SModel {
    pub vendor: String,
    /// The vendor API model id the Lippa id maps to (AC 248).
    pub vendor_model: String,
    /// The panel seat's effort at the high tier (Gate A ruling 4).
    pub effort: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JudgeCfg {
    pub vendor: String,
    pub model: String,
    pub output_cap: u64,
    #[serde(default)]
    pub effort: Option<String>,
    pub seed: u64,
    pub rubric: String,
    /// Findings per judge call.
    pub batch_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepCfg {
    pub rubric: String,
    pub output_cap: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdjudicationCfg {
    /// The random share of non-mandatory decisions a human checks blind.
    pub human_share: f64,
}

#[derive(thiserror::Error, Debug)]
pub enum ConfigError {
    #[error("bench.toml: {0}")]
    Parse(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

impl BenchConfig {
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let c: BenchConfig = toml::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        if c.run.repeats == 0
            || c.s_arms.output_cap == 0
            || !(0.0..=1.0).contains(&c.adjudication.human_share)
        {
            return Err(ConfigError::Parse(
                "repeats and output caps must be positive; human_share in 0..=1".into(),
            ));
        }
        for t in &c.run.tiers {
            if !matches!(t.as_str(), "high" | "medium" | "low") {
                return Err(ConfigError::Parse(format!("tier `{t}`")));
            }
        }
        Ok(c)
    }

    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        Self::parse(&std::fs::read_to_string(path)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const SAMPLE: &str = r#"
[lippa]
base_url = "https://lippa.invalid"
project_id = ""
credits_per_session = 3
[quorum]
commit = "4f4ec22"
[run]
repeats = 3
tiers = ["high", "medium", "low"]
review_timeout_seconds = 1800
seed = 1
[s_arms]
prompt = "bench/prompts/single-v1.md"
output_cap = 16384
[s_arms.models."gpt-6.2-sol"]
vendor = "openai"
vendor_model = "gpt-6.2-sol"
effort = "medium"
[judge]
vendor = "mistral"
model = "mistral-large-4-0"
output_cap = 8192
seed = 7
rubric = "bench/RUBRIC.md"
batch_size = 40
[extraction]
rubric = "bench/rubrics/extraction-v1.md"
output_cap = 4096
[clustering]
rubric = "bench/rubrics/clustering-v1.md"
output_cap = 8192
[adjudication]
human_share = 0.2
"#;

    #[test]
    fn parses_and_validates() {
        let c = BenchConfig::parse(SAMPLE).unwrap();
        assert_eq!(c.s_arms.models["gpt-6.2-sol"].effort, "medium");
        assert!(BenchConfig::parse(&SAMPLE.replace("repeats = 3", "repeats = 0")).is_err());
        assert!(BenchConfig::parse(&SAMPLE.replace("\"low\"]", "\"ultra\"]")).is_err());
        assert!(
            BenchConfig::parse(&SAMPLE.replace("human_share = 0.2", "human_share = 2.0")).is_err()
        );
        assert!(BenchConfig::parse(&format!("{SAMPLE}\nunknown = 1\n")).is_err());
    }
}
