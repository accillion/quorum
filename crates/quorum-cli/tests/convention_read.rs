//! Phase 1C Stage 3 — `quorum convention` read-surface integration.
//!
//! Drives `list`, `show`, `history` end-to-end through a tempdir-rooted
//! quorum dir. The fixture inserts rows directly via SQLite (not
//! `MemoryStore::dismiss`) where needed so we can pin specific states,
//! recurrence counts, and audit-log entries without writing through the
//! full Phase 1B + 1C state machine.
//!
//! ACs covered (Stage 3):
//!   - AC 162 (`list` filtering + `--json`)
//!   - AC 163 (short-hash resolution: short prefix / non-hex / not-found /
//!     ambiguous / full 64-hex)
//!   - AC 168 partial (stderr-warning surface on malformed managed blocks
//!     under `list --orphans`)
//!   - AC 169 (orphan detection both directions)

use assert_cmd::Command;
use predicates::prelude::*;
use quorum_core::memory::identity::finding_identity_hash;
use quorum_core::memory::{
    DismissalReason, FindingIdentityHash, LocalSqliteMemoryStore, MemoryStore, PromotionState,
};
use quorum_core::review::{Finding, FindingSource, Severity};
use tempfile::TempDir;

fn quorum() -> Command {
    Command::cargo_bin("quorum").expect("quorum binary built")
}

fn init_repo() -> TempDir {
    let td = TempDir::new().unwrap();
    git2::Repository::init(td.path()).unwrap();
    td
}

fn sample_finding(title: &str, models: &[&str]) -> Finding {
    Finding {
        severity: Severity::High,
        title: title.to_string(),
        body: format!("Body for {title}. Supported: {}", models.join(",")),
        source: FindingSource::Divergence,
        supported_by: models.iter().map(|s| s.to_string()).collect(),
        confidence: Some(0.9),
    }
}

fn dismiss_then_force_state(
    store: &LocalSqliteMemoryStore,
    title: &str,
    state: PromotionState,
) -> FindingIdentityHash {
    let f = sample_finding(title, &["m"]);
    store
        .dismiss(
            &f,
            "head-sha",
            "main",
            DismissalReason::FalsePositive,
            None,
            Some(time::Duration::days(365)),
        )
        .unwrap();
    let h = finding_identity_hash(&f);
    if state != PromotionState::Candidate {
        let conn = rusqlite::Connection::open(store.path()).unwrap();
        conn.execute(
            "UPDATE dismissals SET promotion_state = ?1 WHERE finding_identity_hash = ?2",
            rusqlite::params![state.as_db_str(), h.to_hex()],
        )
        .unwrap();
    }
    h
}

fn insert_convention_row(
    store: &LocalSqliteMemoryStore,
    hash: &FindingIdentityHash,
    block_id: &str,
    convention_text: &str,
) {
    let conn = rusqlite::Connection::open(store.path()).unwrap();
    conn.execute(
        "INSERT INTO conventions (finding_identity_hash, convention_text, promoted_at, conventions_md_block_id)
         VALUES (?1, ?2, 1000, ?3)",
        rusqlite::params![hash.to_hex(), convention_text, block_id],
    )
    .unwrap();
}

fn insert_transition(
    store: &LocalSqliteMemoryStore,
    hash: &FindingIdentityHash,
    from: &str,
    to: &str,
    trigger: &str,
    ts_ms: i64,
    recurrence: Option<i64>,
) {
    let conn = rusqlite::Connection::open(store.path()).unwrap();
    conn.execute(
        "INSERT INTO state_transitions
            (finding_identity_hash, from_state, to_state, trigger, ts,
             by_review_session_id, recurrence_at_transition)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6)",
        rusqlite::params![hash.to_hex(), from, to, trigger, ts_ms, recurrence],
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// Help-text surface (Stage 3 scope discipline).
// ---------------------------------------------------------------------------

#[test]
fn convention_help_lists_all_subcommands() {
    // Stage 4 adds promote/demote/prune to the clap surface; Stage 3
    // read surface (list/show/history) remains.
    let out = quorum().args(["convention", "--help"]).assert().success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("list"), "help must mention list");
    assert!(stdout.contains("show"), "help must mention show");
    assert!(stdout.contains("history"), "help must mention history");
    assert!(stdout.contains("promote"), "help must mention promote");
    assert!(stdout.contains("demote"), "help must mention demote");
    assert!(stdout.contains("prune"), "help must mention prune");
}

