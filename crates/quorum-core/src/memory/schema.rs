//! SQLite schema v1 / v2 + migration runner.
//!
//! Phase 1B shipped v1 (dismissals + schema_version). Phase 1C adds v2:
//! `state_transitions`, `conventions`, `schema_meta`, plus a binary-side
//! forward-compat marker (`schema_meta.forward_compat_min_version`).
//!
//! The migration runner is idempotent: re-running on a v2 DB is a no-op
//! (all DDL is `IF NOT EXISTS`-guarded; `INSERT OR REPLACE` on schema_meta;
//! the `UPDATE schema_version SET version=2 WHERE version=1` is a no-op
//! once version=2). `schema_version` holds exactly one row whose `version`
//! field is bumped in place rather than appending a new row — this matches
//! Phase 1B's invariant (`reopen_is_idempotent` asserts `COUNT(*) = 1`).
//!
//! No backfill of `state_transitions` for pre-v2 dismissals (spec §4.1
//! last paragraph). Phase 1B rows have no audit trail and stay that way.

use rusqlite::{Connection, OptionalExtension};

use super::MemoryError;

/// The schema version this binary writes / understands.
pub const CURRENT_VERSION: i64 = 2;

/// `schema_meta` key declaring the minimum binary schema_version that may
/// safely open this DB. A 1C binary writes `'2'`; a hypothetical future v3
/// binary writes `'3'`.
pub const FORWARD_COMPAT_MIN_KEY: &str = "forward_compat_min_version";

const SQL_CREATE_SCHEMA_VERSION: &str = "
    CREATE TABLE IF NOT EXISTS schema_version (
        version INTEGER PRIMARY KEY,
        applied_at TEXT NOT NULL
    );
";

