//! Phase 1B Stage 1 integration tests for the `LocalSqliteMemoryStore`.
//!
//! Drives the store end-to-end through a fresh tempdir-rooted git repo:
//! open + migrate, dismiss + AlreadyDismissed, record_seen idempotence per
//! session, delete idempotence on unknown ids, CHECK constraints fire,
//! pragmas applied, reopen integrity after WAL sidecars present.
//!
//! ACs covered: 60, 96, 97, 98, 103, 104, 105, 106, 107, 108, 109, 110,
//! 111, 113, 118.

use quorum_core::memory::identity::finding_identity_hash;
use quorum_core::memory::{
    DismissalReason, FindingIdentityHash, LocalSqliteMemoryStore, MemoryError, MemoryStore,
    PromotionState, BODY_SNAPSHOT_MAX_BYTES,
};
use quorum_core::review::{Finding, FindingSource, Severity};
use rusqlite::Connection;
use tempfile::TempDir;

fn init_repo() -> TempDir {
    let td = TempDir::new().unwrap();
    let _ = git2::Repository::init(td.path()).unwrap();
    td
}

fn sample_finding(title: &str, models: &[&str]) -> Finding {
    Finding {
        severity: Severity::High,
        title: title.to_string(),
        body: format!("Confidence: 0.90. Supported by: {}.", models.join(", ")),
        source: FindingSource::Divergence,
        supported_by: models.iter().map(|s| s.to_string()).collect(),
        confidence: Some(0.90),
    }
}

#[test]
fn opens_and_migrates_to_v1() {
    // AC 104: schema_version row with version=1 exists after first open.
    // AC 110: <repo_root>/.quorum/dismissals.sqlite created on first open.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    assert!(store.path().exists(), "sqlite file must exist after new()");
    let conn = Connection::open(store.path()).unwrap();
    let version: i64 = conn
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 1);
}

#[test]
fn reopen_is_idempotent() {
    // AC 105: re-opening the DB does not re-run version=1 DDL.
    let td = init_repo();
    drop(LocalSqliteMemoryStore::new(td.path()).unwrap());
    drop(LocalSqliteMemoryStore::new(td.path()).unwrap());
    let conn = Connection::open(td.path().join(".quorum").join("dismissals.sqlite")).unwrap();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn pragmas_applied() {
    // AC 111: journal_mode (WAL or fallback), foreign_keys=ON, busy_timeout=5000.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let conn = Connection::open(store.path()).unwrap();
    let journal_mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    // WAL is expected; rollback journal ("delete") is the documented
    // fallback for NFS/SMB filesystems. Either is acceptable.
    assert!(
        journal_mode.eq_ignore_ascii_case("wal") || journal_mode.eq_ignore_ascii_case("delete"),
        "unexpected journal_mode {journal_mode}"
    );
    let foreign_keys: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
        .unwrap();
    assert_eq!(foreign_keys, 1, "foreign_keys must be ON");
    let busy_timeout: i64 = conn
        .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
        .unwrap();
    assert_eq!(busy_timeout, 5000, "busy_timeout must be 5000ms");
}

#[test]
fn dismiss_then_load_active_returns_row() {
    // AC 60 (default 365d) + AC 66 baseline (load reports the dismissal).
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let f = sample_finding("Critical bug", &["claude-sonnet", "gpt-4o"]);
    let id = store
        .dismiss(
            &f,
            "headsha",
            "main",
            DismissalReason::WontFix,
            None,
            Some(time::Duration::days(365)),
        )
        .unwrap();
    assert!(id.0 > 0);
    let active = store.load_active_dismissals().unwrap();
    let h = finding_identity_hash(&f);
    let row = active.get(&h).expect("dismissal must be active");
    assert_eq!(row.recurrence_count, 1);
    assert!(row.expires_at.is_some());
    assert_eq!(row.promotion_state, PromotionState::Candidate);
    assert_eq!(row.reason, DismissalReason::WontFix);
}

#[test]
fn dismiss_duplicate_returns_already_dismissed() {
    // AC 107: duplicate finding_identity_hash → AlreadyDismissed.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let f = sample_finding("dup", &["m"]);
    store
        .dismiss(
            &f,
            "headsha",
            "main",
            DismissalReason::FalsePositive,
            None,
            Some(time::Duration::days(365)),
        )
        .unwrap();
    let err = store
        .dismiss(
            &f,
            "headsha",
            "main",
            DismissalReason::FalsePositive,
            None,
            Some(time::Duration::days(365)),
        )
        .unwrap_err();
    assert!(matches!(err, MemoryError::AlreadyDismissed));
}

#[test]
fn record_seen_idempotent_per_session_and_bumps_per_new_session() {
    // AC 108: idempotent per (hash, session_id). AC 96: bumps once.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let f = sample_finding("ZZZ", &["m"]);
    store
        .dismiss(
            &f,
            "headsha",
            "main",
            DismissalReason::FalsePositive,
            None,
            Some(time::Duration::days(365)),
        )
        .unwrap();
    let h = finding_identity_hash(&f);
    let now = time::OffsetDateTime::now_utc();

    store.record_seen(&[h], "session-A", now).unwrap();
    store
        .record_seen(&[h], "session-A", now + time::Duration::seconds(1))
        .unwrap();
    let row = store.load_active_dismissals().unwrap()[&h].clone();
    assert_eq!(row.recurrence_count, 2, "session-A bumps once total");

    store
        .record_seen(&[h], "session-B", now + time::Duration::seconds(2))
        .unwrap();
    let row = store.load_active_dismissals().unwrap()[&h].clone();
    assert_eq!(row.recurrence_count, 3, "session-B bumps once more");
}

#[test]
fn delete_idempotent_on_unknown_and_known_ids() {
    // AC 109: delete returns Ok(false) on unknown, Ok(true) on known.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    assert!(!store
        .delete(quorum_core::memory::DismissalId(99_999))
        .unwrap());

    let f = sample_finding("D", &["m"]);
    let id = store
        .dismiss(
            &f,
            "h",
            "main",
            DismissalReason::Intentional,
            None,
            Some(time::Duration::days(365)),
        )
        .unwrap();
    assert!(store.delete(id).unwrap());
    assert!(!store.delete(id).unwrap());
}

