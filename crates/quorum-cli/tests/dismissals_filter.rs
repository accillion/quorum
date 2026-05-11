//! Phase 1B Stage 1 — filter-site behavior + cross-finding collision rate.
//!
//! Exercises `apply_dismissals_filter` indirectly via the
//! [`MemoryStore::load_active_dismissals`] + `finding_identity_hash` pair
//! that the CLI's filter site combines. Also asserts the post-adjudication
//! 3-input hash's collision rate stays ≤2% on a ~20-finding corpus
//! (`cross_finding_collision_rate` per D2/D3 directive).
//!
//! ACs covered: 66, 96, 97, 108, 114.

use std::collections::HashSet;

use quorum_core::memory::identity::finding_identity_hash;
use quorum_core::memory::{DismissalReason, LocalSqliteMemoryStore, MemoryStore};
use quorum_core::review::{Finding, FindingSource, Severity};
use tempfile::TempDir;

fn init_repo() -> TempDir {
    let td = TempDir::new().unwrap();
    let _ = git2::Repository::init(td.path()).unwrap();
    td
}

fn f(title: &str, src: FindingSource, models: &[&str]) -> Finding {
    Finding {
        severity: Severity::High,
        title: title.into(),
        body: format!("Confidence: 0.90. Supported by: {}.", models.join(", ")),
        source: src,
        supported_by: models.iter().map(|s| s.to_string()).collect(),
        confidence: Some(0.90),
    }
}

/// Mirror of `commands/review.rs::apply_dismissals_filter` semantics.
/// We can't call the CLI helper directly (it's a private fn on a binary
/// crate), so we re-encode the contract here against the public memory
/// API. If the filter-site shape changes in the CLI, update this mirror
/// to match — the assertion target is the *behavior*, not the helper.
fn filter_and_record(
    review: &mut quorum_core::Review,
    store: &LocalSqliteMemoryStore,
    session_id: &str,
) -> Vec<quorum_core::memory::FindingIdentityHash> {
    let active = store.load_active_dismissals().unwrap();
    let mut matched = Vec::new();
    let mut keep = Vec::with_capacity(review.findings.len());
    let original = std::mem::take(&mut review.findings);
    for f in original {
        let h = finding_identity_hash(&f);
        if active.contains_key(&h) {
            matched.push(h);
        } else {
            keep.push(f);
        }
    }
    review.findings = keep;
    if !matched.is_empty() {
        store
            .record_seen(&matched, session_id, time::OffsetDateTime::now_utc())
            .unwrap();
    }
    matched
}

#[test]
fn dismissed_finding_is_filtered_out_on_subsequent_review() {
    // AC 66: subsequent review suppresses findings matching active dismissals.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let to_dismiss = f(
        "Race condition in cache",
        FindingSource::Divergence,
        &["m1", "m2"],
    );
    let keep = f("Spurious log noise", FindingSource::Agreement, &["m1"]);

    store
        .dismiss(
            &to_dismiss,
            "head",
            "main",
            DismissalReason::WontFix,
            None,
            Some(time::Duration::days(365)),
        )
        .unwrap();

    let mut review = quorum_core::Review {
        session_id: "S1".into(),
        findings: vec![to_dismiss.clone(), keep.clone()],
        model_names: vec!["m1".into(), "m2".into()],
        elapsed: std::time::Duration::ZERO,
        project_id: None,
        base_url: "https://x".into(),
        summary_text: None,
        final_agreement_score: None,
    };
    let matched = filter_and_record(&mut review, &store, "S1");
    assert_eq!(matched.len(), 1);
    assert_eq!(review.findings.len(), 1);
    assert_eq!(review.findings[0].title, "Spurious log noise");
}

#[test]
fn record_seen_bumps_once_per_session_then_again_on_new_session() {
    // AC 96 / AC 108: counter increments idempotently per session.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let target = f("X", FindingSource::Divergence, &["m"]);
    store
        .dismiss(
            &target,
            "h",
            "main",
            DismissalReason::FalsePositive,
            None,
            Some(time::Duration::days(365)),
        )
        .unwrap();
    let hash = finding_identity_hash(&target);

    // Two filter passes within the same session — counter bumps once.
    let mut review = quorum_core::Review {
        session_id: "S1".into(),
        findings: vec![target.clone()],
        model_names: vec!["m".into()],
        elapsed: std::time::Duration::ZERO,
        project_id: None,
        base_url: "https://x".into(),
        summary_text: None,
        final_agreement_score: None,
    };
    filter_and_record(&mut review, &store, "S1");

    review.findings = vec![target.clone()];
    filter_and_record(&mut review, &store, "S1");

    let row = store.load_active_dismissals().unwrap()[&hash].clone();
    assert_eq!(row.recurrence_count, 2, "same session must bump only once");

    review.findings = vec![target.clone()];
    filter_and_record(&mut review, &store, "S2");
    let row = store.load_active_dismissals().unwrap()[&hash].clone();
    assert_eq!(row.recurrence_count, 3);
}

#[test]
fn permanent_dismissal_suppresses_indefinitely() {
    // AC 97 (new in v1.0 P33): permanent dismissals always suppress.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let target = f("permanent issue", FindingSource::Divergence, &["m"]);
    store
        .dismiss(
            &target,
            "h",
            "main",
            DismissalReason::Intentional,
            None,
            None, // permanent
        )
        .unwrap();
    let mut review = quorum_core::Review {
        session_id: "S".into(),
        findings: vec![target.clone()],
        model_names: vec!["m".into()],
        elapsed: std::time::Duration::ZERO,
        project_id: None,
        base_url: "https://x".into(),
        summary_text: None,
        final_agreement_score: None,
    };
    let matched = filter_and_record(&mut review, &store, "S");
    assert_eq!(matched.len(), 1);
    assert!(review.findings.is_empty());
}

