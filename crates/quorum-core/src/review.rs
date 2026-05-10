//! `Review`, `Finding`, severity model, and the `review_from_json` mapping.
//!
//! Phase 1A `Finding` shape diverges from the original spec: Lippa's
//! Consensus detail response is debate-shaped (summary_text + agreement /
//! divergence / assumptions clusters), not findings-shaped. There is no
//! per-finding severity, file, or line range in the wire format.
//! `review_from_json` synthesizes severity from cluster type and confidence.
//! See `specs/Quorum-Phase1A-Preflight-notes.md` §"Spec divergences" D2.

use std::time::Duration;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Review {
    pub session_id: String,
    pub findings: Vec<Finding>,
    /// Alphabetically sorted (case-insensitive) at the mapping boundary,
    /// so AC 17 stdout and render snapshots stay deterministic across
    /// Lippa response shuffles. Preflight-notes §"Determinism for stable rendering".
    pub model_names: Vec<String>,
    pub elapsed: Duration,
    pub project_id: Option<String>,
    pub base_url: String,
    /// The model-aggregated prose summary from Lippa's `summary_text`.
    /// Rendered as the `## Summary` block, separate from per-cluster findings.
    pub summary_text: Option<String>,
    /// Final agreement score (0..=1) from Lippa, or `None` when missing.
    pub final_agreement_score: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Finding {
    pub severity: Severity,
    pub title: String,
    pub body: String,
    pub source: FindingSource,
    pub supported_by: Vec<String>,
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    High,
    Medium,
    Low,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingSource {
    Agreement,
    Divergence,
    Assumption,
}

/// Repo-side metadata supplied by `quorum-cli`. `review_from_json` does
/// not look at the filesystem; everything it needs comes from `Value` + this.
#[derive(Debug, Clone)]
pub struct RepoMetadata {
    pub remote_url: Option<String>,
    pub branch: String,
    pub head_sha: String,
    pub project_id: Option<String>,
    pub base_url: String,
}

#[derive(thiserror::Error, Debug)]
pub enum ParseError {
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("unexpected severity value: {0}")]
    UnknownSeverity(String),
    #[error("malformed JSON shape: {0}")]
    Malformed(String),
}

/// Confidence threshold at which an `agreement` cluster is promoted to
/// `Severity::Medium`. Below it, agreement clusters are `Low`.
pub const AGREEMENT_HIGH_CONFIDENCE: f64 = 0.85;

/// Parse Lippa's GET /consensus/sessions/{id} payload into a `Review`.
///
/// Reduced-scope mapping (spec divergence D2):
///   - `summary_text` → `Review.summary_text`
///   - `divergence[].claim_text` → Finding{severity=High, source=Divergence}
///   - `agreement[].claim_text` with confidence ≥ 0.85 → Medium
///   - `agreement[].claim_text` with confidence < 0.85 → Low
///   - `assumptions[].claim_text` → Info
///
/// Severity ordering is preserved by iteration order (divergence first,
/// then agreement, then assumptions). `tests/render.rs` snapshots rely on this.
pub fn review_from_json(
    value: &serde_json::Value,
    repo: &RepoMetadata,
) -> Result<Review, ParseError> {
    let obj = value
        .as_object()
        .ok_or_else(|| ParseError::Malformed("top-level value not an object".into()))?;

    let session_id = obj
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or(ParseError::MissingField("id"))?
        .to_string();

    let summary_text = obj
        .get("summary_text")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let final_agreement_score = obj.get("final_agreement_score").and_then(|v| v.as_f64());

    // Model names from `models[].vendor` (preflight confirmed it carries
    // useful vendor strings like "claude-sonnet", "gpt-4o", "gemini-pro");
    // fall back to display_name then id if vendor is absent.
    let mut model_names: Vec<String> = obj
        .get("models")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.as_object())
                .filter_map(|m| {
                    m.get("vendor")
                        .and_then(|v| v.as_str())
                        .or_else(|| m.get("display_name").and_then(|v| v.as_str()))
                        .or_else(|| m.get("id").and_then(|v| v.as_str()))
                        .map(|s| s.to_string())
                })
                .collect()
        })
        .unwrap_or_default();
    model_names.sort_by_key(|a| a.to_lowercase());
    model_names.dedup();

    let mut findings: Vec<Finding> = Vec::new();
    findings.extend(parse_clusters(
        obj,
        "divergence",
        FindingSource::Divergence,
    )?);
    findings.extend(parse_clusters(obj, "agreement", FindingSource::Agreement)?);
    findings.extend(parse_clusters(
        obj,
        "assumptions",
        FindingSource::Assumption,
    )?);

    // Elapsed is computed CLI-side from started/completed at the moment
    // of review; we don't trust Lippa's `elapsed_seconds` since it's only
    // present on /status. Caller supplies via the archive layer if known.
    Ok(Review {
        session_id,
        findings,
        model_names,
        elapsed: Duration::ZERO,
        project_id: repo.project_id.clone(),
        base_url: repo.base_url.clone(),
        summary_text,
        final_agreement_score,
    })
}

