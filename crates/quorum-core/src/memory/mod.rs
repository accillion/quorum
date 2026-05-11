//! Dismissals memory store — Phase 1B Stage 1.
//!
//! `MemoryStore` is the trait surface a backend implements. Phase 1B ships
//! one impl (`LocalSqliteMemoryStore`); Phase 2 layers an outbox on top of
//! the local store. The trait is **sync** by design — `rusqlite` is sync and
//! we want to avoid `spawn_blocking` plumbing in the review pipeline.
//!
//! Two operations, not one (P01 from v0.1 adjudication):
//!   - [`MemoryStore::dismiss`] is the user write path. INSERT-only; returns
//!     [`MemoryError::AlreadyDismissed`] on duplicate hash. Validates the
//!     `Other` reason / note pairing and the 2KB note ceiling.
//!   - [`MemoryStore::record_seen`] is the filter-side bulk update. Bumps
//!     `recurrence_count` + `last_seen_at` exactly once per (hash,
//!     review_session_id) pair.
//!
//! Conflating the two produced v0.1's upsert-vs-UNIQUE bug.

pub mod gitignore;
pub mod identity;
pub mod schema;
pub mod sqlite;

use std::collections::HashMap;

use crate::review::Finding;

pub use identity::{finding_identity_hash, FindingIdentityHash};
pub use sqlite::LocalSqliteMemoryStore;

/// Type returned by `dismiss` so callers can address the row later (TUI
/// undo, `quorum dismissals remove <id>`). Wraps the SQLite rowid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct DismissalId(pub i64);

impl std::fmt::Display for DismissalId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for DismissalId {
    type Err = std::num::ParseIntError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<i64>().map(DismissalId)
    }
}

/// User-supplied reason for dismissing a finding. Stored as a lowercase
/// snake_case string; the SQLite CHECK constraint enforces the exact set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DismissalReason {
    FalsePositive,
    Intentional,
    OutOfScope,
    WontFix,
    Other,
}

impl DismissalReason {
    /// String form written to / read from SQLite. Must round-trip with
    /// [`DismissalReason::from_db_str`].
    pub fn as_db_str(self) -> &'static str {
        match self {
            DismissalReason::FalsePositive => "false_positive",
            DismissalReason::Intentional => "intentional",
            DismissalReason::OutOfScope => "out_of_scope",
            DismissalReason::WontFix => "wont_fix",
            DismissalReason::Other => "other",
        }
    }

    pub fn from_db_str(s: &str) -> Option<DismissalReason> {
        match s {
            "false_positive" => Some(DismissalReason::FalsePositive),
            "intentional" => Some(DismissalReason::Intentional),
            "out_of_scope" => Some(DismissalReason::OutOfScope),
            "wont_fix" => Some(DismissalReason::WontFix),
            "other" => Some(DismissalReason::Other),
            _ => None,
        }
    }
}

/// Phase 1B writes only [`PromotionState::Candidate`]. Phase 1C adds the
/// state machine that transitions through [`PromotionState::LocalOnly`] →
/// [`PromotionState::PromotedConvention`] based on `recurrence_count`
/// thresholds + user approval. The variants exist now so the schema is
/// forward-compatible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PromotionState {
    Candidate,
    LocalOnly,
    PromotedConvention,
}

impl PromotionState {
    pub fn as_db_str(self) -> &'static str {
        match self {
            PromotionState::Candidate => "candidate",
            PromotionState::LocalOnly => "local_only",
            PromotionState::PromotedConvention => "promoted_convention",
        }
    }
}