#[test]
fn hash_stable_under_body_and_confidence_drift() {
    // AC 114: hash doesn't change when only body / confidence differ.
    let a = Finding {
        body: "Confidence: 0.95. Supported by: alpha, beta.".into(),
        confidence: Some(0.95),
        ..f(
            "Same finding",
            FindingSource::Divergence,
            &["alpha", "beta"],
        )
    };
    let b = Finding {
        body: "Confidence: 0.71. Supported by: alpha, beta.".into(),
        confidence: Some(0.71),
        ..f(
            "Same finding",
            FindingSource::Divergence,
            &["alpha", "beta"],
        )
    };
    assert_eq!(finding_identity_hash(&a), finding_identity_hash(&b));
}

#[test]
fn hash_changes_when_model_set_changes() {
    // AC 114: model-set change → new hash. Intentional: prior judgment
    // was made under a different evidence set (v1.0 §4.2.4).
    let a = f("T", FindingSource::Agreement, &["alpha", "beta"]);
    let b = f("T", FindingSource::Agreement, &["alpha", "beta", "gamma"]);
    assert_ne!(finding_identity_hash(&a), finding_identity_hash(&b));
}

#[test]
fn cross_finding_collision_rate() {
    // D2/D3 adjudication directive: run ~20 varied diffs against the
    // mock fixture, count distinct (title, source.type, sorted_models)
    // triples per logical finding. Assert collision rate ≤2%.
    //
    // Here "varied diffs" is simulated as 20 distinct findings spanning
    // sensible (title, source.type, model-set) combinations. For each
    // pair (i, j) with i < j we ask: does h(i) == h(j) when the
    // *intent* is that they are distinct? A collision is a false-match.
    //
    // Total pairs = C(20, 2) = 190. The 2% threshold = max 3 collisions.

    let findings = vec![
        f(
            "Race condition in cache",
            FindingSource::Divergence,
            &["m1", "m2"],
        ),
        f(
            "Race condition in cache",
            FindingSource::Divergence,
            &["m1", "m3"],
        ),
        f(
            "Race condition in queue",
            FindingSource::Divergence,
            &["m1", "m2"],
        ),
        f(
            "Unbounded recursion in parser",
            FindingSource::Divergence,
            &["m1", "m2", "m3"],
        ),
        f(
            "Missing input validation on endpoint",
            FindingSource::Divergence,
            &["m1"],
        ),
        f(
            "Authentication bypass via admin flag",
            FindingSource::Divergence,
            &["m1", "m2", "m3"],
        ),
        f(
            "SQL injection in search filter",
            FindingSource::Divergence,
            &["m2"],
        ),
        f(
            "Token leak in error response",
            FindingSource::Agreement,
            &["m1", "m3"],
        ),
        f(
            "Stale cache eviction policy",
            FindingSource::Agreement,
            &["m1", "m2"],
        ),
        f(
            "Inconsistent timezone handling",
            FindingSource::Agreement,
            &["m1", "m2"],
        ),
        f(
            "Hard-coded retry count",
            FindingSource::Agreement,
            &["m1", "m2"],
        ),
        f(
            "Off-by-one in pagination",
            FindingSource::Agreement,
            &["m1", "m3"],
        ),
        f(
            "Magic number 42 in scheduler",
            FindingSource::Assumption,
            &[],
        ),
        f(
            "Caller is expected to validate UTF-8",
            FindingSource::Assumption,
            &[],
        ),
        f(
            "File system is case-insensitive",
            FindingSource::Assumption,
            &[],
        ),
        f(
            "HTTPS terminated at the proxy",
            FindingSource::Assumption,
            &[],
        ),
        f(
            "Stack frames are bounded by recursion depth",
            FindingSource::Assumption,
            &[],
        ),
        f(
            "Logging level is INFO by default",
            FindingSource::Assumption,
            &[],
        ),
        f(
            "Process restart is acceptable on OOM",
            FindingSource::Assumption,
            &[],
        ),
        f(
            "Network is not partitioned during boot",
            FindingSource::Assumption,
            &[],
        ),
    ];
    assert_eq!(findings.len(), 20, "test corpus stays at 20 findings");
    let hashes: Vec<_> = findings.iter().map(finding_identity_hash).collect();

    // Collision counter.
    let mut collisions = 0;
    let n = findings.len();
    for i in 0..n {
        for j in (i + 1)..n {
            if hashes[i] == hashes[j] {
                collisions += 1;
            }
        }
    }
    let total_pairs = n * (n - 1) / 2;
    let rate = (collisions as f64) / (total_pairs as f64);
    assert!(
        rate <= 0.02,
        "cross-finding collision rate exceeds 2% guard: {} / {} = {:.2}%",
        collisions,
        total_pairs,
        rate * 100.0,
    );

    // Sanity: at least one distinct triple per logical finding (hashes
    // are stable, so distinct triples must produce distinct hashes).
    let distinct: HashSet<_> = hashes.iter().collect();
    assert!(
        distinct.len() >= 18,
        "expected close-to-distinct hashes; got {} distinct of 20",
        distinct.len()
    );
}