// ---------------------------------------------------------------------------
// `list` (AC 162) + cross-validation with bundle render surface.
// ---------------------------------------------------------------------------

#[test]
fn list_with_no_flags_prints_all_states_sorted() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h_a = dismiss_then_force_state(&store, "alpha-cand", PromotionState::Candidate);
    let _h_b = dismiss_then_force_state(&store, "bravo-local", PromotionState::LocalOnly);
    let _h_c = dismiss_then_force_state(&store, "charlie-prom", PromotionState::PromotedConvention);
    // Bump alpha's recurrence so it sorts first regardless of state.
    {
        let conn = rusqlite::Connection::open(store.path()).unwrap();
        conn.execute(
            "UPDATE dismissals SET recurrence_count = 9 WHERE finding_identity_hash = ?1",
            [h_a.to_hex()],
        )
        .unwrap();
    }

    let out = quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "list",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    // All three rows present.
    assert!(stdout.contains("alpha-cand"));
    assert!(stdout.contains("bravo-local"));
    assert!(stdout.contains("charlie-prom"));
    // Header row.
    assert!(stdout.contains("HASH"));
    assert!(stdout.contains("STATE"));
    // alpha (recurrence=9) appears before charlie (recurrence=1).
    let idx_a = stdout.find("alpha-cand").unwrap();
    let idx_c = stdout.find("charlie-prom").unwrap();
    assert!(idx_a < idx_c, "highest-recurrence row first");
}

#[test]
fn list_with_state_filters() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    dismiss_then_force_state(&store, "cand-row", PromotionState::Candidate);
    dismiss_then_force_state(&store, "local-row", PromotionState::LocalOnly);

    let out = quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "list",
            "--state",
            "local_only",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("local-row"));
    assert!(!stdout.contains("cand-row"));
}

#[test]
fn list_with_bad_state_exits_2() {
    let td = init_repo();
    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "list",
            "--state",
            "bogus",
        ])
        .assert()
        .failure()
        .code(predicate::eq(2))
        .stderr(predicate::str::contains("unknown --state"));
}

#[test]
fn list_json_emits_parseable_array() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = dismiss_then_force_state(&store, "json-row", PromotionState::LocalOnly);

    let out = quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "list",
            "--json",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let arr: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    let arr = arr.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    let row = &arr[0];
    assert_eq!(row["title"], "json-row");
    assert_eq!(row["state"], "local_only");
    assert_eq!(row["short_hash"], &h.to_hex()[..12]);
    assert_eq!(row["hash"], h.to_hex());
    assert!(row["dismissed_at"].is_string());
    assert!(row["last_seen_at"].is_string());
}

#[test]
fn list_json_ordering_matches_default_text() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h_low = dismiss_then_force_state(&store, "low-rec", PromotionState::Candidate);
    let h_high = dismiss_then_force_state(&store, "high-rec", PromotionState::Candidate);
    {
        let conn = rusqlite::Connection::open(store.path()).unwrap();
        conn.execute(
            "UPDATE dismissals SET recurrence_count = 9 WHERE finding_identity_hash = ?1",
            [h_high.to_hex()],
        )
        .unwrap();
        conn.execute(
            "UPDATE dismissals SET recurrence_count = 2 WHERE finding_identity_hash = ?1",
            [h_low.to_hex()],
        )
        .unwrap();
    }
    let out = quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "list",
            "--json",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let arr: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    let arr = arr.as_array().unwrap();
    assert_eq!(arr[0]["title"], "high-rec");
    assert_eq!(arr[1]["title"], "low-rec");
}

