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
/// thresholds + user approval.
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

    pub fn from_db_str(s: &str) -> Option<PromotionState> {
        match s {
            "candidate" => Some(PromotionState::Candidate),
            "local_only" => Some(PromotionState::LocalOnly),
            "promoted_convention" => Some(PromotionState::PromotedConvention),
            _ => None,
        }
    }
}

/// Phase 1C: the `trigger` enum on `state_transitions.trigger`. T4 (prune)
/// and T5 (undismiss) are audit-trail-silent and do not appear here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TransitionTrigger {
    /// T1 — `candidate` → `local_only`, fired inside `record_seen()` when
    /// `recurrence_count` reaches `candidate_threshold`.
    AutoRecurrence,
    /// T2 — `local_only` → `promoted_convention`, fired by
    /// `quorum convention promote` / TUI `p`. (Stage 4 wires up the write
    /// site; the variant ships in Stage 1 so the audit-row CHECK enum is
    /// covered.)
    ExplicitPromote,
    /// T3 — `promoted_convention` → `local_only`, fired by
    /// `quorum convention demote`. (Stage 4 — same as above.)
    ExplicitDemote,
}

impl TransitionTrigger {
    pub fn as_db_str(self) -> &'static str {
        match self {
            TransitionTrigger::AutoRecurrence => "auto_recurrence",
            TransitionTrigger::ExplicitPromote => "explicit_promote",
            TransitionTrigger::ExplicitDemote => "explicit_demote",
        }
    }

    pub fn from_db_str(s: &str) -> Option<TransitionTrigger> {
        match s {
            "auto_recurrence" => Some(TransitionTrigger::AutoRecurrence),
            "explicit_promote" => Some(TransitionTrigger::ExplicitPromote),
            "explicit_demote" => Some(TransitionTrigger::ExplicitDemote),
            _ => None,
        }
    }
}

/// Phase 1C — one row of the `state_transitions` table, read back for the
/// `quorum convention show` / `history` CLI surface. Differs from
/// [`TransitionEvent`] (which is the in-flight event emitted by
/// `record_seen`): `StateTransitionRow` is what a reader sees after the
/// audit row was committed and may be `None` on the nullable columns
/// per §4.2 schema (`by_review_session_id`, `recurrence_at_transition`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateTransitionRow {
    pub from_state: PromotionState,
    pub to_state: PromotionState,
    pub trigger: TransitionTrigger,
    /// Unix epoch milliseconds.
    pub ts_ms: i64,
    pub by_review_session_id: Option<String>,
    pub recurrence_at_transition: Option<u32>,
}

/// Outcome of [`MemoryStore::find_by_short_hash`]. The CLI surface (Stage 3
/// `convention show` / `history`) maps these to exit codes + user-facing
/// errors; the storage layer just reports what it saw.
///
/// `Exact` carries a boxed `Dismissal` so the enum stays narrow — the
/// variant size disparity with `NotFound` / `Ambiguous(Vec<…>)` would
/// otherwise trip `clippy::large_enum_variant`.
#[derive(Debug, Clone)]
pub enum ShortHashResolution {
    /// Exactly one dismissal matches the supplied prefix.
    Exact(Box<Dismissal>),
    /// More than one dismissal matches; caller must show the
    /// disambiguation list.
    Ambiguous(Vec<Dismissal>),
    /// No dismissal matched the prefix.
    NotFound,
}

