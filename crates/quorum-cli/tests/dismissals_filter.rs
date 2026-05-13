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
use quorum_core::memory::{
    DismissalReason, LocalSqliteMemoryStore, MemoryStore, PromotionState, TransitionTrigger,
};
use quorum_core::review::{Finding, FindingSource, Severity};
use rusqlite::Connection;
use std::sync::{Arc, Barrier};
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
    filter_and_record_with_threshold(review, store, session_id, 3)
}

fn filter_and_record_with_threshold(
    review: &mut quorum_core::Review,
    store: &LocalSqliteMemoryStore,
    session_id: &str,
    candidate_threshold: u32,
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
            .record_seen(
                &matched,
                session_id,
                time::OffsetDateTime::now_utc(),
                candidate_threshold,
            )
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

// ===============================================================
// Phase 1C Stage 1 — T1 auto-transition (candidate -> local_only).
// ===============================================================

/// AC 134: A fresh dismissal lives in `candidate` with no audit row.
#[test]
fn new_dismissal_is_candidate_with_no_audit_row() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let target = f("only-dismissed-once", FindingSource::Divergence, &["m"]);
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
    let row = store.load_active_dismissals().unwrap()[&hash].clone();
    assert_eq!(row.promotion_state, PromotionState::Candidate);

    let conn = Connection::open(store.path()).unwrap();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM state_transitions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0, "candidate state has no audit row (state is implicit)");
}

/// AC 135: T1 fires when `recurrence_count` reaches `candidate_threshold`.
/// Exactly one transition + one audit row + one returned `TransitionEvent`.
#[test]
fn record_seen_promotes_at_threshold_and_emits_one_event() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let target = f("threshold-walker", FindingSource::Divergence, &["m"]);
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
    let now = time::OffsetDateTime::now_utc();
    let threshold = 3u32;

    // Step 1 — recurrence goes 1 -> 2. Below threshold, no event.
    let events = store.record_seen(&[hash], "S-A", now, threshold).unwrap();
    assert!(events.is_empty(), "below-threshold call must emit no event");
    let row = store.load_active_dismissals().unwrap()[&hash].clone();
    assert_eq!(row.recurrence_count, 2);
    assert_eq!(row.promotion_state, PromotionState::Candidate);

    // Step 2 — recurrence goes 2 -> 3. Hits threshold. T1 fires.
    let events = store
        .record_seen(&[hash], "S-B", now + time::Duration::seconds(1), threshold)
        .unwrap();
    assert_eq!(events.len(), 1, "exactly one event at threshold");
    let ev = &events[0];
    assert_eq!(ev.from_state, PromotionState::Candidate);
    assert_eq!(ev.to_state, PromotionState::LocalOnly);
    assert_eq!(ev.trigger, TransitionTrigger::AutoRecurrence);
    assert_eq!(ev.recurrence_at_transition, 3);
    assert_eq!(ev.finding_identity_hash, hash);
    assert_eq!(
        ev.short_hash.len(),
        12,
        "short_hash must be the 12-char hex prefix"
    );

    // State flipped + audit row present.
    let row = store.load_active_dismissals().unwrap()[&hash].clone();
    assert_eq!(row.promotion_state, PromotionState::LocalOnly);
    assert_eq!(row.recurrence_count, 3);
    let conn = Connection::open(store.path()).unwrap();
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM state_transitions
             WHERE finding_identity_hash = ?1
               AND from_state = 'candidate' AND to_state = 'local_only'
               AND trigger = 'auto_recurrence'",
            [hash.to_hex()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1, "exactly one audit row");

    // Step 3 — recurrence continues past threshold (already local_only).
    // No event, no audit row, no state change.
    let events = store
        .record_seen(&[hash], "S-C", now + time::Duration::seconds(2), threshold)
        .unwrap();
    assert!(
        events.is_empty(),
        "post-threshold call must not re-fire T1: {events:?}"
    );
    let row = store.load_active_dismissals().unwrap()[&hash].clone();
    assert_eq!(row.recurrence_count, 4);
    assert_eq!(row.promotion_state, PromotionState::LocalOnly);
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM state_transitions WHERE finding_identity_hash = ?1",
            [hash.to_hex()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1, "no additional audit row after T1 once");
}

/// AC 136 + AC 144: two concurrent `record_seen` calls on the same hash
/// past the threshold produce exactly ONE state transition and exactly
/// ONE audit row. Uses `std::thread::spawn` (spec §C6) and
/// `std::sync::Barrier` to widen the timing spread past 1 ms. The
/// `rows_affected > 0` gate inside `record_seen` is the primary
/// protection; the SQL UNIQUE is secondary.
#[test]
fn concurrent_record_seen_at_threshold_fires_exactly_once() {
    let td = init_repo();
    let store_path = {
        let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
        let target = f("contended", FindingSource::Divergence, &["m"]);
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
        // Pre-bump to threshold-1 (recurrence = 2; next bump hits 3).
        let now = time::OffsetDateTime::now_utc();
        store
            .record_seen(&[hash], "S-pre", now, 1_000_000u32)
            .unwrap();
        store.path().to_owned()
    };

    // Open the store from the same repo root via two separate handles.
    // Each thread holds its own LocalSqliteMemoryStore so we exercise
    // the cross-handle race the WAL + Mutex pattern is meant to survive
    // (spec §3.2 T1: the rows_affected gate is the load-bearing
    // protection regardless of whether the threads share a Connection).
    let repo_root = td.path().to_owned();
    let f_clone = f("contended", FindingSource::Divergence, &["m"]);
    let hash = finding_identity_hash(&f_clone);

    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for tid in 0..2 {
        let bar = Arc::clone(&barrier);
        let root = repo_root.clone();
        let h = hash;
        handles.push(std::thread::spawn(move || {
            let store = LocalSqliteMemoryStore::new(&root).unwrap();
            bar.wait();
            // Add a small per-thread offset to widen timestamp spread
            // past 1 ms; AC 144 explicitly requires the gate to hold
            // even when the two writes don't collide on the same
            // millisecond. The seen_at parameter feeds the audit row's
            // `ts` column, so different values produce distinct
            // (hash, from, to, ts) tuples — meaning the UNIQUE
            // constraint cannot save us; only the rows_affected gate
            // can. That's exactly what AC 144 demands.
            let seen_at =
                time::OffsetDateTime::now_utc() + time::Duration::milliseconds(tid as i64 * 5);
            store
                .record_seen(&[h], &format!("S-{tid}"), seen_at, 3u32)
                .unwrap()
        }));
    }

    let mut total_events = 0usize;
    for h in handles {
        total_events += h.join().unwrap().len();
    }
    assert_eq!(
        total_events, 1,
        "exactly one TransitionEvent across both threads"
    );

    let conn = Connection::open(&store_path).unwrap();
    let audit_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM state_transitions
             WHERE finding_identity_hash = ?1
               AND from_state = 'candidate' AND to_state = 'local_only'",
            [hash.to_hex()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(audit_count, 1, "exactly one audit row");
    let state: String = conn
        .query_row(
            "SELECT promotion_state FROM dismissals WHERE finding_identity_hash = ?1",
            [hash.to_hex()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state, "local_only");
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