/// One row of the dismissals table, decoded back into a Rust value.
/// Internal-only — archive serialization projects to a separate
/// `SuppressionSummary` so we don't drag `time::OffsetDateTime` through
/// serde here.
#[derive(Debug, Clone)]
pub struct Dismissal {
    pub id: DismissalId,
    pub finding_identity_hash: FindingIdentityHash,
    pub title_snapshot: String,
    pub body_snapshot: Option<String>,
    pub source_type_snapshot: String,
    pub models_snapshot: Vec<String>,
    pub branch_snapshot: String,
    pub reason: DismissalReason,
    pub note: Option<String>,
    pub dismissed_at: time::OffsetDateTime,
    pub last_seen_at: time::OffsetDateTime,
    pub last_seen_session_id: Option<String>,
    pub recurrence_count: u32,
    pub expires_at: Option<time::OffsetDateTime>,
    pub repo_head_sha_first: String,
    pub promotion_state: PromotionState,
}

/// Maximum bytes stored in the `body_snapshot` column. The §4.2.4 hash
/// input is a prefix of this window (currently dropped per adjudication
/// D2/D3; the column remains for Phase 1C convention seeding).
pub const BODY_SNAPSHOT_MAX_BYTES: usize = 2048;

/// Maximum bytes accepted in a free-text `note` value. Enforced by the
/// trait via [`MemoryError::InvalidNote`] before insert.
pub const NOTE_MAX_BYTES: usize = 2048;

/// Errors from the memory store. `Backend` boxes the underlying error to
/// keep the trait surface free of `rusqlite` types — Phase 2's outbox will
/// wrap `reqwest::Error` through the same variant.
#[derive(thiserror::Error, Debug)]
pub enum MemoryError {
    #[error("storage backend: {0}")]
    Backend(Box<dyn std::error::Error + Send + Sync>),
    #[error("schema migration failed: {0}")]
    Migration(String),
    #[error("dismissal hash already exists")]
    AlreadyDismissed,
    #[error("Other reason requires a non-empty note")]
    OtherWithoutNote,
    #[error("note exceeds 2KB or contains forbidden characters")]
    InvalidNote,
}

/// The dismissals store contract. Phase 1B has one implementor
/// ([`LocalSqliteMemoryStore`]); Phase 2 layers an async outbox on top of
/// it but the trait stays sync.
pub trait MemoryStore {
    /// Returns active (non-expired) dismissals keyed by identity hash.
    /// Called once per `quorum review` invocation before the filter site.
    /// Permanent (`expires_at IS NULL`) dismissals are always included.
    fn load_active_dismissals(
        &self,
    ) -> Result<HashMap<FindingIdentityHash, Dismissal>, MemoryError>;

    /// Records a NEW dismissal. INSERT-only; duplicate hash returns
    /// [`MemoryError::AlreadyDismissed`]. The impl truncates
    /// `finding.body` to [`BODY_SNAPSHOT_MAX_BYTES`] at a UTF-8 codepoint
    /// boundary before storing as `body_snapshot` (P29) — caller passes
    /// the full `&Finding`.
    ///
    /// `expires_in = None` writes `expires_at = NULL` (permanent).
    /// `expires_in = Some(d)` writes `expires_at = now + d`.
    fn dismiss(
        &self,
        finding: &Finding,
        repo_head_sha: &str,
        branch: &str,
        reason: DismissalReason,
        note: Option<String>,
        expires_in: Option<time::Duration>,
    ) -> Result<DismissalId, MemoryError>;

    /// Bulk increments `recurrence_count` and updates `last_seen_at` for
    /// the supplied hashes. Idempotent per `(hash, review_session_id)`
    /// pair — back-to-back calls with the same session id do not
    /// double-bump within a single process. Cross-process race is a
    /// documented residual risk (§4.2.2 P30).
    fn record_seen(
        &self,
        hashes: &[FindingIdentityHash],
        review_session_id: &str,
        seen_at: time::OffsetDateTime,
    ) -> Result<(), MemoryError>;

    /// Permanent removal by id. Returns `Ok(false)` if the id is not
    /// present; the caller (TUI undo, `quorum dismissals remove`) is
    /// responsible for id provenance — the storage layer does not enforce
    /// session affinity.
    fn delete(&self, id: DismissalId) -> Result<bool, MemoryError>;

