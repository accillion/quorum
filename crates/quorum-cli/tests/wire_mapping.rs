//! Wire-format → Review mapping test, seeded from Lippa preflight payload.
//!
//! The fixture `tests/fixtures/lippa_v1_session_detail.json` is a verbatim
//! copy of a real converged Consensus session (session id replaced once at
//! preflight time, no other doctoring). Criterion 36's evidence anchor.

use quorum_core::{review_from_json, FindingSource, RepoMetadata, Severity};

fn fixture_value() -> serde_json::Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/lippa_v1_session_detail.json");
    let bytes = std::fs::read(&path).expect("fixture present");
    serde_json::from_slice(&bytes).expect("fixture parses")
}

fn repo() -> RepoMetadata {
    RepoMetadata {
        remote_url: Some("https://github.com/o/r".into()),
        branch: "main".into(),
        head_sha: "abc1234".into(),
        project_id: Some("p_1".into()),
        base_url: "https://app.lippa.ai".into(),
    }
}

#[test]
fn fixture_parses_into_review() {
    let v = fixture_value();
    let r = review_from_json(&v, &repo()).expect("review_from_json succeeds");
    assert_eq!(r.session_id, "cc36bbe2-41cb-4ae1-8a21-456307969008");
    assert!(r.summary_text.is_some(), "summary_text present");
}

#[test]
fn model_names_alphabetical_case_insensitive() {
    let v = fixture_value();
    let r = review_from_json(&v, &repo()).unwrap();
    let mut sorted = r.model_names.clone();
    sorted.sort_by_key(|a| a.to_lowercase());
    assert_eq!(r.model_names, sorted, "sorted case-insensitive");
    // Live preflight ran with 3 models; assert >= 2.
    assert!(r.model_names.len() >= 2, "multi-model");
}

#[test]
fn agreement_clusters_map_to_findings() {
    let v = fixture_value();
    let r = review_from_json(&v, &repo()).unwrap();
    let agreement: Vec<_> = r
        .findings
        .iter()
        .filter(|f| f.source == FindingSource::Agreement)
        .collect();
    assert!(
        !agreement.is_empty(),
        "preflight session had agreement clusters"
    );
    for f in agreement {
        // Severity must be Medium (conf>=0.85) or Low.
        assert!(
            matches!(f.severity, Severity::Medium | Severity::Low),
            "agreement severity must be Medium or Low, got {:?}",
            f.severity
        );
    }
}

#[test]
fn findings_ordered_divergence_first_then_agreement_then_assumption() {
    let v = fixture_value();
    let r = review_from_json(&v, &repo()).unwrap();
    let mut last_rank = 0;
    for f in &r.findings {
        let rank = match f.source {
            FindingSource::Divergence => 0,
            FindingSource::Agreement => 1,
            FindingSource::Assumption => 2,
        };
        assert!(rank >= last_rank, "findings not ordered by source");
        last_rank = rank;
    }
}

#[test]
fn missing_id_field_errors() {
    let bad = serde_json::json!({"summary_text": "x"});
    assert!(review_from_json(&bad, &repo()).is_err());
}