const SQL_CREATE_DISMISSALS: &str = "
    CREATE TABLE IF NOT EXISTS dismissals (
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
";

const SQL_CREATE_INDEX_EXPIRES_AT: &str =
    "CREATE INDEX IF NOT EXISTS idx_dismissals_expires_at ON dismissals(expires_at);";

// ----- Phase 1C v2 DDL ------------------------------------------------------
//
// `finding_identity_hash` is TEXT to match Phase 1B's `dismissals` column
// (the FK requires matching affinity). The spec §4.2 / §4.3 wording uses
// "BLOB" generically; the storage shape Phase 1B shipped is the hex-encoded
// string, and we keep that throughout v2 so the FK works without a column
// rewrite.

const SQL_CREATE_STATE_TRANSITIONS: &str = "
    CREATE TABLE IF NOT EXISTS state_transitions (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        finding_identity_hash TEXT NOT NULL
            REFERENCES dismissals(finding_identity_hash) ON DELETE CASCADE,
        from_state TEXT NOT NULL
            CHECK (from_state IN ('candidate','local_only','promoted_convention')),
        to_state   TEXT NOT NULL
            CHECK (to_state   IN ('candidate','local_only','promoted_convention')),
        trigger    TEXT NOT NULL
            CHECK (trigger    IN ('auto_recurrence','explicit_promote','explicit_demote')),
        ts INTEGER NOT NULL,
        by_review_session_id TEXT,
        recurrence_at_transition INTEGER,
        UNIQUE (finding_identity_hash, from_state, to_state, ts)
    );
";

const SQL_CREATE_INDEX_STATE_TRANSITIONS_HASH: &str =
    "CREATE INDEX IF NOT EXISTS idx_state_transitions_hash
        ON state_transitions(finding_identity_hash, ts);";

const SQL_CREATE_CONVENTIONS: &str = "
    CREATE TABLE IF NOT EXISTS conventions (
        finding_identity_hash TEXT PRIMARY KEY
            REFERENCES dismissals(finding_identity_hash) ON DELETE CASCADE,
        convention_text TEXT NOT NULL
            CHECK (length(CAST(convention_text AS BLOB)) BETWEEN 1 AND 4096),
        promoted_at INTEGER NOT NULL,
        conventions_md_block_id TEXT NOT NULL
    );
";

const SQL_CREATE_SCHEMA_META: &str = "
    CREATE TABLE IF NOT EXISTS schema_meta (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
";

/// Run all outstanding migrations up to [`CURRENT_VERSION`]. Idempotent.
///
/// - Fresh DB: applies v1 + v2 DDL, writes `schema_version` and
///   `schema_meta.forward_compat_min_version = '2'`.
/// - v1 DB (Phase 1B): applies v2 DDL, bumps `schema_version.version` from
///   1 to 2, writes `schema_meta.forward_compat_min_version = '2'`.
/// - v2 DB: no-op (all DDL is `IF NOT EXISTS`-guarded; the schema_version
///   UPDATE is a no-op; the schema_meta upsert is the documented INSERT
///   OR REPLACE).
///
/// The whole migration runs in one transaction so a failure leaves the DB
/// in its pre-migration state.
pub fn migrate(conn: &mut Connection) -> Result<(), MemoryError> {
    // The `schema_version` table itself is created outside the migration
    // transaction so we can probe the current version. This is consistent
    // with the Phase 1B pattern.
    conn.execute(SQL_CREATE_SCHEMA_VERSION, [])
        .map_err(|e| MemoryError::Migration(format!("schema_version DDL: {e}")))?;

    let current: Option<i64> = conn
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .optional()
        .map_err(|e| MemoryError::Migration(format!("schema_version probe: {e}")))?;

    let tx = conn
        .transaction()
        .map_err(|e| MemoryError::Migration(format!("begin migration tx: {e}")))?;

    let now_rfc3339 = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| MemoryError::Migration(format!("now() format: {e}")))?;

    match current {
        None => {
            // Fresh install: apply v1 DDL + v2 DDL, then INSERT the
            // version row at CURRENT_VERSION directly.
            apply_v1_ddl(&tx)?;
            apply_v2_ddl(&tx)?;
            tx.execute(
                "INSERT INTO schema_version (version, applied_at) VALUES (?1, ?2)",
                rusqlite::params![CURRENT_VERSION, now_rfc3339],
            )
            .map_err(|e| MemoryError::Migration(format!("schema_version insert: {e}")))?;
            write_forward_compat(&tx, CURRENT_VERSION)?;
        }
        Some(1) => {
            // Phase 1B → Phase 1C upgrade: v1 tables already exist.
            apply_v2_ddl(&tx)?;
            tx.execute(
                "UPDATE schema_version SET version = ?1, applied_at = ?2 WHERE version = 1",
                rusqlite::params![CURRENT_VERSION, now_rfc3339],
            )
            .map_err(|e| MemoryError::Migration(format!("schema_version bump: {e}")))?;
            write_forward_compat(&tx, CURRENT_VERSION)?;
        }
        Some(v) if v == CURRENT_VERSION => {
            // Already at current. Idempotent re-run: confirm v1+v2 DDL
            // present (no-op under IF NOT EXISTS) and recover the
            // schema_meta marker only if missing — never lower a higher
            // value, which a future-binary downgrade scenario could set.
            apply_v1_ddl(&tx)?;
            apply_v2_ddl(&tx)?;
            ensure_forward_compat_at_least(&tx, CURRENT_VERSION)?;
        }
        Some(v) => {
            // Future schema. The migration runner does NOT downgrade; the
            // binary-side forward-compat check in `sqlite::new` surfaces
            // this to the user. We commit a no-op tx to leave state
            // unchanged and let the caller handle the version mismatch.
            tx.commit()
                .map_err(|e| MemoryError::Migration(format!("commit no-op tx: {e}")))?;
            // No-op return so `sqlite::new` can read schema_version /
            // schema_meta and emit `SchemaTooNew`.
            let _ = v;
            return Ok(());
        }
    }

    tx.commit()
        .map_err(|e| MemoryError::Migration(format!("commit migration tx: {e}")))?;
    Ok(())
}

fn apply_v1_ddl(tx: &rusqlite::Transaction<'_>) -> Result<(), MemoryError> {
    tx.execute(SQL_CREATE_DISMISSALS, [])
        .map_err(|e| MemoryError::Migration(format!("dismissals DDL: {e}")))?;
    tx.execute(SQL_CREATE_INDEX_EXPIRES_AT, [])
        .map_err(|e| MemoryError::Migration(format!("index DDL: {e}")))?;
    Ok(())
}

fn apply_v2_ddl(tx: &rusqlite::Transaction<'_>) -> Result<(), MemoryError> {
    tx.execute(SQL_CREATE_STATE_TRANSITIONS, [])
        .map_err(|e| MemoryError::Migration(format!("state_transitions DDL: {e}")))?;
    tx.execute(SQL_CREATE_INDEX_STATE_TRANSITIONS_HASH, [])
        .map_err(|e| MemoryError::Migration(format!("state_transitions index: {e}")))?;
    tx.execute(SQL_CREATE_CONVENTIONS, [])
        .map_err(|e| MemoryError::Migration(format!("conventions DDL: {e}")))?;
    tx.execute(SQL_CREATE_SCHEMA_META, [])
        .map_err(|e| MemoryError::Migration(format!("schema_meta DDL: {e}")))?;
    Ok(())
}

fn write_forward_compat(tx: &rusqlite::Transaction<'_>, v: i64) -> Result<(), MemoryError> {
    tx.execute(
        "INSERT OR REPLACE INTO schema_meta (key, value) VALUES (?1, ?2)",
        rusqlite::params![FORWARD_COMPAT_MIN_KEY, v.to_string()],
    )
    .map_err(|e| MemoryError::Migration(format!("schema_meta write: {e}")))?;
    Ok(())
}

/// Insert the forward-compat marker only if absent. Never lowers an
/// existing value — preserves a future-binary-set floor across a
/// downgrade-then-rerun of a current-version binary.
fn ensure_forward_compat_at_least(
    tx: &rusqlite::Transaction<'_>,
    v: i64,
) -> Result<(), MemoryError> {
    tx.execute(
        "INSERT OR IGNORE INTO schema_meta (key, value) VALUES (?1, ?2)",
        rusqlite::params![FORWARD_COMPAT_MIN_KEY, v.to_string()],
    )
    .map_err(|e| MemoryError::Migration(format!("schema_meta ensure: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_is_idempotent_on_fresh() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        migrate(&mut conn).unwrap();
        let version: i64 = conn
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_VERSION);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "second migrate must not insert a second version row");
        let meta_v: String = conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key = ?1",
                [FORWARD_COMPAT_MIN_KEY],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(meta_v, "2");
    }

    #[test]
    fn migrate_upgrades_v1_to_v2_and_preserves_dismissals() {
        // AC 145: v1→v2 migration runs cleanly on a Phase 1B fixture DB.
        let mut conn = Connection::open_in_memory().unwrap();
        // Hand-roll a v1-shaped DB.
        conn.execute(SQL_CREATE_SCHEMA_VERSION, []).unwrap();
        conn.execute(SQL_CREATE_DISMISSALS, []).unwrap();
        conn.execute(SQL_CREATE_INDEX_EXPIRES_AT, []).unwrap();
        conn.execute(
            "INSERT INTO schema_version (version, applied_at) VALUES (1, '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        // Add a Phase 1B-shaped dismissal row.
        conn.execute(
            "INSERT INTO dismissals (
                finding_identity_hash, title_snapshot, source_type_snapshot,
                models_snapshot, branch_snapshot, reason,
                dismissed_at, last_seen_at, recurrence_count, repo_head_sha_first
            ) VALUES (?1, 't', 'agreement', '[]', 'main', 'false_positive',
                      ?2, ?2, 5, 'sha')",
            rusqlite::params!["a".repeat(64), "2026-01-01T00:00:00Z"],
        )
        .unwrap();

        migrate(&mut conn).unwrap();

        // schema_version bumped to 2.
        let version: i64 = conn
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 2);

        // schema_meta carries the forward-compat marker.
        let meta_v: String = conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key = ?1",
                [FORWARD_COMPAT_MIN_KEY],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(meta_v, "2");

        // Dismissal row preserved.
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM dismissals", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        // No backfill of state_transitions for the pre-v2 row (spec §4.1).
        let st_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM state_transitions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            st_count, 0,
            "no backfill of state_transitions for pre-v2 rows"
        );

        // Re-running is a no-op.
        migrate(&mut conn).unwrap();
        let count2: i64 = conn
            .query_row("SELECT COUNT(*) FROM dismissals", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count2, 1);
    }

    #[test]
    fn new_v2_tables_present_after_migrate() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        for table in ["state_transitions", "conventions", "schema_meta"] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "table {table} missing after migrate");
        }
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                  WHERE type='index' AND name='idx_state_transitions_hash'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "state_transitions index missing");
    }

    #[test]
    fn check_constraints_present() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        // recurrence_count >= 1 violation
        let err = conn.execute(
            "INSERT INTO dismissals (
                finding_identity_hash, title_snapshot, source_type_snapshot,
                models_snapshot, branch_snapshot, reason,
                dismissed_at, last_seen_at, repo_head_sha_first, recurrence_count
            ) VALUES (?1, 't', 'agreement', '[]', 'main', 'false_positive',
                      ?2, ?2, 'sha', 0)",
            rusqlite::params!["a".repeat(64), "2026-01-01T00:00:00Z"],
        );
        assert!(err.is_err(), "recurrence_count = 0 should fail CHECK");

        // reason 'other' without note
        let err = conn.execute(
            "INSERT INTO dismissals (
                finding_identity_hash, title_snapshot, source_type_snapshot,
                models_snapshot, branch_snapshot, reason,
                dismissed_at, last_seen_at, repo_head_sha_first
            ) VALUES (?1, 't', 'agreement', '[]', 'main', 'other',
                      ?2, ?2, 'sha')",
            rusqlite::params!["b".repeat(64), "2026-01-01T00:00:00Z"],
        );
        assert!(err.is_err(), "reason=other without note should fail CHECK");

        // unknown reason
        let err = conn.execute(
            "INSERT INTO dismissals (
                finding_identity_hash, title_snapshot, source_type_snapshot,
                models_snapshot, branch_snapshot, reason,
                dismissed_at, last_seen_at, repo_head_sha_first
            ) VALUES (?1, 't', 'agreement', '[]', 'main', 'bogus',
                      ?2, ?2, 'sha')",
            rusqlite::params!["c".repeat(64), "2026-01-01T00:00:00Z"],
        );
        assert!(err.is_err(), "unknown reason should fail CHECK");

        // unknown promotion_state
        let err = conn.execute(
            "INSERT INTO dismissals (
                finding_identity_hash, title_snapshot, source_type_snapshot,
                models_snapshot, branch_snapshot, reason,
                dismissed_at, last_seen_at, repo_head_sha_first, promotion_state
            ) VALUES (?1, 't', 'agreement', '[]', 'main', 'false_positive',
                      ?2, ?2, 'sha', 'bogus')",
            rusqlite::params!["d".repeat(64), "2026-01-01T00:00:00Z"],
        );
        assert!(err.is_err(), "unknown promotion_state should fail CHECK");
    }
}