    fn list_all(&self) -> Result<Vec<Dismissal>, MemoryError>;
    fn get(&self, id: DismissalId) -> Result<Option<Dismissal>, MemoryError>;
}

/// Trait-layer validation of a free-text note. Returns `()` if the note
/// (if present) is acceptable; this is also reused by the TUI's
/// pre-submit validator so users see the error before they hit Enter.
pub fn validate_note(reason: DismissalReason, note: Option<&str>) -> Result<(), MemoryError> {
    match (reason, note) {
        (DismissalReason::Other, None) => Err(MemoryError::OtherWithoutNote),
        (DismissalReason::Other, Some(s)) if s.trim().is_empty() => {
            Err(MemoryError::OtherWithoutNote)
        }
        (_, Some(s)) if s.len() > NOTE_MAX_BYTES => Err(MemoryError::InvalidNote),
        (_, Some(s)) if s.contains('\n') || s.contains('\r') => Err(MemoryError::InvalidNote),
        (_, Some(s)) if s.chars().any(|c| (c as u32) < 0x20 && c != '\t') => {
            Err(MemoryError::InvalidNote)
        }
        _ => Ok(()),
    }
}

/// Truncate `s` to at most `max_bytes` at a UTF-8 codepoint boundary.
/// Public so [`sqlite::LocalSqliteMemoryStore`] can use it and tests can
/// assert on the boundary behavior directly.
pub fn truncate_at_codepoint_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut cut = max_bytes;
    let bytes = s.as_bytes();
    while cut > 0 && (bytes[cut] & 0xC0) == 0x80 {
        cut -= 1;
    }
    // SAFETY: cut is at a UTF-8 codepoint boundary by construction.
    std::str::from_utf8(&bytes[..cut]).expect("codepoint boundary")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_round_trip() {
        for r in [
            DismissalReason::FalsePositive,
            DismissalReason::Intentional,
            DismissalReason::OutOfScope,
            DismissalReason::WontFix,
            DismissalReason::Other,
        ] {
            assert_eq!(DismissalReason::from_db_str(r.as_db_str()), Some(r));
        }
        assert_eq!(DismissalReason::from_db_str("bogus"), None);
    }

    #[test]
    fn validate_note_rules() {
        assert!(matches!(
            validate_note(DismissalReason::Other, None),
            Err(MemoryError::OtherWithoutNote)
        ));
        assert!(matches!(
            validate_note(DismissalReason::Other, Some("   ")),
            Err(MemoryError::OtherWithoutNote)
        ));
        assert!(validate_note(DismissalReason::Other, Some("legitimate note")).is_ok());
        assert!(matches!(
            validate_note(DismissalReason::FalsePositive, Some("ok\nbad")),
            Err(MemoryError::InvalidNote)
        ));
        let too_big = "x".repeat(NOTE_MAX_BYTES + 1);
        assert!(matches!(
            validate_note(DismissalReason::FalsePositive, Some(&too_big)),
            Err(MemoryError::InvalidNote)
        ));
        assert!(matches!(
            validate_note(DismissalReason::FalsePositive, Some("ok\u{0007}bel")),
            Err(MemoryError::InvalidNote)
        ));
        assert!(validate_note(
            DismissalReason::FalsePositive,
            Some("tabs\tok and unicode café")
        )
        .is_ok());
        assert!(validate_note(DismissalReason::FalsePositive, None).is_ok());
    }

    #[test]
    fn truncate_at_codepoint_boundary_basic() {
        assert_eq!(truncate_at_codepoint_boundary("hello", 100), "hello");
        assert_eq!(truncate_at_codepoint_boundary("hello", 3), "hel");
        // "café" — 'é' is two bytes (C3 A9). Cut at 4 falls inside it,
        // must back off to 3.
        let s = "café";
        assert_eq!(s.len(), 5);
        assert_eq!(truncate_at_codepoint_boundary(s, 4), "caf");
        assert_eq!(truncate_at_codepoint_boundary(s, 5), "café");
    }
}
