//! `LocalSqliteMemoryStore` — the Phase 1B impl of [`MemoryStore`].
//!
//! Holds a `rusqlite::Connection` for the lifetime of one `quorum review`
//! invocation. CLI constructs once, drops at exit.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use time::format_description::well_known::Rfc3339;

use super::gitignore;
use super::identity::FindingIdentityHash;
use super::schema;
use super::{
    truncate_at_codepoint_boundary, validate_note, Dismissal, DismissalId, DismissalReason,
    MemoryError, MemoryStore, PromotionState, BODY_SNAPSHOT_MAX_BYTES,
};
use crate::review::Finding;

pub struct LocalSqliteMemoryStore {
    conn: Mutex<Connection>,
    db_path: PathBuf,
}

impl LocalSqliteMemoryStore {
    /// Open or create `<repo_root>/.quorum/dismissals.sqlite`.
    ///
    /// Side effects on first creation:
    ///   - Creates `<repo_root>/.quorum/` (idempotent).
    ///   - Runs the v1 migration (idempotent).
    ///   - Applies pragmas: `journal_mode=WAL` (falls back to `DELETE`
    ///     with stderr note if WAL isn't supported by the filesystem;
    ///     P37), `synchronous=NORMAL`, `foreign_keys=ON`,
    ///     `busy_timeout=5000`.
    ///   - Writes `.quorum/dismissals.sqlite*` to `.gitignore` if not
    ///     already covered.
    ///   - Emits a stderr warning if the DB is currently tracked by git.
    pub fn new(repo_root: &Path) -> Result<Self, MemoryError> {
        let quorum_dir = repo_root.join(".quorum");
        std::fs::create_dir_all(&quorum_dir).map_err(|e| MemoryError::Backend(Box::new(e)))?;
        let db_path = quorum_dir.join("dismissals.sqlite");
        let conn = Connection::open(&db_path).map_err(|e| MemoryError::Backend(Box::new(e)))?;

        // WAL with fallback. `journal_mode = WAL` returns the new mode as
        // a single-column row; we read it and fall back to DELETE if not 'wal'.
        let mode: String = conn
            .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
            .map_err(|e| MemoryError::Backend(Box::new(e)))?;
        if !mode.eq_ignore_ascii_case("wal") {
            let _ = conn.pragma_update(None, "journal_mode", "DELETE");
            eprintln!(
                "warning: WAL mode unavailable on this filesystem; using rollback journal. \
                 Concurrent invocations may serialize more aggressively."
            );
        }
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| MemoryError::Backend(Box::new(e)))?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| MemoryError::Backend(Box::new(e)))?;
        conn.pragma_update(None, "busy_timeout", 5000)
            .map_err(|e| MemoryError::Backend(Box::new(e)))?;

        schema::migrate_to_v1(&conn)?;

        // .gitignore discipline (warnings only; do not fail open).
        let _ = gitignore::ensure_ignored(repo_root).map(|wrote| {
            if wrote {
                eprintln!("wrote .gitignore: .quorum/dismissals.sqlite*");
            }
        });
        if gitignore::is_tracked(repo_root) {
            eprintln!(
                "warning: .quorum/dismissals.sqlite is tracked by git; this file contains \
                 free-text dismissal notes and should be gitignored. \
                 Run `git rm --cached .quorum/dismissals.sqlite` and gitignore the file."
            );
        }

        Ok(Self {
            conn: Mutex::new(conn),
            db_path,
        })
    }

    /// Path to the underlying SQLite file. Useful for tests + `dismissals show`.
    pub fn path(&self) -> &Path {
        &self.db_path
    }
}

fn now_rfc3339() -> Result<String, MemoryError> {
    time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|e| MemoryError::Backend(Box::new(e)))
}

fn rfc3339(t: time::OffsetDateTime) -> Result<String, MemoryError> {
    t.format(&Rfc3339)
        .map_err(|e| MemoryError::Backend(Box::new(e)))
}

fn parse_rfc3339(s: &str) -> Result<time::OffsetDateTime, MemoryError> {
    time::OffsetDateTime::parse(s, &Rfc3339).map_err(|e| MemoryError::Backend(Box::new(e)))
}