/// Phase 1C — an audited state transition. Returned by
/// [`MemoryStore::record_seen`] alongside its existing return shape so the
/// CLI binary can emit a stderr informational note (§5.3 returned-event
/// pattern). Owned + `Clone` so the CLI can format-and-emit without
/// lifetime entanglement with the SQLite transaction that produced it.
///
/// One event per fired transition; `Vec` is empty in the common case.
/// `short_hash` is the 12-char hex prefix of the identity hash, suitable
/// for direct interpolation into the user-visible message text from
/// §3.2 T1.
#[derive(Debug, Clone)]
pub struct TransitionEvent {
    pub finding_identity_hash: FindingIdentityHash,
    pub short_hash: String,
    pub from_state: PromotionState,
    pub to_state: PromotionState,
    pub trigger: TransitionTrigger,
    pub recurrence_at_transition: u32,
    pub ts_ms: i64,
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
    /// AC 174: the on-disk SQLite is from a newer Quorum binary; we refuse
    /// to read or write it. `schema_version` is the version stamped on the
    /// last migration; `forward_compat_min` is the binary version floor
    /// that DB declares it requires.
    #[error(
        ".quorum/dismissals.sqlite is from a newer Quorum (schema={schema_version}, \
         forward_compat_min={forward_compat_min}); upgrade your binary"
    )]
    SchemaTooNew {
        schema_version: i64,
        forward_compat_min: i64,
    },
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
    ///
    /// Phase 1C: returns a `Vec<TransitionEvent>` describing any state
    /// transitions that fired inside this call (T1
    /// `candidate → local_only` when `recurrence_count` crosses
    /// `candidate_threshold`). The CLI consumes this value to emit the
    /// §5.3 stderr informational note. Empty vec is the common case.
    fn record_seen(
        &self,
        hashes: &[FindingIdentityHash],
        review_session_id: &str,
        seen_at: time::OffsetDateTime,
        candidate_threshold: u32,
    ) -> Result<Vec<TransitionEvent>, MemoryError>;

    /// Permanent removal by id. Returns `Ok(false)` if the id is not
    /// present; the caller (TUI undo, `quorum dismissals remove`) is
    /// responsible for id provenance — the storage layer does not enforce
    /// session affinity.
    fn delete(&self, id: DismissalId) -> Result<bool, MemoryError>;

    fn list_all(&self) -> Result<Vec<Dismissal>, MemoryError>;
    fn get(&self, id: DismissalId) -> Result<Option<Dismissal>, MemoryError>;

    /// Phase 1C — read surface for the bundle-assembly stage (Stage 2).
    /// Returns the rows in state `local_only` plus any
    /// `promoted_convention` rows that will be rendered into the memory
    /// section via the §6.2 bridge (Stage 2 makes the bridge decision
    /// per-row at render time; this surface returns the raw candidates).
    ///
    /// Sorted by `recurrence_count DESC, last_seen_at DESC` (spec §6.1)
    /// so the bundle assembler doesn't have to re-sort.
    fn load_local_only_conventions(&self) -> Result<Vec<Dismissal>, MemoryError>;

    /// Phase 1C — read surface for the `quorum convention list` CLI.
    /// `state == None` returns every row across all three states.
    /// `state == Some(s)` filters to one state. Sort order matches
    /// [`MemoryStore::load_local_only_conventions`]: recurrence_count DESC,
    /// last_seen_at DESC.
    fn list_by_state(&self, state: Option<PromotionState>) -> Result<Vec<Dismissal>, MemoryError>;

    /// Phase 1C — resolve a hex `finding_identity_hash` prefix to a single
    /// row, an ambiguous match set, or not-found. Callers (`show`,
    /// `history`) must enforce the ≥ 8-char minimum BEFORE calling — the
    /// store treats any non-hex / too-short prefix as a precondition error
    /// and returns it as a `MemoryError::Backend`. A full 64-hex prefix
    /// always resolves as `Exact` or `NotFound`.
    fn find_by_short_hash(&self, prefix: &str) -> Result<ShortHashResolution, MemoryError>;

    /// Phase 1C — read the `state_transitions` audit log for one hash,
    /// oldest-first (ts ASC). Returns an empty vec for pre-v2 dismissals
    /// (no backfill — spec §4.1). Also empty for an unknown hash.
    fn load_transitions(
        &self,
        hash: &FindingIdentityHash,
    ) -> Result<Vec<StateTransitionRow>, MemoryError>;

    /// Phase 1C — every row from the `conventions` table joined with its
    /// `dismissals.title_snapshot`. Used by orphan detection. Sort: stable
    /// by `conventions_md_block_id` ASC so callers see deterministic
    /// output.
    fn list_conventions(&self) -> Result<Vec<crate::conventions::ConventionRow>, MemoryError>;

    /// Phase 1C Stage 4 — SQLite-side of T2 (`local_only →
    /// promoted_convention`). Called by the CLI / TUI promote orchestrator
    /// AFTER the conventions.md atomic rename succeeded (B5 ordering;
    /// spec §3.2 T2 step 3).
    ///
    /// Runs a single transaction:
    ///   1. `UPDATE dismissals SET promotion_state='promoted_convention'
    ///      WHERE finding_identity_hash=? AND promotion_state='local_only'`
    ///   2. `INSERT INTO conventions (...)`
    ///   3. `INSERT INTO state_transitions (..., 'explicit_promote')`
    ///   4. COMMIT.
    ///
    /// If step 1's `rows_affected == 0` (another writer beat us; state has
    /// drifted), the transaction is rolled back and [`PromoteOutcome::StateDrifted`]
    /// is returned — caller surfaces this to the user (file is now slightly
    /// ahead of SQLite; `quorum convention list --orphans` reconciles).
    fn commit_promote(
        &self,
        hash: &FindingIdentityHash,
        convention_text: &str,
        conventions_md_block_id: &str,
        ts_ms: i64,
    ) -> Result<PromoteOutcome, MemoryError>;

    /// Phase 1C Stage 4 — SQLite-side of T3 (`promoted_convention →
    /// local_only`). Caller has already removed the managed block from
    /// conventions.md (or skipped the write per Q7 missing-file lean).
    ///
    /// Single transaction:
    ///   1. `UPDATE dismissals SET promotion_state='local_only'
    ///      WHERE finding_identity_hash=? AND promotion_state='promoted_convention'`
    ///   2. `DELETE FROM conventions WHERE finding_identity_hash=?`
    ///   3. `INSERT INTO state_transitions (..., 'explicit_demote')`
    ///   4. COMMIT.
    ///
    /// `rows_affected == 0` on step 1 → [`DemoteOutcome::StateDrifted`].
    fn commit_demote(
        &self,
        hash: &FindingIdentityHash,
        ts_ms: i64,
    ) -> Result<DemoteOutcome, MemoryError>;

    /// Phase 1C Stage 4 — T4 prune: DELETE candidate dismissals whose
    /// `last_seen_at < older_than`. `state_transitions` rows for the
    /// deleted hashes are cascade-dropped by the FK. Audit-trail-silent
    /// (spec §3.2 T4 / §4.6). Returns the count of rows deleted. Promoted
    /// or local_only rows are never touched (filter is by query
    /// construction — AC 142).
    fn prune_candidates(&self, older_than: time::OffsetDateTime) -> Result<u64, MemoryError>;
}

/// Outcome of [`MemoryStore::commit_promote`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromoteOutcome {
    /// The UPDATE flipped one row; conventions + audit rows were inserted;
    /// transaction committed.
    Committed,
    /// The row's `promotion_state` was not `local_only` at UPDATE time
    /// (concurrent writer beat us OR state had drifted). Transaction was
    /// rolled back; file remains ahead of SQLite — orphan detection
    /// reconciles on next run.
    StateDrifted,
}

/// Outcome of [`MemoryStore::commit_demote`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoteOutcome {
    Committed,
    StateDrifted,
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
