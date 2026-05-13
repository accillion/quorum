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
fn opens_and_migrates_to_current_version() {
    // AC 104 (1B) + AC 145 (1C): schema_version row exists at the
    // current binary version after first open. Phase 1B asserted v=1;
    // Phase 1C bumps it to v=2 with the schema_meta forward-compat row.
    // AC 110: <repo_root>/.quorum/dismissals.sqlite created on first open.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    assert!(store.path().exists(), "sqlite file must exist after new()");
    let conn = Connection::open(store.path()).unwrap();
    let version: i64 = conn
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 2);
    let fwd: String = conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key = 'forward_compat_min_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fwd, "2");
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

// =================================================================
// Phase 1C Stage 1 — v2 migration, forward-compat, byte-count CHECK.
// =================================================================

/// Build a v1-shaped SQLite at `path`, with one Phase 1B dismissal row.
/// Mirrors the schema Phase 1B shipped, so we can verify the in-place
/// v1→v2 upgrade path.
fn build_v1_fixture(path: &std::path::Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE schema_version (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL
        );
        CREATE TABLE dismissals (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            finding_identity_hash TEXT NOT NULL UNIQUE,
            title_snapshot TEXT NOT NULL,
            body_snapshot TEXT,
            source_type_snapshot TEXT NOT NULL,
            models_snapshot TEXT NOT NULL,
            branch_snapshot TEXT NOT NULL,
            reason TEXT NOT NULL,
            note TEXT,
            dismissed_at TEXT NOT NULL,
            last_seen_at TEXT NOT NULL,
            last_seen_session_id TEXT,
            recurrence_count INTEGER NOT NULL DEFAULT 1,
            expires_at TEXT,
            repo_head_sha_first TEXT NOT NULL,
            promotion_state TEXT NOT NULL DEFAULT 'candidate',
            CHECK (recurrence_count >= 1),
            CHECK (reason IN ('false_positive','intentional','out_of_scope','wont_fix','other')),
            CHECK (reason != 'other' OR note IS NOT NULL),
            CHECK (promotion_state IN ('candidate','local_only','promoted_convention'))
        );
        CREATE INDEX idx_dismissals_expires_at ON dismissals(expires_at);
        INSERT INTO schema_version (version, applied_at) VALUES (1, '2026-01-01T00:00:00Z');",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO dismissals (
            finding_identity_hash, title_snapshot, source_type_snapshot,
            models_snapshot, branch_snapshot, reason,
            dismissed_at, last_seen_at, recurrence_count, repo_head_sha_first
        ) VALUES (?1, 'preexisting', 'agreement', '[]', 'main', 'false_positive',
                  ?2, ?2, 2, 'sha')",
        rusqlite::params!["e".repeat(64), "2026-01-01T00:00:00Z"],
    )
    .unwrap();
}

#[test]
fn migrate_upgrades_v1_fixture_to_v2() {
    // AC 145: v1→v2 migration runs cleanly on a Phase 1B-shaped fixture.
    // Pre-existing dismissal row survives; schema_meta marker present;
    // schema_version bumped to 2.
    let td = init_repo();
    let quorum_dir = td.path().join(".quorum");
    std::fs::create_dir_all(&quorum_dir).unwrap();
    let db_path = quorum_dir.join("dismissals.sqlite");
    build_v1_fixture(&db_path);

    // Opening the store runs the v2 migration.
    let _store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let conn = Connection::open(&db_path).unwrap();

    let version: i64 = conn
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 2, "schema_version must be bumped to 2");
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "schema_version must remain a single row");
    let fwd: String = conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key = 'forward_compat_min_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fwd, "2");

    // Pre-existing dismissal row preserved.
    let surviving: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM dismissals WHERE title_snapshot = 'preexisting'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(surviving, 1);

    // No backfill of state_transitions for pre-v2 rows (spec §4.1).
    let st_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM state_transitions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(st_count, 0);
}