fn row_to_dismissal(row: &rusqlite::Row<'_>) -> rusqlite::Result<Dismissal> {
    let id: i64 = row.get("id")?;
    let hash_hex: String = row.get("finding_identity_hash")?;
    let title: String = row.get("title_snapshot")?;
    let body: Option<String> = row.get("body_snapshot")?;
    let source_type: String = row.get("source_type_snapshot")?;
    let models_json: String = row.get("models_snapshot")?;
    let branch: String = row.get("branch_snapshot")?;
    let reason: String = row.get("reason")?;
    let note: Option<String> = row.get("note")?;
    let dismissed_at: String = row.get("dismissed_at")?;
    let last_seen_at: String = row.get("last_seen_at")?;
    let last_seen_session_id: Option<String> = row.get("last_seen_session_id")?;
    let recurrence_count: i64 = row.get("recurrence_count")?;
    let expires_at: Option<String> = row.get("expires_at")?;
    let repo_head_sha_first: String = row.get("repo_head_sha_first")?;
    let promotion_state: String = row.get("promotion_state")?;

    let hash = FindingIdentityHash::from_hex(&hash_hex).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "bad hex hash",
            )),
        )
    })?;
    let models: Vec<String> = serde_json::from_str(&models_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let reason = DismissalReason::from_db_str(&reason).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "bad reason",
            )),
        )
    })?;
    let promotion_state = match promotion_state.as_str() {
        "candidate" => PromotionState::Candidate,
        "local_only" => PromotionState::LocalOnly,
        "promoted_convention" => PromotionState::PromotedConvention,
        _ => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "bad promotion_state",
                )),
            ))
        }
    };
    let parse_dt = |s: &str| {
        time::OffsetDateTime::parse(s, &Rfc3339).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })
    };
    let dismissed_at = parse_dt(&dismissed_at)?;
    let last_seen_at = parse_dt(&last_seen_at)?;
    let expires_at = expires_at.as_deref().map(parse_dt).transpose()?;

    Ok(Dismissal {
        id: DismissalId(id),
        finding_identity_hash: hash,
        title_snapshot: title,
        body_snapshot: body,
        source_type_snapshot: source_type,
        models_snapshot: models,
        branch_snapshot: branch,
        reason,
        note,
        dismissed_at,
        last_seen_at,
        last_seen_session_id,
        recurrence_count: recurrence_count.max(0) as u32,
        expires_at,
        repo_head_sha_first,
        promotion_state,
    })
}

const SELECT_COLUMNS: &str = "
    id, finding_identity_hash, title_snapshot, body_snapshot,
    source_type_snapshot, models_snapshot, branch_snapshot, reason, note,
    dismissed_at, last_seen_at, last_seen_session_id, recurrence_count,
    expires_at, repo_head_sha_first, promotion_state
";