#[test]
fn list_cross_validates_with_bundle_load_local_only() {
    // The same `Dismissal` rows that the bundle (`load_local_only_conventions`)
    // sees must surface in `list --state local_only` with matching hash +
    // title + recurrence. This protects against silent divergence between
    // the bundle render path (Stage 2) and the CLI list surface (Stage 3).
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h1 = dismiss_then_force_state(&store, "lc-row-1", PromotionState::LocalOnly);
    let _ = dismiss_then_force_state(&store, "cand-row", PromotionState::Candidate);

    let bundle_rows = store.load_local_only_conventions().unwrap();
    assert_eq!(bundle_rows.len(), 1);
    assert_eq!(bundle_rows[0].finding_identity_hash, h1);

    let out = quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "list",
            "--state",
            "local_only",
            "--json",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let arr: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    let arr = arr.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["hash"], h1.to_hex());
    assert_eq!(arr[0]["title"], "lc-row-1");
}

// ---------------------------------------------------------------------------
// Short-hash resolution (AC 163).
// ---------------------------------------------------------------------------

#[test]
fn show_rejects_short_hash_prefix() {
    let td = init_repo();
    let _ = LocalSqliteMemoryStore::new(td.path()).unwrap();
    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "show",
            "abcd",
        ])
        .assert()
        .failure()
        .code(predicate::eq(2))
        .stderr(predicate::str::contains("≥ 8 hex chars"));
}

#[test]
fn show_rejects_nonhex_prefix() {
    let td = init_repo();
    let _ = LocalSqliteMemoryStore::new(td.path()).unwrap();
    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "show",
            "ZZZZZZZZ",
        ])
        .assert()
        .failure()
        .code(predicate::eq(2))
        .stderr(predicate::str::contains("lowercase hex"));
}

#[test]
fn show_not_found_exits_2() {
    let td = init_repo();
    let _ = LocalSqliteMemoryStore::new(td.path()).unwrap();
    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "show",
            "00000000",
        ])
        .assert()
        .failure()
        .code(predicate::eq(2))
        .stderr(predicate::str::contains("no dismissal matches"));
}

#[test]
fn show_ambiguous_exits_2_with_disambiguation_list() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h1 = dismiss_then_force_state(&store, "amb-1", PromotionState::Candidate);
    let h2 = dismiss_then_force_state(&store, "amb-2", PromotionState::Candidate);
    // Patch hashes to share a known 8-hex prefix.
    let prefix = "feedface";
    let full1 = format!("{prefix}{}", &h1.to_hex()[8..]);
    let full2 = format!("{prefix}{}", &h2.to_hex()[8..]);
    {
        let conn = rusqlite::Connection::open(store.path()).unwrap();
        conn.execute(
            "UPDATE dismissals SET finding_identity_hash = ?1 WHERE title_snapshot = 'amb-1'",
            [&full1],
        )
        .unwrap();
        conn.execute(
            "UPDATE dismissals SET finding_identity_hash = ?1 WHERE title_snapshot = 'amb-2'",
            [&full2],
        )
        .unwrap();
    }
    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "show",
            prefix,
        ])
        .assert()
        .failure()
        .code(predicate::eq(2))
        .stderr(predicate::str::contains("is ambiguous"))
        .stderr(predicate::str::contains("amb-1"))
        .stderr(predicate::str::contains("amb-2"));
}

#[test]
fn show_full_64_hex_resolves_exact() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = dismiss_then_force_state(&store, "full-hash", PromotionState::Candidate);
    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "show",
            &h.to_hex(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("full-hash"));
}

// ---------------------------------------------------------------------------
// `show` and `history` content (WI-6).
// ---------------------------------------------------------------------------