fn parse_clusters(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &'static str,
    source: FindingSource,
) -> Result<Vec<Finding>, ParseError> {
    let Some(arr) = obj.get(key).and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let m = item
            .as_object()
            .ok_or_else(|| ParseError::Malformed(format!("{key}[] entry not an object")))?;
        let title = m
            .get("claim_text")
            .and_then(|v| v.as_str())
            .ok_or(ParseError::MissingField("claim_text"))?
            .to_string();
        let confidence = m.get("confidence").and_then(|v| v.as_f64());
        let supported_by: Vec<String> = m
            .get("supported_by")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let severity = match source {
            FindingSource::Divergence => Severity::High,
            FindingSource::Agreement => {
                if confidence.unwrap_or(0.0) >= AGREEMENT_HIGH_CONFIDENCE {
                    Severity::Medium
                } else {
                    Severity::Low
                }
            }
            FindingSource::Assumption => Severity::Info,
        };

        let mut body = String::new();
        if let Some(conf) = confidence {
            body.push_str(&format!("Confidence: {:.2}.", conf));
        }
        if !supported_by.is_empty() {
            if !body.is_empty() {
                body.push(' ');
            }
            body.push_str(&format!("Supported by: {}.", supported_by.join(", ")));
        }

        out.push(Finding {
            severity,
            title,
            body,
            source,
            supported_by,
            confidence,
        });
    }
    Ok(out)
}

impl Review {
    /// `true` if any finding has severity High. Drives exit code 1 vs 0.
    pub fn has_high_severity(&self) -> bool {
        self.findings.iter().any(|f| f.severity == Severity::High)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoMetadata {
        RepoMetadata {
            remote_url: None,
            branch: "main".into(),
            head_sha: "abc1234".into(),
            project_id: Some("proj_x".into()),
            base_url: "https://app.lippa.ai".into(),
        }
    }

    #[test]
    fn maps_summary_and_clusters() {
        let v = serde_json::json!({
            "id": "sess_1",
            "status": "converged",
            "summary_text": "OK.",
            "final_agreement_score": 0.92,
            "models": [
                {"id":"m1","vendor":"gpt-4o","display_name":"GPT 4o","role":"analyst","dropped":false,"health_state":"active"},
                {"id":"m2","vendor":"claude-sonnet","display_name":"Claude","role":"analyst","dropped":false,"health_state":"active"}
            ],
            "agreement": [
                {"cluster_id":"a","claim_text":"safe","confidence":0.95,"supported_by":["gpt-4o","claude-sonnet"],"has_memory_match":false},
                {"cluster_id":"b","claim_text":"meh","confidence":0.5,"supported_by":["gpt-4o"],"has_memory_match":false}
            ],
            "divergence": [
                {"cluster_id":"c","claim_text":"disputed","confidence":0.4,"supported_by":["claude-sonnet"],"has_memory_match":false}
            ],
            "assumptions": [],
            "rounds": []
        });
        let r = review_from_json(&v, &repo()).unwrap();
        assert_eq!(r.session_id, "sess_1");
        assert_eq!(r.summary_text.as_deref(), Some("OK."));
        // model_names sorted case-insensitive
        assert_eq!(
            r.model_names,
            vec!["claude-sonnet".to_string(), "gpt-4o".to_string()]
        );
        // Findings ordered divergence-first
        assert_eq!(r.findings[0].source, FindingSource::Divergence);
        assert_eq!(r.findings[0].severity, Severity::High);
        assert_eq!(r.findings[1].source, FindingSource::Agreement);
        assert_eq!(r.findings[1].severity, Severity::Medium);
        assert_eq!(r.findings[2].source, FindingSource::Agreement);
        assert_eq!(r.findings[2].severity, Severity::Low);
        assert!(r.has_high_severity());
    }

    #[test]
    fn missing_id_errors() {
        let v = serde_json::json!({"summary_text":"x"});
        match review_from_json(&v, &repo()) {
            Err(ParseError::MissingField("id")) => {}
            other => panic!("expected MissingField(id), got {other:?}"),
        }
    }

    #[test]
    fn empty_clusters_yield_no_findings() {
        let v = serde_json::json!({
            "id":"s","models":[],"agreement":[],"divergence":[],"assumptions":[]
        });
        let r = review_from_json(&v, &repo()).unwrap();
        assert!(r.findings.is_empty());
        assert!(!r.has_high_severity());
    }
}