#[test]
fn permanent_dismissal_persists_indefinitely() {
    // AC 97: permanent (`expires_at=NULL`) still returned by load_active.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let f = sample_finding("permanent", &["m"]);
    store
        .dismiss(&f, "h", "main", DismissalReason::Intentional, None, None)
        .unwrap();
    let active = store.load_active_dismissals().unwrap();
    let row = active.get(&finding_identity_hash(&f)).unwrap();
    assert!(row.expires_at.is_none());
}

#[test]
fn expired_dismissal_not_returned() {
    // AC 97 negative: explicitly expired rows are filtered out.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let f = sample_finding("already-expired", &["m"]);
    store
        .dismiss(
            &f,
            "h",
            "main",
            DismissalReason::FalsePositive,
            None,
            Some(time::Duration::seconds(-1)),
        )
        .unwrap();
    let active = store.load_active_dismissals().unwrap();
    assert!(active.is_empty());
}

#[test]
fn check_constraints_fire() {
    // AC 106: CHECK constraints. We verify by talking to SQLite directly
    // via the store's file (the trait API guards these via validate_note,
    // so to exercise SQLite's CHECK we drop down to raw SQL here).
    let td = init_repo();
    drop(LocalSqliteMemoryStore::new(td.path()).unwrap());
    let conn = Connection::open(td.path().join(".quorum").join("dismissals.sqlite")).unwrap();

    let insert_with = |reason: &str, note: Option<&str>, promo: &str, count: i64| {
        conn.execute(
            "INSERT INTO dismissals (
                finding_identity_hash, title_snapshot, source_type_snapshot,
                models_snapshot, branch_snapshot, reason, note,
                dismissed_at, last_seen_at, recurrence_count,
                repo_head_sha_first, promotion_state
            ) VALUES (?1, 't', 'agreement', '[]', 'main', ?2, ?3, ?4, ?4, ?5, 'sha', ?6)",
            rusqlite::params![
                format!("{:064}", count), // unique per insert attempt
                reason,
                note,
                "2026-01-01T00:00:00Z",
                count,
                promo,
            ],
        )
    };

    assert!(
        insert_with("false_positive", None, "candidate", 0).is_err(),
        "recurrence_count = 0 must fail"
    );
    assert!(
        insert_with("bogus_reason", None, "candidate", 1).is_err(),
        "unknown reason must fail"
    );
    assert!(
        insert_with("other", None, "candidate", 1).is_err(),
        "reason=other without note must fail"
    );
    assert!(
        insert_with("false_positive", None, "bogus_promo", 1).is_err(),
        "unknown promotion_state must fail"
    );
    // Sanity: a fully valid row goes in.
    assert!(
        insert_with("false_positive", None, "candidate", 1).is_ok(),
        "valid row should succeed"
    );
}