#[test]
fn show_renders_all_fields_for_v2_row() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = dismiss_then_force_state(&store, "shown", PromotionState::LocalOnly);
    insert_transition(
        &store,
        &h,
        "candidate",
        "local_only",
        "auto_recurrence",
        1000,
        Some(3),
    );

    let out = quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "show",
            &h.to_hex()[..12],
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("title:"));
    assert!(stdout.contains("shown"));
    assert!(stdout.contains("state:"));
    assert!(stdout.contains("local_only"));
    assert!(stdout.contains("transitions:"));
    assert!(stdout.contains("candidate → local_only"));
    assert!(stdout.contains("auto_recurrence"));
    assert!(stdout.contains("recurrence=3 at transition"));
}

#[test]
fn show_pre_v2_row_emits_no_history_note() {
    // BUG 3: a row that has advanced past `candidate` with zero
    // state_transitions rows can only come from a v1 row migrated under
    // schema v2 — v2 rows leaving `candidate` always write a T1
    // transition. Forcing the row to `local_only` (no transitions
    // inserted) simulates that pre-v2 migration shape.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = dismiss_then_force_state(&store, "pre-v2", PromotionState::LocalOnly);

    let out = quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "show",
            &h.to_hex(),
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("predates schema v2"));
    assert!(
        !stdout.contains("dismissal is in initial state"),
        "non-candidate row should not get the v2-initial-state message"
    );
}

#[test]
fn show_fresh_candidate_emits_initial_state_note() {
    // BUG 3: a fresh v2 candidate row with no transitions must NOT be
    // reported as "predates schema v2". `candidate` is the initial state
    // and no transition row is written when a row enters it.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = dismiss_then_force_state(&store, "fresh-cand", PromotionState::Candidate);

    let out = quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "show",
            &h.to_hex(),
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("dismissal is in initial state"));
    assert!(
        !stdout.contains("predates schema v2"),
        "fresh candidate row must not be misreported as pre-v2"
    );
}

#[test]
fn show_with_no_body_snapshot_shows_placeholder() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    // Insert a row with empty body via dismiss → body_snapshot = None.
    let f = Finding {
        body: String::new(),
        ..sample_finding("no-body", &["m"])
    };
    let h = finding_identity_hash(&f);
    store
        .dismiss(
            &f,
            "sha",
            "main",
            DismissalReason::FalsePositive,
            None,
            Some(time::Duration::days(365)),
        )
        .unwrap();
    let out = quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "show",
            &h.to_hex(),
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("<no body snapshot>"));
}

#[test]
fn history_renders_transition_log_oldest_first() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = dismiss_then_force_state(&store, "histed", PromotionState::PromotedConvention);
    insert_transition(
        &store,
        &h,
        "candidate",
        "local_only",
        "auto_recurrence",
        1000,
        Some(3),
    );
    insert_transition(
        &store,
        &h,
        "local_only",
        "promoted_convention",
        "explicit_promote",
        5000,
        None,
    );

    let out = quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "history",
            &h.to_hex(),
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let i1 = stdout.find("auto_recurrence").unwrap();
    let i2 = stdout.find("explicit_promote").unwrap();
    assert!(i1 < i2, "oldest-first ordering");
}

#[test]
fn history_pre_v2_row_emits_note() {
    // BUG 3: pre-v2 simulation uses a non-candidate state with no
    // transitions (see show_pre_v2_row_emits_no_history_note).
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = dismiss_then_force_state(&store, "pre-v2-h", PromotionState::LocalOnly);
    let out = quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "history",
            &h.to_hex(),
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("predates schema v2"));
}

#[test]
fn history_fresh_candidate_emits_initial_state_note() {
    // BUG 3: companion to show_fresh_candidate_emits_initial_state_note —
    // verifies the `history` subcommand also distinguishes the two cases.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = dismiss_then_force_state(&store, "fresh-cand-h", PromotionState::Candidate);
    let out = quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "history",
            &h.to_hex(),
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("dismissal is in initial state"));
}

// ---------------------------------------------------------------------------
// `--orphans` (AC 169) + AC 168 partial.
// ---------------------------------------------------------------------------