impl MemoryStore for LocalSqliteMemoryStore {
    fn load_active_dismissals(
        &self,
    ) -> Result<HashMap<FindingIdentityHash, Dismissal>, MemoryError> {
        let conn = self.conn.lock().unwrap();
        let now = now_rfc3339()?;
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM dismissals WHERE expires_at IS NULL OR expires_at > ?1"
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| MemoryError::Backend(Box::new(e)))?;
        let rows = stmt
            .query_map([&now], row_to_dismissal)
            .map_err(|e| MemoryError::Backend(Box::new(e)))?;
        let mut out = HashMap::new();
        for r in rows {
            let d = r.map_err(|e| MemoryError::Backend(Box::new(e)))?;
            out.insert(d.finding_identity_hash, d);
        }
        Ok(out)
    }

    fn dismiss(
        &self,
        finding: &Finding,
        repo_head_sha: &str,
        branch: &str,
        reason: DismissalReason,
        note: Option<String>,
        expires_in: Option<time::Duration>,
    ) -> Result<DismissalId, MemoryError> {
        validate_note(reason, note.as_deref())?;

        let hash = super::identity::finding_identity_hash(finding).to_hex();
        let body_snap = if finding.body.is_empty() {
            None
        } else {
            Some(truncate_at_codepoint_boundary(&finding.body, BODY_SNAPSHOT_MAX_BYTES).to_string())
        };
        let models_json = serde_json::to_string(&finding.supported_by)
            .map_err(|e| MemoryError::Backend(Box::new(e)))?;
        let source_type = match finding.source {
            crate::review::FindingSource::Divergence => "divergence",
            crate::review::FindingSource::Agreement => "agreement",
            crate::review::FindingSource::Assumption => "assumption",
        };
        let now = time::OffsetDateTime::now_utc();
        let now_s = rfc3339(now)?;
        let expires_at_s = match expires_in {
            None => None,
            Some(d) => Some(rfc3339(now + d)?),
        };

        let conn = self.conn.lock().unwrap();
        let result = conn.execute(
            "INSERT INTO dismissals (
                finding_identity_hash, title_snapshot, body_snapshot,
                source_type_snapshot, models_snapshot, branch_snapshot, reason, note,
                dismissed_at, last_seen_at, recurrence_count,
                expires_at, repo_head_sha_first
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, 1, ?10, ?11)",
            params![
                hash,
                finding.title,
                body_snap,
                source_type,
                models_json,
                branch,
                reason.as_db_str(),
                note,
                now_s,
                expires_at_s,
                repo_head_sha,
            ],
        );
        match result {
            Ok(_) => Ok(DismissalId(conn.last_insert_rowid())),
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                // Was it the UNIQUE on finding_identity_hash specifically?
                let existing: Option<i64> = conn
                    .query_row(
                        "SELECT id FROM dismissals WHERE finding_identity_hash = ?1",
                        [&hash],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(|e| MemoryError::Backend(Box::new(e)))?;
                if existing.is_some() {
                    Err(MemoryError::AlreadyDismissed)
                } else {
                    Err(MemoryError::Backend(Box::new(
                        rusqlite::Error::SqliteFailure(e, None),
                    )))
                }
            }
            Err(e) => Err(MemoryError::Backend(Box::new(e))),
        }
    }

    fn record_seen(
        &self,
        hashes: &[FindingIdentityHash],
        review_session_id: &str,
        seen_at: time::OffsetDateTime,
    ) -> Result<(), MemoryError> {
        if hashes.is_empty() {
            return Ok(());
        }
        let seen_at_s = rfc3339(seen_at)?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction()
            .map_err(|e| MemoryError::Backend(Box::new(e)))?;
        {
            // For idempotency per (hash, session_id), the UPDATE only fires
            // when last_seen_session_id IS NULL OR != session_id. Subsequent
            // calls with the same session_id are silent no-ops.
            let hex_hashes: Vec<String> = hashes.iter().map(|h| h.to_hex()).collect();
            let placeholders: Vec<String> = (0..hex_hashes.len())
                .map(|i| format!("?{}", i + 3))
                .collect();
            let sql = format!(
                "UPDATE dismissals
                 SET recurrence_count = recurrence_count + 1,
                     last_seen_at = ?1,
                     last_seen_session_id = ?2
                 WHERE finding_identity_hash IN ({})
                   AND (last_seen_session_id IS NULL OR last_seen_session_id != ?2)",
                placeholders.join(",")
            );
            let mut param_vec: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(hex_hashes.len() + 2);
            param_vec.push(&seen_at_s);
            param_vec.push(&review_session_id);
            for h in &hex_hashes {
                param_vec.push(h);
            }
            tx.execute(&sql, params_from_iter(param_vec.iter().copied()))
                .map_err(|e| MemoryError::Backend(Box::new(e)))?;
        }
        tx.commit().map_err(|e| MemoryError::Backend(Box::new(e)))?;
        Ok(())
    }

    fn delete(&self, id: DismissalId) -> Result<bool, MemoryError> {
        let conn = self.conn.lock().unwrap();
        let n = conn
            .execute("DELETE FROM dismissals WHERE id = ?1", [id.0])
            .map_err(|e| MemoryError::Backend(Box::new(e)))?;
        Ok(n > 0)
    }

    fn list_all(&self) -> Result<Vec<Dismissal>, MemoryError> {
        let conn = self.conn.lock().unwrap();
        let sql = format!("SELECT {SELECT_COLUMNS} FROM dismissals ORDER BY id DESC");
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| MemoryError::Backend(Box::new(e)))?;
        let rows = stmt
            .query_map([], row_to_dismissal)
            .map_err(|e| MemoryError::Backend(Box::new(e)))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| MemoryError::Backend(Box::new(e)))?);
        }
        Ok(out)
    }

    fn get(&self, id: DismissalId) -> Result<Option<Dismissal>, MemoryError> {
        let conn = self.conn.lock().unwrap();
        let sql = format!("SELECT {SELECT_COLUMNS} FROM dismissals WHERE id = ?1");
        conn.query_row(&sql, [id.0], row_to_dismissal)
            .optional()
            .map_err(|e| MemoryError::Backend(Box::new(e)))
    }
}