#[test]
fn body_snapshot_bounded_to_2kb_at_codepoint_boundary() {
    // AC: bounded ≤2KB; body_snapshot is a prefix.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let huge = "ä".repeat(3000); // 6000 bytes — well above 2KB.
    let f = Finding {
        body: huge.clone(),
        ..sample_finding("BIG", &["m"])
    };
    let id = store
        .dismiss(
            &f,
            "h",
            "main",
            DismissalReason::FalsePositive,
            None,
            Some(time::Duration::days(365)),
        )
        .unwrap();
    let row = store.get(id).unwrap().unwrap();
    let body = row.body_snapshot.unwrap();
    assert!(body.len() <= BODY_SNAPSHOT_MAX_BYTES);
    assert!(huge.starts_with(&body));
}

#[test]
fn reopen_after_dropping_with_wal_sidecars() {
    // AC 118: SQLite reopen integrity. Replaces v0.1's WAL-cleanup AC.
    let td = init_repo();
    let f = sample_finding("preserved", &["m"]);
    let key: FindingIdentityHash;
    {
        let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
        key = finding_identity_hash(&f);
        store
            .dismiss(
                &f,
                "h",
                "main",
                DismissalReason::WontFix,
                None,
                Some(time::Duration::days(365)),
            )
            .unwrap();
        // Implicit drop simulates hard-kill (no explicit close).
    }
    let store2 = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let active = store2.load_active_dismissals().unwrap();
    assert!(active.contains_key(&key));
}

#[test]
fn other_without_note_rejected_at_trait_layer() {
    // AC: trait-layer validation of OtherWithoutNote.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let f = sample_finding("o", &["m"]);
    let err = store
        .dismiss(
            &f,
            "h",
            "main",
            DismissalReason::Other,
            None,
            Some(time::Duration::days(365)),
        )
        .unwrap_err();
    assert!(matches!(err, MemoryError::OtherWithoutNote));
}

#[test]
fn note_size_and_format_validated() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let f = sample_finding("n", &["m"]);
    let too_big = "x".repeat(quorum_core::memory::NOTE_MAX_BYTES + 1);
    let err = store
        .dismiss(
            &f,
            "h",
            "main",
            DismissalReason::FalsePositive,
            Some(too_big),
            Some(time::Duration::days(365)),
        )
        .unwrap_err();
    assert!(matches!(err, MemoryError::InvalidNote));

    let with_newline = "valid\nnope".to_string();
    let f2 = sample_finding("n2", &["m"]);
    let err = store
        .dismiss(
            &f2,
            "h",
            "main",
            DismissalReason::FalsePositive,
            Some(with_newline),
            Some(time::Duration::days(365)),
        )
        .unwrap_err();
    assert!(matches!(err, MemoryError::InvalidNote));
}

#[test]
fn gitignore_written_on_first_open() {
    // AC 98: .quorum/dismissals.sqlite* appended to .gitignore.
    let td = init_repo();
    let _store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let gi = std::fs::read_to_string(td.path().join(".gitignore")).unwrap();
    assert!(gi.contains(".quorum/dismissals.sqlite*"));
    assert!(gi.contains("Quorum dismissals store"));
}

#[test]
fn no_secret_material_persisted() {
    // AC 124: dump the DB and confirm no cookie / password / session id /
    // env var values leak. The dismissals row carries only metadata.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let f = sample_finding("Normal finding", &["claude-sonnet", "gpt-4o", "gemini-pro"]);
    store
        .dismiss(
            &f,
            "head-sha-abc",
            "main",
            DismissalReason::FalsePositive,
            Some("safe note about a code pattern".into()),
            Some(time::Duration::days(365)),
        )
        .unwrap();
    drop(store);
    let bytes = std::fs::read(td.path().join(".quorum").join("dismissals.sqlite")).unwrap();
    let dump = String::from_utf8_lossy(&bytes);
    for forbidden in [
        "QUORUM_LIPPA_SESSION",
        "QUORUM_LIPPA_PASSWORD",
        "session=",
        "@lippa", // common email-domain prefix; insufficient on its own but a defense-in-depth grep
        "Cookie:",
    ] {
        assert!(
            !dump.contains(forbidden),
            "dismissals.sqlite must not leak {forbidden}"
        );
    }
}