const MANAGED_FENCE_OPEN: &str = "<!-- quorum:managed-section v=1 -->";
const MANAGED_FENCE_CLOSE: &str = "<!-- /quorum:managed-section -->";

fn write_conventions_md(td: &TempDir, body: &str) {
    let dir = td.path().join(".quorum");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("conventions.md"), body).unwrap();
}

#[test]
fn orphans_reports_file_and_db_orphan() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    // DB row whose block_id will NOT appear in conventions.md → db orphan.
    let h = dismiss_then_force_state(&store, "db-orph", PromotionState::PromotedConvention);
    insert_convention_row(&store, &h, "abcdef012345", "body");
    // conventions.md with a different block id → that block is a file orphan.
    let md = format!(
        "{MANAGED_FENCE_OPEN}
<!-- quorum:convention id=ffffeeeeaaaa v=1 -->
### Convention: hand-edited
body
<!-- /quorum:convention -->
{MANAGED_FENCE_CLOSE}
"
    );
    write_conventions_md(&td, &md);

    let out = quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "list",
            "--orphans",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("file orphans"));
    assert!(stdout.contains("ffffeeeeaaaa"));
    assert!(stdout.contains("db orphans"));
    assert!(stdout.contains("abcdef012345"));
}

#[test]
fn orphans_clean_state_reports_no_orphans() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = dismiss_then_force_state(&store, "matching", PromotionState::PromotedConvention);
    insert_convention_row(&store, &h, "abcdef012345", "body");
    let md = format!(
        "{MANAGED_FENCE_OPEN}
<!-- quorum:convention id=abcdef012345 v=1 -->
### Convention: matching
body
<!-- /quorum:convention -->
{MANAGED_FENCE_CLOSE}
"
    );
    write_conventions_md(&td, &md);

    let out = quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "list",
            "--orphans",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("(no orphans)"));
}

#[test]
fn orphans_missing_file_reports_all_db_rows() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = dismiss_then_force_state(&store, "missing-md", PromotionState::PromotedConvention);
    insert_convention_row(&store, &h, "abcdef012345", "body");
    // No conventions.md written.

    let out = quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "list",
            "--orphans",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("not found"));
    assert!(stdout.contains("abcdef012345"));
}

#[test]
fn orphans_malformed_block_emits_stderr_warning() {
    // AC 168 partial: a malformed block surfaces as a stderr warning while
    // the rest of the file still parses. Stage 4 closes the symmetric
    // emission from promote/demote.
    let td = init_repo();
    let _ = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let md = format!(
        "{MANAGED_FENCE_OPEN}
<!-- quorum:convention id=a1b2c3d4e5f6 v=1 -->
### Convention: dangling
missing close marker
{MANAGED_FENCE_CLOSE}
"
    );
    write_conventions_md(&td, &md);

    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "list",
            "--orphans",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("warning:"))
        .stderr(predicate::str::contains("a1b2c3d4e5f6"));
}

#[test]
fn orphans_json_is_parseable() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = dismiss_then_force_state(&store, "j-row", PromotionState::PromotedConvention);
    insert_convention_row(&store, &h, "abcdef012345", "body");
    let md = format!(
        "{MANAGED_FENCE_OPEN}
<!-- quorum:convention id=999988887777 v=1 -->
### Convention: only in file
body
<!-- /quorum:convention -->
{MANAGED_FENCE_CLOSE}
"
    );
    write_conventions_md(&td, &md);

    let out = quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "list",
            "--orphans",
            "--json",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let val: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(val["file_missing"], false);
    assert_eq!(val["fence_absent"], false);
    let file_orph = val["file_orphans"].as_array().unwrap();
    assert_eq!(file_orph.len(), 1);
    assert_eq!(file_orph[0]["id"], "999988887777");
    let db_orph = val["db_orphans"].as_array().unwrap();
    assert_eq!(db_orph.len(), 1);
    assert_eq!(db_orph[0]["conventions_md_block_id"], "abcdef012345");
}