#[test]
fn migrate_is_idempotent_on_v2() {
    // AC 145: re-running the migration on a v2 DB is a no-op (no errors,
    // no duplicate rows, schema_meta unchanged).
    let td = init_repo();
    let db_path = {
        let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
        store.path().to_owned()
    };
    // Insert a dismissal so we can detect any accidental wipe.
    {
        let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
        let f = sample_finding("idempotency probe", &["m"]);
        store
            .dismiss(
                &f,
                "h",
                "main",
                DismissalReason::FalsePositive,
                None,
                Some(time::Duration::days(365)),
            )
            .unwrap();
    }
    // Re-open several times: the migration runs each time but is a no-op.
    for _ in 0..3 {
        drop(LocalSqliteMemoryStore::new(td.path()).unwrap());
    }
    let conn = Connection::open(&db_path).unwrap();
    let version: i64 = conn
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 2);
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "no extra schema_version rows after re-open");
    let meta_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_meta WHERE key = 'forward_compat_min_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(meta_count, 1);
    let dismissals: i64 = conn
        .query_row("SELECT COUNT(*) FROM dismissals", [], |r| r.get(0))
        .unwrap();
    assert_eq!(dismissals, 1, "existing dismissal preserved across re-open");
}

#[test]
fn state_transitions_fk_cascades_on_dismissal_delete() {
    // AC 146: ON DELETE CASCADE from dismissals(finding_identity_hash)
    // wipes the audit log when a candidate row is deleted.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let f = sample_finding("for cascade", &["m"]);
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
    let hash_hex = finding_identity_hash(&f).to_hex();

    // Hand-insert an audit row (Stage 1 does not yet expose a write API
    // for state_transitions, but FK semantics are testable directly).
    let conn = Connection::open(store.path()).unwrap();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    conn.execute(
        "INSERT INTO state_transitions
            (finding_identity_hash, from_state, to_state, trigger, ts,
             by_review_session_id, recurrence_at_transition)
         VALUES (?1, 'candidate', 'local_only', 'auto_recurrence', ?2, ?3, 3)",
        rusqlite::params![hash_hex, 1_700_000_000_000i64, "S-cascade-test"],
    )
    .unwrap();
    let pre: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM state_transitions WHERE finding_identity_hash = ?1",
            [&hash_hex],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pre, 1);

    // Delete the dismissal via the trait (cascades through FK).
    assert!(store.delete(id).unwrap());
    let post: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM state_transitions WHERE finding_identity_hash = ?1",
            [&hash_hex],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(post, 0, "FK CASCADE must drop the audit log");
}

#[test]
fn forward_compat_rejects_future_schema_version() {
    // AC 174: a SQLite whose schema_version > CURRENT_VERSION is rejected
    // with SchemaTooNew. The store refuses to open even if the rest of
    // the file is well-formed.
    let td = init_repo();
    // First, normal open — bring it up to v2.
    drop(LocalSqliteMemoryStore::new(td.path()).unwrap());
    let db_path = td.path().join(".quorum").join("dismissals.sqlite");
    // Now hand-bump schema_version to 3.
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute("UPDATE schema_version SET version = 3", [])
            .unwrap();
    }
    let err = match LocalSqliteMemoryStore::new(td.path()) {
        Ok(_) => panic!("expected SchemaTooNew error"),
        Err(e) => e,
    };
    match err {
        MemoryError::SchemaTooNew {
            schema_version,
            forward_compat_min,
        } => {
            assert_eq!(schema_version, 3);
            assert_eq!(forward_compat_min, 2);
        }
        other => panic!("expected SchemaTooNew, got {other:?}"),
    }
}

