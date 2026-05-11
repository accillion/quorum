//! SQLite schema v1 + migration runner.
//!
//! v1.0 §4.2.3. Idempotent re-open: `schema_version` is queried first;
//! if `(version=1, applied_at=...)` exists, DDL is skipped. After
//! successful DDL, the row is inserted.

use rusqlite::{Connection, OptionalExtension};

use super::MemoryError;

/// The version this binary writes.
pub const CURRENT_VERSION: i64 = 1;

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

/// Run the v1 migration if not already applied. Idempotent.
pub fn migrate_to_v1(conn: &Connection) -> Result<(), MemoryError> {
    conn.execute(SQL_CREATE_SCHEMA_VERSION, [])
        .map_err(|e| MemoryError::Migration(format!("schema_version DDL: {e}")))?;
    let existing: Option<i64> = conn
        .query_row(
            "SELECT version FROM schema_version WHERE version = ?1",
            [CURRENT_VERSION],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| MemoryError::Migration(format!("schema_version probe: {e}")))?;
    if existing.is_some() {
        return Ok(());
    }
    conn.execute(SQL_CREATE_DISMISSALS, [])
        .map_err(|e| MemoryError::Migration(format!("dismissals DDL: {e}")))?;
    conn.execute(SQL_CREATE_INDEX_EXPIRES_AT, [])
        .map_err(|e| MemoryError::Migration(format!("index DDL: {e}")))?;
    let now_rfc3339 = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| MemoryError::Migration(format!("now() format: {e}")))?;
    conn.execute(
        "INSERT INTO schema_version (version, applied_at) VALUES (?1, ?2)",
        rusqlite::params![CURRENT_VERSION, now_rfc3339],
    )
    .map_err(|e| MemoryError::Migration(format!("schema_version insert: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        migrate_to_v1(&conn).unwrap();
        migrate_to_v1(&conn).unwrap();
        let version: i64 = conn
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 1);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "second migrate must not insert a second version row");
    }

    #[test]
    fn check_constraints_present() {
        let conn = Connection::open_in_memory().unwrap();
        migrate_to_v1(&conn).unwrap();
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
