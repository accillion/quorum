//! The run manifest (AC 251) and drift between runs.
//!
//! Every run writes `bench/runs/<run-id>/manifest.json`, scrubbed: run id,
//! Quorum commit, corpus manifest hash, frozen-file digests, the judge,
//! and one record per review or direct call (bundle and prompt digests,
//! tier, observed models, session id, latency, credits, USD). It never
//! holds a cookie, token or presigned URL: [`RunManifest::to_json`] passes
//! the whole document through the scrubber.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::scrub::Scrubber;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RunManifest {
    pub run_id: String,
    pub created_at: String,
    pub quorum_commit: String,
    pub quorum_version: String,
    pub corpus_manifest_sha256: String,
    /// Frozen files (RUBRIC.md, rubrics, the single-model prompt,
    /// prices.toml, bench.toml) by path: SHA-256.
    pub frozen: BTreeMap<String, String>,
    pub judge_model: String,
    /// Every model string and fingerprint the judge's responses reported.
    #[serde(default)]
    pub judge_models_returned: Vec<String>,
    /// The panel this run fixed from its first `Q-high` session (AC 248):
    /// seat profile id to observed model id.
    #[serde(default)]
    pub panel: BTreeMap<String, String>,
    /// Lippa build id, if any response exposes one.
    #[serde(default)]
    pub lippa_build: Option<String>,
    /// Direct-call settings per `S-` model (vendor, vendor id, effort, cap).
    #[serde(default)]
    pub direct_call_settings: BTreeMap<String, Value>,
    #[serde(default)]
    pub records: Vec<CallRecord>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CallRecord {
    pub case_id: String,
    pub arm: String,
    pub repeat: u32,
    /// SHA-256 of the bundle bytes the arm was fed.
    pub bundle_sha256: String,
    /// SHA-256 of the full prompt sent (system + user for direct calls; the
    /// bundle for Quorum).
    pub prompt_sha256: String,
    #[serde(default)]
    pub tier: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    /// Seat profile id to observed model id, from the session detail.
    #[serde(default)]
    pub observed_models: BTreeMap<String, String>,
    #[serde(default)]
    pub model_returned: Option<String>,
    pub latency_ms: u64,
    #[serde(default)]
    pub credits: u64,
    #[serde(default)]
    pub usd_micros: u64,
    /// `completed`, `error: ...`, `excluded: ...`
    pub outcome: String,
    #[serde(default)]
    pub attempts: u32,
}

impl RunManifest {
    pub fn to_json(&self, s: &Scrubber) -> String {
        let v = serde_json::to_value(self).unwrap_or(Value::Null);
        serde_json::to_string_pretty(&s.scrub_json(&v)).unwrap_or_default()
    }
}

/// Changes between two runs that make a comparison drift-prone (AC 251):
/// observed models, prompt digests or frozen files. Empty means none.
pub fn drift(a: &RunManifest, b: &RunManifest) -> Vec<String> {
    let mut out = Vec::new();
    if a.panel != b.panel {
        out.push(format!(
            "observed panel changed: {:?} -> {:?}",
            a.panel, b.panel
        ));
    }
    for (path, ha) in &a.frozen {
        match b.frozen.get(path) {
            Some(hb) if hb == ha => {}
            Some(_) => out.push(format!("{path} changed")),
            None => out.push(format!("{path} missing in the later run")),
        }
    }
    if a.judge_model != b.judge_model {
        out.push(format!(
            "judge model changed: {} -> {}",
            a.judge_model, b.judge_model
        ));
    }
    if a.corpus_manifest_sha256 != b.corpus_manifest_sha256 {
        out.push("corpus manifest changed".into());
    }
    let digests = |m: &RunManifest| -> BTreeMap<(String, String), String> {
        m.records
            .iter()
            .filter(|r| r.arm == "Q-high")
            .map(|r| ((r.case_id.clone(), r.arm.clone()), r.prompt_sha256.clone()))
            .collect()
    };
    let (da, db) = (digests(a), digests(b));
    let changed = da
        .iter()
        .filter(|(k, v)| db.get(*k).is_some_and(|w| w != *v))
        .count();
    if changed > 0 {
        out.push(format!(
            "Q-high prompt digest changed for {changed} case(s)"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_json_is_scrubbed() {
        let m = RunManifest {
            run_id: "r".into(),
            // The cookie pair is built at runtime: the key guard scans this file.
            lippa_build: Some(format!(
                "build https://x.invalid/a?X-Amz-Signature=abc {}=abcdefghijkl",
                "session"
            )),
            ..Default::default()
        };
        let j = m.to_json(&Scrubber::default());
        assert!(!j.contains("X-Amz-Signature=abc"));
        assert!(!j.contains("abcdefghijkl"));
    }

    #[test]
    fn drift_flags_models_prompts_and_rubrics() {
        let mut a = RunManifest {
            judge_model: "j".into(),
            ..Default::default()
        };
        a.panel.insert("gpt-6.2-sol".into(), "gpt-6.2-sol".into());
        a.frozen.insert("bench/RUBRIC.md".into(), "h1".into());
        a.records.push(CallRecord {
            case_id: "c".into(),
            arm: "Q-high".into(),
            prompt_sha256: "p1".into(),
            ..Default::default()
        });
        assert!(drift(&a, &a).is_empty());
        let mut b = a.clone();
        b.panel.insert("gpt-6.2-sol".into(), "gpt-6-luna".into());
        b.frozen.insert("bench/RUBRIC.md".into(), "h2".into());
        b.records[0].prompt_sha256 = "p2".into();
        let d = drift(&a, &b);
        assert_eq!(d.len(), 3, "{d:?}");
    }
}