// Keep `parse_rfc3339` reachable in case future code (Phase 1C) needs it
// outside `row_to_dismissal`. Currently only used internally.
#[allow(dead_code)]
fn _ensure_parse_rfc3339_usable(s: &str) -> Option<time::OffsetDateTime> {
    parse_rfc3339(s).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::{FindingSource, Severity};
    use tempfile::TempDir;

    fn sample_finding(title: &str) -> Finding {
        Finding {
            severity: Severity::High,
            title: title.to_string(),
            body: format!("Body for {title}. Confidence: 0.85. Supported by: alpha, beta."),
            source: FindingSource::Divergence,
            supported_by: vec!["alpha".into(), "beta".into()],
            confidence: Some(0.85),
        }
    }

    fn setup() -> (TempDir, LocalSqliteMemoryStore) {
        let td = TempDir::new().unwrap();
        let _ = git2::Repository::init(td.path()).unwrap();
        let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
        (td, store)
    }

    #[test]
    fn dismiss_then_load_active() {
        let (_td, store) = setup();
        let f = sample_finding("Race condition in cache");
        let id = store
            .dismiss(
                &f,
                "abc1234",
                "main",
                DismissalReason::WontFix,
                None,
                Some(time::Duration::days(365)),
            )
            .unwrap();
        assert!(id.0 > 0);
        let active = store.load_active_dismissals().unwrap();
        assert_eq!(active.len(), 1);
        let key = super::super::identity::finding_identity_hash(&f);
        let row = active.get(&key).unwrap();
        assert_eq!(row.reason, DismissalReason::WontFix);
        assert_eq!(row.recurrence_count, 1);
        assert_eq!(row.promotion_state, PromotionState::Candidate);
        assert!(row.expires_at.is_some());
    }

    #[test]
    fn dismiss_duplicate_returns_already_dismissed() {
        let (_td, store) = setup();
        let f = sample_finding("Same finding twice");
        store
            .dismiss(
                &f,
                "abc",
                "main",
                DismissalReason::FalsePositive,
                None,
                Some(time::Duration::days(365)),
            )
            .unwrap();
        let err = store
            .dismiss(
                &f,
                "abc",
                "main",
                DismissalReason::FalsePositive,
                None,
                Some(time::Duration::days(365)),
            )
            .unwrap_err();
        assert!(matches!(err, MemoryError::AlreadyDismissed));
    }

    #[test]
    fn permanent_dismissal_returned_by_load_active() {
        let (_td, store) = setup();
        let f = sample_finding("Permanent");
        store
            .dismiss(
                &f,
                "abc",
                "main",
                DismissalReason::Intentional,
                None,
                None, // permanent
            )
            .unwrap();
        let active = store.load_active_dismissals().unwrap();
        assert_eq!(active.len(), 1);
        let key = super::super::identity::finding_identity_hash(&f);
        let row = active.get(&key).unwrap();
        assert!(row.expires_at.is_none());
    }

    #[test]
    fn expired_dismissal_filtered_out() {
        let (_td, store) = setup();
        let f = sample_finding("Already expired");
        // 1 second window — likely expires before load_active fires, but
        // to be deterministic we manually shift by negative duration.
        store
            .dismiss(
                &f,
                "abc",
                "main",
                DismissalReason::FalsePositive,
                None,
                Some(time::Duration::seconds(-1)),
            )
            .unwrap();
        let active = store.load_active_dismissals().unwrap();
        assert!(active.is_empty(), "expired row must not appear");
    }

    #[test]
    fn record_seen_idempotent_per_session() {
        let (_td, store) = setup();
        let f = sample_finding("Repeated finding");
        store
            .dismiss(
                &f,
                "abc",
                "main",
                DismissalReason::FalsePositive,
                None,
                Some(time::Duration::days(365)),
            )
            .unwrap();
        let key = super::super::identity::finding_identity_hash(&f);
        let now = time::OffsetDateTime::now_utc();
        store.record_seen(&[key], "session-A", now).unwrap();
        store
            .record_seen(&[key], "session-A", now + time::Duration::seconds(1))
            .unwrap();
        // Same session should not double-bump.
        let active = store.load_active_dismissals().unwrap();
        let row = active.get(&key).unwrap();
        assert_eq!(row.recurrence_count, 2, "exactly one bump for session-A");
        // Different session bumps once more.
        store
            .record_seen(&[key], "session-B", now + time::Duration::seconds(2))
            .unwrap();
        let active = store.load_active_dismissals().unwrap();
        let row = active.get(&key).unwrap();
        assert_eq!(row.recurrence_count, 3, "session-B bumps once");
    }

    #[test]
    fn delete_is_idempotent_on_unknown() {
        let (_td, store) = setup();
        assert!(!store.delete(DismissalId(999_999)).unwrap());
        // Insert one, then delete it twice.
        let f = sample_finding("X");
        let id = store
            .dismiss(
                &f,
                "abc",
                "main",
                DismissalReason::FalsePositive,
                None,
                Some(time::Duration::days(365)),
            )
            .unwrap();
        assert!(store.delete(id).unwrap());
        assert!(!store.delete(id).unwrap());
    }

    #[test]
    fn body_snapshot_truncated_to_2kb() {
        let (_td, store) = setup();
        let big = "ä".repeat(3000); // 6000 bytes
        let f = Finding {
            body: big.clone(),
            ..sample_finding("BIG")
        };
        let id = store
            .dismiss(
                &f,
                "abc",
                "main",
                DismissalReason::FalsePositive,
                None,
                Some(time::Duration::days(365)),
            )
            .unwrap();
        let row = store.get(id).unwrap().unwrap();
        let body = row.body_snapshot.unwrap();
        assert!(
            body.len() <= BODY_SNAPSHOT_MAX_BYTES,
            "body_snapshot must be ≤ 2KB: got {}",
            body.len()
        );
        // Must be a prefix of the original.
        assert!(big.starts_with(&body));
    }

    #[test]
    fn other_without_note_rejected_at_trait_layer() {
        let (_td, store) = setup();
        let f = sample_finding("X");
        let err = store
            .dismiss(
                &f,
                "abc",
                "main",
                DismissalReason::Other,
                None,
                Some(time::Duration::days(365)),
            )
            .unwrap_err();
        assert!(matches!(err, MemoryError::OtherWithoutNote));
    }

    #[test]
    fn reopen_integrity_with_wal_sidecars() {
        // AC 118: simulate hard-kill by dropping the connection without
        // closing it explicitly. WAL/SHM sidecars remain. Next open
        // must read all committed rows.
        let td = TempDir::new().unwrap();
        let _ = git2::Repository::init(td.path()).unwrap();
        let f = sample_finding("preserved");
        let hash;
        {
            let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
            hash = super::super::identity::finding_identity_hash(&f);
            store
                .dismiss(
                    &f,
                    "abc",
                    "main",
                    DismissalReason::WontFix,
                    None,
                    Some(time::Duration::days(365)),
                )
                .unwrap();
            // store drops here; WAL may or may not flush
        }
        let store2 = LocalSqliteMemoryStore::new(td.path()).unwrap();
        let active = store2.load_active_dismissals().unwrap();
        assert!(
            active.contains_key(&hash),
            "reopen must surface the committed dismissal"
        );
    }
}