#[test]
fn forward_compat_rejects_future_min_version_marker() {
    // AC 174 — second arm: schema_version is OK (==2) but
    // schema_meta.forward_compat_min_version is > CURRENT_VERSION.
    // Simulates a future-binary upgrade-then-downgrade scenario where
    // the schema integers don't change but the floor does.
    let td = init_repo();
    drop(LocalSqliteMemoryStore::new(td.path()).unwrap());
    let db_path = td.path().join(".quorum").join("dismissals.sqlite");
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('forward_compat_min_version', '3')",
            [],
        )
        .unwrap();
    }
    let err = match LocalSqliteMemoryStore::new(td.path()) {
        Ok(_) => panic!("expected SchemaTooNew error"),
        Err(e) => e,
    };
    match err {
        MemoryError::SchemaTooNew {
            schema_version,
            forward_compat_min,
        } => {
            assert_eq!(schema_version, 2);
            assert_eq!(forward_compat_min, 3);
        }
        other => panic!("expected SchemaTooNew, got {other:?}"),
    }
}

#[test]
fn conventions_text_byte_count_check_roundtrip() {
    // AC 151 — conventions.convention_text CHECK enforces 1..=4096 BYTES
    // (cast to BLOB), not codepoints. Stage 1 has no public write
    // surface for `conventions`, so insertions go through raw SQL.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    // We need a real dismissal row first (PK FK).
    let f = sample_finding("for-check-roundtrip", &["m"]);
    store
        .dismiss(
            &f,
            "h",
            "main",
            DismissalReason::FalsePositive,
            None,
            Some(time::Duration::days(365)),
        )
        .unwrap();
    let hash_hex = finding_identity_hash(&f).to_hex();
    let conn = Connection::open(store.path()).unwrap();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();

    let insert = |hash: &str, text: &str| -> rusqlite::Result<usize> {
        conn.execute(
            "INSERT INTO conventions
                (finding_identity_hash, convention_text, promoted_at, conventions_md_block_id)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![hash, text, 1_700_000_000_000i64, "abcdef012345"],
        )
    };

    // 4096 bytes — passes.
    let ok_4096 = "a".repeat(4096);
    insert(&hash_hex, &ok_4096).expect("4096-byte text must pass");
    // Re-using the same PK requires deletion first.
    conn.execute(
        "DELETE FROM conventions WHERE finding_identity_hash = ?1",
        [&hash_hex],
    )
    .unwrap();

    // 4097 bytes — fails on CHECK.
    let too_big = "a".repeat(4097);
    assert!(
        insert(&hash_hex, &too_big).is_err(),
        "4097-byte text must fail byte-count CHECK"
    );

    // 1 byte — passes.
    insert(&hash_hex, "x").expect("1-byte text must pass");
    conn.execute(
        "DELETE FROM conventions WHERE finding_identity_hash = ?1",
        [&hash_hex],
    )
    .unwrap();

    // 0 bytes — fails (CHECK floor is 1).
    assert!(
        insert(&hash_hex, "").is_err(),
        "empty text must fail byte-count CHECK"
    );

    // Multi-byte UTF-8: the CHECK measures BYTES, not codepoints. Each
    // emoji is 4 bytes in UTF-8, so 1025 emojis = 4100 bytes > 4096
    // even though codepoint count is well under 4096.
    let mut emoji_overflow = String::with_capacity(4100);
    for _ in 0..1025 {
        emoji_overflow.push('\u{1F600}'); // 😀 — 4 UTF-8 bytes
    }
    assert_eq!(emoji_overflow.len(), 4100);
    assert!(
        insert(&hash_hex, &emoji_overflow).is_err(),
        "multi-byte text >4096 bytes must fail; CHECK is byte-count, not codepoint-count"
    );

    // 1024 emojis = 4096 bytes — passes.
    let mut emoji_ok = String::with_capacity(4096);
    for _ in 0..1024 {
        emoji_ok.push('\u{1F600}');
    }
    assert_eq!(emoji_ok.len(), 4096);
    insert(&hash_hex, &emoji_ok).expect("exactly-4096-byte UTF-8 text must pass");
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
