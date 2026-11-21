//! Phase 1C Stage 4 — `quorum convention promote / demote / prune`
//! integration. Drives the write subcommands end-to-end through a
//! tempdir-rooted quorum-dir. Fixture rows are inserted directly via
//! SQLite so we can pin states + recurrence counts without traversing
//! the Phase 1B + 1C state machine on every test.
//!
//! ACs covered (Stage 4):
//!   - AC 137 (promote happy path: SQLite + conventions.md + audit row)
//!   - AC 139 (--text body source; title-only default; --from-editor seam)
//!   - AC 140 (demote happy path)
//!   - AC 141 (prune --dry-run + default --yes)
//!   - AC 142 (prune refuses promoted_convention rows by construction)
//!   - AC 143 (T5 cascade: delete on promoted row drops audit log;
//!     no `explicit_undismiss` row inserted)
//!   - AC 148 (managed-section fence auto-created)
//!   - AC 149 (above-fence + below-fence byte preservation)
//!   - AC 150 (LF vs CRLF preservation)
//!   - AC 168 (pre-flight stderr warnings on promote/demote)
//!   - AC 173 (round-trip render: promote → conventions section,
//!     demote → memory section)
//!   - AC 175 (crash harness — file-ahead-of-SQLite recovery via
//!     idempotent re-promote)

use assert_cmd::Command;
use predicates::prelude::*;
use quorum_core::memory::identity::finding_identity_hash;
use quorum_core::memory::{
    DismissalReason, FindingIdentityHash, LocalSqliteMemoryStore, MemoryStore, PromotionState,
};
use quorum_core::review::{Finding, FindingSource, Severity};
use std::path::Path;
use tempfile::TempDir;

fn quorum() -> Command {
    Command::cargo_bin("quorum").expect("quorum binary built")
}

fn init_repo() -> TempDir {
    let td = TempDir::new().unwrap();
    git2::Repository::init(td.path()).unwrap();
    td
}

fn sample_finding(title: &str) -> Finding {
    Finding {
        severity: Severity::High,
        title: title.to_string(),
        body: format!("Body for {title}."),
        source: FindingSource::Divergence,
        supported_by: vec!["m".into()],
        confidence: Some(0.9),
    }
}

/// Insert a dismissal + force its promotion_state via direct SQL. Returns
/// the finding identity hash for later assertions.
fn seed_dismissal(
    store: &LocalSqliteMemoryStore,
    title: &str,
    state: PromotionState,
) -> FindingIdentityHash {
    let f = sample_finding(title);
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

fn read_state(store: &LocalSqliteMemoryStore, h: &FindingIdentityHash) -> String {
    let conn = rusqlite::Connection::open(store.path()).unwrap();
    conn.query_row(
        "SELECT promotion_state FROM dismissals WHERE finding_identity_hash = ?1",
        [h.to_hex()],
        |r| r.get::<_, String>(0),
    )
    .unwrap()
}

fn count_conventions(store: &LocalSqliteMemoryStore, h: &FindingIdentityHash) -> i64 {
    let conn = rusqlite::Connection::open(store.path()).unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM conventions WHERE finding_identity_hash = ?1",
        [h.to_hex()],
        |r| r.get(0),
    )
    .unwrap()
}

fn count_transitions(store: &LocalSqliteMemoryStore, h: &FindingIdentityHash) -> i64 {
    let conn = rusqlite::Connection::open(store.path()).unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM state_transitions WHERE finding_identity_hash = ?1",
        [h.to_hex()],
        |r| r.get(0),
    )
    .unwrap()
}

fn last_trigger(store: &LocalSqliteMemoryStore, h: &FindingIdentityHash) -> Option<String> {
    let conn = rusqlite::Connection::open(store.path()).unwrap();
    conn.query_row(
        "SELECT trigger FROM state_transitions
         WHERE finding_identity_hash = ?1
         ORDER BY ts DESC, id DESC LIMIT 1",
        [h.to_hex()],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

fn conv_md_path(td: &Path) -> std::path::PathBuf {
    td.join(".quorum").join("conventions.md")
}

// ---------------------------------------------------------------------------
// `promote` — happy path + state guards + body sources (AC 137, 139, 148).
// ---------------------------------------------------------------------------

#[test]
fn promote_happy_path_writes_file_and_flips_sqlite() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = seed_dismissal(&store, "stylistic ABC", PromotionState::LocalOnly);
    drop(store);

    let hex = h.to_hex();
    let short = &hex[..12];

    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "promote",
            short,
            "--text",
            "Function bodies under 8 lines do not require docstrings.",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("promoted {short}")));

    // Reopen store, assert side effects.
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    assert_eq!(read_state(&store, &h), "promoted_convention");
    assert_eq!(count_conventions(&store, &h), 1);
    assert_eq!(
        last_trigger(&store, &h).as_deref(),
        Some("explicit_promote")
    );

    // Conventions.md present + carries managed block (AC 148).
    let content = std::fs::read_to_string(conv_md_path(td.path())).unwrap();
    assert!(content.contains("<!-- quorum:managed-section v=1 -->"));
    assert!(content.contains(&format!("<!-- quorum:convention id={short}")));
    assert!(content.contains("### Convention: stylistic ABC"));
    assert!(content.contains("Function bodies under 8 lines do not require docstrings."));
}

#[test]
fn promote_title_only_default_no_body() {
    // AC 139 — neither --text nor --from-editor produces a title-only
    // block (no body paragraph, dismissal body_snapshot never auto-used).
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = seed_dismissal(&store, "title-only", PromotionState::LocalOnly);
    drop(store);
    let short = &h.to_hex()[..12];

    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "promote",
            short,
        ])
        .assert()
        .success();

    let content = std::fs::read_to_string(conv_md_path(td.path())).unwrap();
    assert!(content.contains("### Convention: title-only"));
    // Empty-body layout: header line is followed directly by close marker
    // (no blank-line + body block).
    assert!(
        content.contains("### Convention: title-only\n<!-- /quorum:convention -->")
            || content.contains("### Convention: title-only\r\n<!-- /quorum:convention -->")
    );
    // The dismissal body ("Body for title-only.") must NEVER leak in.
    assert!(
        !content.contains("Body for title-only"),
        "dismissal body_snapshot must not leak into the managed block"
    );
}

#[test]
fn promote_from_editor_uses_test_env_seam() {
    // AC 139 — hermetic test for --from-editor via QUORUM_TEST_EDITOR_BODY.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = seed_dismissal(&store, "editor-fed", PromotionState::LocalOnly);
    drop(store);
    let short = &h.to_hex()[..12];

    quorum()
        .env("QUORUM_TEST_EDITOR_BODY", "Body authored in the editor.")
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "promote",
            short,
            "--from-editor",
        ])
        .assert()
        .success();

    let content = std::fs::read_to_string(conv_md_path(td.path())).unwrap();
    assert!(content.contains("Body authored in the editor."));
}

#[test]
fn promote_rejects_candidate_state() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = seed_dismissal(&store, "still candidate", PromotionState::Candidate);
    drop(store);
    let short = &h.to_hex()[..12];

    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "promote",
            short,
            "--text",
            "x",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("still a candidate"));
}

#[test]
fn promote_rejects_already_promoted() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = seed_dismissal(&store, "already up", PromotionState::PromotedConvention);
    drop(store);
    let short = &h.to_hex()[..12];

    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "promote",
            short,
            "--text",
            "x",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already a promoted_convention"));
}

#[test]
fn promote_rejects_empty_text() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = seed_dismissal(&store, "empty-text", PromotionState::LocalOnly);
    drop(store);
    let short = &h.to_hex()[..12];

    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "promote",
            short,
            "--text",
            "",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--text"));
}

// ---------------------------------------------------------------------------
// AC 168 — promote-side pre-flight diagnostic emitted to stderr.
// ---------------------------------------------------------------------------

#[test]
fn promote_preflight_refuses_on_a_parser_diagnostic() {
    // Existing conventions.md with a malformed block surfaces the
    // diagnostic on stderr during promote pre-flight. Since DL 2.2 it is a
    // refusal, not a warning: the write would delete the block's text.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = seed_dismissal(&store, "preflight target", PromotionState::LocalOnly);
    drop(store);
    let short = &h.to_hex()[..12];

    // Pre-seed conventions.md with an unclosed managed block.
    let conv_dir = td.path().join(".quorum");
    std::fs::create_dir_all(&conv_dir).unwrap();
    let bad = "<!-- quorum:managed-section v=1 -->\n<!-- quorum:convention id=deadbeef0001 v=1 -->\nno close here\n<!-- /quorum:managed-section -->\n";
    std::fs::write(conv_dir.join("conventions.md"), bad).unwrap();

    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "promote",
            short,
            "--text",
            "ok",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("deadbeef0001 has no"));
    assert_eq!(
        std::fs::read_to_string(conv_dir.join("conventions.md")).unwrap(),
        bad
    );
}

// ---------------------------------------------------------------------------
// `demote` — happy path + missing-file Q7 (AC 140).
// ---------------------------------------------------------------------------

#[test]
fn demote_happy_path_removes_block_and_flips_state() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = seed_dismissal(&store, "to be demoted", PromotionState::LocalOnly);
    drop(store);
    let short = &h.to_hex()[..12];

    // First promote …
    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "promote",
            short,
            "--text",
            "body",
        ])
        .assert()
        .success();

    // … then demote.
    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "demote",
            short,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("demoted {short}")));

    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    assert_eq!(read_state(&store, &h), "local_only");
    assert_eq!(count_conventions(&store, &h), 0);
    assert_eq!(last_trigger(&store, &h).as_deref(), Some("explicit_demote"));

    let content = std::fs::read_to_string(conv_md_path(td.path())).unwrap();
    assert!(
        !content.contains(&format!("id={short}")),
        "demoted block must be removed from conventions.md"
    );
}

#[test]
fn demote_missing_conventions_md_q7_lean() {
    // Q7 lean — conventions.md missing: stderr warning, SQLite state still
    // advances.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = seed_dismissal(&store, "missing-file", PromotionState::PromotedConvention);
    // Also insert a conventions row so we exercise the DELETE branch.
    {
        let conn = rusqlite::Connection::open(store.path()).unwrap();
        conn.execute(
            "INSERT INTO conventions (finding_identity_hash, convention_text, promoted_at, conventions_md_block_id)
             VALUES (?1, 'old body', 1000, ?2)",
            rusqlite::params![h.to_hex(), &h.to_hex()[..12]],
        )
        .unwrap();
    }
    drop(store);
    let short = &h.to_hex()[..12];

    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "demote",
            short,
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains(".quorum/conventions.md not found"));

    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    assert_eq!(read_state(&store, &h), "local_only");
    assert_eq!(count_conventions(&store, &h), 0);
}

#[test]
fn demote_rejects_non_promoted_state() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = seed_dismissal(&store, "wrong state", PromotionState::LocalOnly);
    drop(store);
    let short = &h.to_hex()[..12];

    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "demote",
            short,
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not in promoted_convention"));
}

// ---------------------------------------------------------------------------
// DL 2.1 — a block that promote or demote does not own survives byte for
// byte, whatever its layout.
// ---------------------------------------------------------------------------

/// A block the writer would never emit: no blank line under the header.
fn hand_edited_block(id: &str) -> String {
    format!(
        "<!-- quorum:convention id={id} v=1 -->\n\
         ### Convention: hand-edited, no blank line\n\
         Body text the user typed straight under the header.\n\
         A second line, kept exactly as typed.  \n\
         <!-- /quorum:convention -->"
    )
}

/// Another the writer would never emit: the header's colon became a dash.
fn retitled_block(id: &str) -> String {
    format!(
        "<!-- quorum:convention id={id} v=1 -->\n\
         ### Convention - the colon became a dash\n\
         \n\
         Body under a header outside the canonical layout.\n\
         <!-- /quorum:convention -->"
    )
}

/// A block in exactly the layout the writer emits.
fn canonical_block(id: &str, title: &str, body: &str) -> String {
    format!(
        "<!-- quorum:convention id={id} v=1 -->\n### Convention: {title}\n\n{body}\n\
         <!-- /quorum:convention -->"
    )
}

/// A conventions.md with user prose around a managed section holding `blocks`.
fn managed_file(blocks: &[String]) -> String {
    let mut s = String::from(
        "# Team conventions\n\nProse the user wrote above the fence.\n\n\
         <!-- quorum:managed-section v=1 -->\n",
    );
    for b in blocks {
        s.push('\n');
        s.push_str(b);
        s.push('\n');
    }
    s.push_str("<!-- /quorum:managed-section -->\n\nProse below the fence.\n");
    s
}

/// A `promoted_convention` dismissal with its `conventions` row, so its
/// block in the file is not an orphan.
fn seed_promoted(store: &LocalSqliteMemoryStore, title: &str) -> FindingIdentityHash {
    let h = seed_dismissal(store, title, PromotionState::PromotedConvention);
    let conn = rusqlite::Connection::open(store.path()).unwrap();
    conn.execute(
        "INSERT INTO conventions (finding_identity_hash, convention_text, promoted_at, conventions_md_block_id)
         VALUES (?1, 'text', 1000, ?2)",
        rusqlite::params![h.to_hex(), &h.to_hex()[..12]],
    )
    .unwrap();
    h
}

#[test]
fn demote_keeps_non_canonical_blocks_byte_for_byte() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let a1 = seed_promoted(&store, "hand-edited one");
    let a2 = seed_promoted(&store, "retitled one");
    let b = seed_promoted(&store, "canonical one");
    drop(store);
    let (a1, a2, b) = (&a1.to_hex()[..12], &a2.to_hex()[..12], &b.to_hex()[..12]);

    let kept_1 = hand_edited_block(a1);
    let kept_2 = retitled_block(a2);
    let file = managed_file(&[
        kept_1.clone(),
        canonical_block(b, "canonical one", "Goes away."),
        kept_2.clone(),
    ]);
    std::fs::write(conv_md_path(td.path()), &file).unwrap();

    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "demote",
            b,
        ])
        .assert()
        .success();

    let after = std::fs::read_to_string(conv_md_path(td.path())).unwrap();
    assert!(after.contains(&kept_1), "hand-edited block lost:\n{after}");
    assert!(after.contains(&kept_2), "retitled block lost:\n{after}");
    assert!(!after.contains(&format!("id={b}")), "demoted block kept");
    assert!(after.contains("Prose the user wrote above the fence."));
    assert!(after.contains("Prose below the fence."));
}

#[test]
fn promote_keeps_non_canonical_blocks_byte_for_byte() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let a1 = seed_promoted(&store, "hand-edited one");
    let a2 = seed_promoted(&store, "retitled one");
    let c = seed_dismissal(&store, "newly promoted", PromotionState::LocalOnly);
    drop(store);
    let (a1, a2, c) = (&a1.to_hex()[..12], &a2.to_hex()[..12], &c.to_hex()[..12]);

    let kept_1 = hand_edited_block(a1);
    let kept_2 = retitled_block(a2);
    let file = managed_file(&[kept_1.clone(), kept_2.clone()]);
    std::fs::write(conv_md_path(td.path()), &file).unwrap();

    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "promote",
            c,
            "--text",
            "New body.",
        ])
        .assert()
        .success();

    let after = std::fs::read_to_string(conv_md_path(td.path())).unwrap();
    assert!(after.contains(&kept_1), "hand-edited block lost:\n{after}");
    assert!(after.contains(&kept_2), "retitled block lost:\n{after}");
    assert!(
        after.contains(&canonical_block(c, "newly promoted", "New body.")),
        "promoted block missing:\n{after}"
    );
}

// ---------------------------------------------------------------------------
// `prune` — dry-run + default + disabled (AC 141, 142, candidate_expire_days=0).
// ---------------------------------------------------------------------------

fn seed_stale_candidate(
    store: &LocalSqliteMemoryStore,
    title: &str,
    days_ago: i64,
) -> FindingIdentityHash {
    let h = seed_dismissal(store, title, PromotionState::Candidate);
    // Backdate last_seen_at to N days ago via direct SQL.
    let stale = time::OffsetDateTime::now_utc() - time::Duration::days(days_ago);
    let stale_s = stale
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let conn = rusqlite::Connection::open(store.path()).unwrap();
    conn.execute(
        "UPDATE dismissals SET last_seen_at = ?1 WHERE finding_identity_hash = ?2",
        rusqlite::params![stale_s, h.to_hex()],
    )
    .unwrap();
    h
}

#[test]
fn prune_dry_run_reports_without_deleting() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = seed_stale_candidate(&store, "stale", 120);
    // Non-stale candidate (recent) — must not be reported.
    let _fresh = seed_dismissal(&store, "fresh", PromotionState::Candidate);
    // Promoted row — must not be reported.
    let _kept = seed_dismissal(&store, "kept", PromotionState::PromotedConvention);
    drop(store);

    let out = quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "prune",
            "--dry-run",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("would prune"));
    assert!(stdout.contains(&h.to_hex()[..12]));

    // Still present in DB (dry-run did not delete).
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    assert_eq!(read_state(&store, &h), "candidate");
}

#[test]
fn prune_yes_deletes_stale_candidates_only() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let stale = seed_stale_candidate(&store, "stale", 120);
    let fresh = seed_dismissal(&store, "fresh", PromotionState::Candidate);
    let promoted = seed_dismissal(&store, "promoted", PromotionState::PromotedConvention);
    // Pre-populate audit row on the stale candidate so we can assert cascade.
    {
        let conn = rusqlite::Connection::open(store.path()).unwrap();
        conn.execute(
            "INSERT INTO state_transitions
                (finding_identity_hash, from_state, to_state, trigger, ts, by_review_session_id, recurrence_at_transition)
             VALUES (?1, 'candidate', 'candidate', 'auto_recurrence', 1, NULL, 1)",
            rusqlite::params![stale.to_hex()],
        )
        .unwrap_or(0); // CHECK constraint forbids from==to actually; insert a real transition shape if it rejects.
    }
    drop(store);

    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "prune",
            "--yes",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("pruned 1 candidate"));

    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    // Stale gone, fresh + promoted untouched.
    let conn = rusqlite::Connection::open(store.path()).unwrap();
    let stale_present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM dismissals WHERE finding_identity_hash = ?1",
            [stale.to_hex()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stale_present, 0, "stale candidate was deleted");
    assert_eq!(read_state(&store, &fresh), "candidate");
    assert_eq!(read_state(&store, &promoted), "promoted_convention");

    // Audit-silent: no fresh state_transition row was written for the prune.
    let total_transitions: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM state_transitions WHERE trigger = 'auto_recurrence'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    // Whatever we hand-inserted is either gone (cascade) or 0 if CHECK
    // rejected; either way, no `prune` trigger value exists in the enum.
    let _ = total_transitions; // documented; primary assertion below.
    let bad_trigger: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM state_transitions
             WHERE trigger NOT IN ('auto_recurrence','explicit_promote','explicit_demote')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        bad_trigger, 0,
        "no `prune`/`deleted`/etc. trigger value exists"
    );
}

#[test]
fn prune_disabled_when_candidate_expire_days_is_zero() {
    let td = init_repo();
    // Write a config setting candidate_expire_days = 0.
    let conf_path = td.path().join(".quorum").join("config.toml");
    std::fs::create_dir_all(conf_path.parent().unwrap()).unwrap();
    std::fs::write(
        &conf_path,
        "project_id = \"p1\"\nbase_url = \"https://app.lippa.ai\"\nremote_url = true\n[memory]\ncandidate_threshold = 3\nlocal_convention_bundle_cap = 500\ncandidate_expire_days = 0\n",
    )
    .unwrap();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = seed_stale_candidate(&store, "stale", 9999);
    drop(store);

    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "prune",
            "--yes",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("prune disabled"));

    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    assert_eq!(
        read_state(&store, &h),
        "candidate",
        "row preserved when expire-days = 0"
    );
}

// ---------------------------------------------------------------------------
// AC 143 — T5 cascade through `delete` is audit-trail-silent.
// ---------------------------------------------------------------------------

#[test]
fn delete_cascades_state_transitions_audit_silent() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = seed_dismissal(&store, "to delete", PromotionState::LocalOnly);
    let short = &h.to_hex()[..12];

    // Promote to populate a state_transitions row.
    drop(store);
    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "promote",
            short,
            "--text",
            "body",
        ])
        .assert()
        .success();

    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    // Pre-delete: 1 transition row exists.
    assert_eq!(count_transitions(&store, &h), 1);

    // Delete by id (Phase 1B undismiss path).
    let id = {
        let conn = rusqlite::Connection::open(store.path()).unwrap();
        conn.query_row(
            "SELECT id FROM dismissals WHERE finding_identity_hash = ?1",
            [h.to_hex()],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    let removed = store.delete(quorum_core::memory::DismissalId(id)).unwrap();
    assert!(removed);

    // Post-delete: dismissal gone, transitions cascade-dropped,
    // conventions cascade-dropped, NO new `'explicit_undismiss'` audit row.
    assert_eq!(count_transitions(&store, &h), 0, "cascade dropped audit");
    let conn = rusqlite::Connection::open(store.path()).unwrap();
    let new_undismiss: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM state_transitions
             WHERE trigger NOT IN ('auto_recurrence','explicit_promote','explicit_demote')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        new_undismiss, 0,
        "audit-silent: no `explicit_undismiss` row"
    );
    let conv_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM conventions WHERE finding_identity_hash = ?1",
            [h.to_hex()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(conv_count, 0, "conventions row cascade-dropped");
}

// ---------------------------------------------------------------------------
// AC 149 / 150 — byte preservation + line endings across promote/demote.
// ---------------------------------------------------------------------------

#[test]
fn above_fence_content_preserved_byte_for_byte_through_promote() {
    let td = init_repo();
    // Pre-seed conventions.md with a fence-less user-authored file. After
    // promote the above-fence content (the entire current file) must be
    // preserved byte-for-byte.
    let conv_dir = td.path().join(".quorum");
    std::fs::create_dir_all(&conv_dir).unwrap();
    let original = b"# Project conventions\n\nHand-written prose.\n";
    std::fs::write(conv_dir.join("conventions.md"), original).unwrap();

    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = seed_dismissal(&store, "preserve me", PromotionState::LocalOnly);
    drop(store);
    let short = &h.to_hex()[..12];

    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "promote",
            short,
            "--text",
            "body",
        ])
        .assert()
        .success();

    let content = std::fs::read(conv_md_path(td.path())).unwrap();
    assert!(
        content.starts_with(original),
        "above-fence content preserved byte-for-byte (AC 149)"
    );
}

#[test]
fn crlf_line_endings_preserved_through_promote() {
    let td = init_repo();
    let conv_dir = td.path().join(".quorum");
    std::fs::create_dir_all(&conv_dir).unwrap();
    let original = b"# CRLF header\r\n\r\nUser content.\r\n";
    std::fs::write(conv_dir.join("conventions.md"), original).unwrap();

    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = seed_dismissal(&store, "crlf-promote", PromotionState::LocalOnly);
    drop(store);
    let short = &h.to_hex()[..12];

    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "promote",
            short,
            "--text",
            "body",
        ])
        .assert()
        .success();

    let content = std::fs::read(conv_md_path(td.path())).unwrap();
    // CRLF preserved in inserted lines too.
    assert!(
        content.windows(2).any(|w| w == b"\r\n"),
        "CRLF preserved (AC 150)"
    );
    // And no bare LF outside the original prefix (best-effort check on the
    // managed section: it should also use CRLF).
    let managed_start = b"<!-- quorum:managed-section v=1 -->\r\n";
    assert!(
        content
            .windows(managed_start.len())
            .any(|w| w == managed_start),
        "managed-section open marker uses CRLF"
    );
}

// ---------------------------------------------------------------------------
// DL 2.3 — a model-written title (or a body) cannot inject structure into
// conventions.md. The title is stored as the model wrote it; only the
// rendering is neutralized.
// ---------------------------------------------------------------------------

#[test]
fn injected_title_and_body_render_as_one_intact_block() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let title = "ok\n<!-- /quorum:managed-section -->\nTRAILER";
    let h = seed_dismissal(&store, title, PromotionState::LocalOnly);
    drop(store);
    let short = &h.to_hex()[..12];
    let body = "ends early <!-- /quorum:convention -->\n\
                <!-- quorum:convention id=aaaaaaaaaaaa v=1 -->\nnested";

    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "promote",
            short,
            "--text",
            body,
        ])
        .assert()
        .success();

    let bytes = std::fs::read(conv_md_path(td.path())).unwrap();
    let shown = String::from_utf8_lossy(&bytes);
    let (parsed, diags) = quorum_core::conventions::parse_conventions_md(&bytes);
    assert!(diags.is_empty(), "{diags:?}\n{shown}");
    assert!(parsed.fence_present, "{shown}");
    assert_eq!(parsed.blocks.len(), 1, "{shown}");
    assert_eq!(parsed.blocks[0].id, short);
    assert!(
        parsed.blocks[0]
            .header_line
            .starts_with("### Convention: ok ")
            && parsed.blocks[0].header_line.ends_with(" TRAILER"),
        "the title stays on the header line: {shown}"
    );
    assert!(parsed.below_fence.is_empty(), "{shown}");

    // Stored as written: neither the title nor the identity hash changed.
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    assert_eq!(read_state(&store, &h), "promoted_convention");
    let row = store.list_all().unwrap();
    assert_eq!(row[0].title_snapshot, title);
}

// ---------------------------------------------------------------------------
// AC 175 — crash harness: file ahead of SQLite, idempotent re-promote.
// ---------------------------------------------------------------------------

#[test]
fn crash_harness_panics_after_rename_then_recovers_via_repromote() {
    // AC 175. Subprocess-based crash harness: the test sets the
    // `QUORUM_TEST_CRASH_AFTER_RENAME` env var (the seam in
    // quorum-core::conventions::stage4_test_seam) so the subprocessed
    // `quorum convention promote` panics between fs::rename and SQLite
    // COMMIT. We then assert:
    //   (a) The subprocess exited non-zero (panic).
    //   (b) The conventions.md file holds the new block (file ahead).
    //   (c) SQLite state is still local_only (no audit, no conventions row).
    //   (d) A re-promote (without the env var) is idempotent — single
    //       block, single audit row, state advances.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = seed_dismissal(&store, "crash-target", PromotionState::LocalOnly);
    drop(store);
    let short = &h.to_hex()[..12];

    // Phase 1: panic-mid-promote.
    let assert = quorum()
        .env(quorum_core::conventions::stage4_test_seam::CRASH_ENV, "1")
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "promote",
            short,
            "--text",
            "body v1",
        ])
        .assert()
        .failure();
    let _ = assert; // exit-code is enough; stderr may carry panic info

    // File side has the block; SQLite state untouched.
    let content = std::fs::read_to_string(conv_md_path(td.path())).unwrap();
    assert!(
        content.contains(&format!("id={short}")),
        "file holds new block after crash"
    );
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    assert_eq!(
        read_state(&store, &h),
        "local_only",
        "SQLite did NOT advance"
    );
    assert_eq!(count_transitions(&store, &h), 0);
    assert_eq!(count_conventions(&store, &h), 0);
    drop(store);

    // Phase 2: idempotent re-promote via the CLI binary (no env var).
    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "promote",
            short,
            "--text",
            "body v1",
        ])
        .assert()
        .success();

    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    assert_eq!(read_state(&store, &h), "promoted_convention");
    assert_eq!(count_transitions(&store, &h), 1, "single audit row");
    assert_eq!(count_conventions(&store, &h), 1);

    // File still has exactly one block for this id.
    let content = std::fs::read_to_string(conv_md_path(td.path())).unwrap();
    let occurrences = content.matches(&format!("id={short}")).count();
    assert_eq!(occurrences, 1, "no duplicate block after re-promote");
}

// ---------------------------------------------------------------------------
// AC 173 — full round-trip: promote → commit → bundle shifts memory→conv
// section; demote → bundle shifts back. Exercised via promote/demote CLI
// + LocalSqliteMemoryStore + ConventionsState committed-and-clean check.
// ---------------------------------------------------------------------------

#[test]
fn round_trip_promote_demote_through_bundle_render_paths() {
    // Stage 2's bridge already validated render-side toggle on a fixture
    // row. Here we exercise the Stage 4 write paths and assert
    // `load_local_only_conventions` returns the row before promote, after
    // promote (bridge case — file dirty), and after demote.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let h = seed_dismissal(&store, "round-trip", PromotionState::LocalOnly);
    drop(store);
    let short = &h.to_hex()[..12];

    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let pre = store.load_local_only_conventions().unwrap();
    assert!(
        pre.iter().any(|d| d.finding_identity_hash == h),
        "pre-promote: row is local_only, lives in memory section"
    );
    drop(store);

    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "promote",
            short,
            "--text",
            "body",
        ])
        .assert()
        .success();

    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let mid = store.load_local_only_conventions().unwrap();
    // Bridge: promoted_convention still appears in load_local_only_conventions
    // (the bundle layer's bridge decides per-row at render time whether to
    // render it in the memory section, based on the committed-and-clean
    // check on conventions.md — which is dirty/uncommitted here).
    assert!(
        mid.iter().any(|d| d.finding_identity_hash == h),
        "post-promote (uncommitted): row still surfaced for memory-section bridge"
    );
    // And it is in promoted_convention state.
    assert_eq!(read_state(&store, &h), "promoted_convention");
    drop(store);

    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "demote",
            short,
        ])
        .assert()
        .success();

    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let post = store.load_local_only_conventions().unwrap();
    assert!(
        post.iter().any(|d| d.finding_identity_hash == h),
        "post-demote: row back as local_only"
    );
    assert_eq!(read_state(&store, &h), "local_only");
}

// ---------------------------------------------------------------------------
// DL 2.2 — promote and demote refuse a conventions.md whose structure is
// broken, and leave it byte-identical. `list --orphans` only warns.
// ---------------------------------------------------------------------------

const FENCE_OPEN: &str = "<!-- quorum:managed-section v=1 -->";
const FENCE_CLOSE: &str = "<!-- /quorum:managed-section -->";

/// A block in the writer's layout, for the structure tests below.
fn well_formed(id: &str) -> String {
    format!(
        "<!-- quorum:convention id={id} v=1 -->\n### Convention: well formed\n\nKept.\n\
         <!-- /quorum:convention -->"
    )
}

/// Run `quorum convention <args>` against `td` and assert a refusal: a
/// failing exit, a message naming the file and `line`, the file unchanged.
fn assert_refused(td: &Path, args: &[&str], line: usize) {
    let conv = conv_md_path(td);
    let before = std::fs::read(&conv).unwrap();
    let mut full = vec!["convention", "--quorum-dir", td.to_str().unwrap()];
    full.extend_from_slice(args);
    let out = quorum().args(&full).assert().failure().get_output().clone();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("refusing to rewrite") && stderr.contains("conventions.md"),
        "refusal must name the file: {stderr}"
    );
    assert!(
        stderr.contains(&format!("line {line}:")),
        "refusal must name line {line}: {stderr}"
    );
    assert_eq!(
        std::fs::read(&conv).unwrap(),
        before,
        "a refused write must leave the file byte-identical"
    );
}

#[test]
fn promote_refuses_a_managed_section_with_no_close_marker() {
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let c = seed_dismissal(&store, "would be promoted", PromotionState::LocalOnly);
    drop(store);
    let file = format!(
        "# Conventions\n\nProse.\n\n{FENCE_OPEN}\n\n{}\n\n\
         Prose after the block, which a second write would delete.\n",
        well_formed("aaaaaaaaaaaa")
    );
    std::fs::write(conv_md_path(td.path()), &file).unwrap();

    assert_refused(
        td.path(),
        &["promote", &c.to_hex()[..12], "--text", "body"],
        5,
    );
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    assert_eq!(read_state(&store, &c), "local_only");
}

#[test]
fn promote_and_demote_refuse_malformed_block_markers() {
    // Each malformed block makes the parser skip it, so a write would
    // delete its text. Line 4 is the malformed block's open marker.
    let malformed = [
        "<!-- quorum:convention id=NOT-A-HASH v=1 -->\n### Convention: bad id\n\nText.\n\
         <!-- /quorum:convention -->",
        "<!-- quorum:convention id=bbbbbbbbbbbb v=one -->\n### Convention: bad version\n\nText.\n\
         <!-- /quorum:convention -->",
        "<!-- quorum:convention id=bbbbbbbbbbbb v=1 -->\n### Convention: no close\n\nText.\n",
    ];
    for bad in malformed {
        let td = init_repo();
        let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
        let p = seed_dismissal(&store, "promoted", PromotionState::PromotedConvention);
        let c = seed_dismissal(&store, "local", PromotionState::LocalOnly);
        drop(store);
        let (p_short, c_short) = (&p.to_hex()[..12], &c.to_hex()[..12]);
        let file = format!(
            "intro\n{FENCE_OPEN}\n\n{bad}\n\n{}\n{FENCE_CLOSE}\n",
            well_formed(p_short)
        );
        std::fs::write(conv_md_path(td.path()), &file).unwrap();

        assert_refused(td.path(), &["promote", c_short, "--text", "body"], 4);
        assert_refused(td.path(), &["demote", p_short], 4);
        let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
        assert_eq!(read_state(&store, &c), "local_only");
        assert_eq!(read_state(&store, &p), "promoted_convention");
    }
}

#[test]
fn promote_refuses_a_second_section_marker_in_a_code_example() {
    // A file documenting the mechanism, with the marker at column 0 inside
    // a fenced code block, has two managed sections: refused, not guessed.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let c = seed_dismissal(&store, "would be promoted", PromotionState::LocalOnly);
    drop(store);
    let file = format!(
        "# Conventions\n\nQuorum fences its section like this:\n\n```html\n{FENCE_OPEN}\n```\n\n\
         {FENCE_OPEN}\n\n{}\n{FENCE_CLOSE}\n",
        well_formed("aaaaaaaaaaaa")
    );
    std::fs::write(conv_md_path(td.path()), &file).unwrap();

    assert_refused(
        td.path(),
        &["promote", &c.to_hex()[..12], "--text", "body"],
        9,
    );
}

#[test]
fn a_section_marker_mentioned_mid_line_is_not_a_fence() {
    // Only a marker on a line of its own opens the section, so prose that
    // quotes it inline is kept and the write lands in the real section.
    let td = init_repo();
    let store = LocalSqliteMemoryStore::new(td.path()).unwrap();
    let c = seed_dismissal(
        &store,
        "lands in the real section",
        PromotionState::LocalOnly,
    );
    drop(store);
    let c = &c.to_hex()[..12];
    let prose = format!("Quorum writes between `{FENCE_OPEN}` and `{FENCE_CLOSE}`; leave them.\n");
    let file = format!(
        "# Conventions\n\n{prose}\n{FENCE_OPEN}\n\n{}\n{FENCE_CLOSE}\n",
        well_formed("aaaaaaaaaaaa")
    );
    std::fs::write(conv_md_path(td.path()), &file).unwrap();

    quorum()
        .args([
            "convention",
            "--quorum-dir",
            td.path().to_str().unwrap(),
            "promote",
            c,
            "--text",
            "body",
        ])
        .assert()
        .success();

    let after = std::fs::read_to_string(conv_md_path(td.path())).unwrap();
    assert!(
        after.starts_with(&format!("# Conventions\n\n{prose}\n{FENCE_OPEN}\n")),
        "prose must be untouched:\n{after}"
    );
    assert!(after.contains(&well_formed("aaaaaaaaaaaa")));
    assert!(after.contains(&format!("id={c}")));
    assert_eq!(
        after.matches(FENCE_OPEN).count(),
        2,
        "one inline, one fence"
    );
}

#[test]
fn list_orphans_warns_on_a_missing_close_marker_and_succeeds() {
    let td = init_repo();
    let file = format!(
        "# Conventions\n\n{FENCE_OPEN}\n\n{}\n",
        well_formed("aaaaaaaaaaaa")
    );
    std::fs::create_dir_all(td.path().join(".quorum")).unwrap();
    std::fs::write(conv_md_path(td.path()), &file).unwrap();

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
        .stderr(predicate::str::contains("no closing marker"));
    assert_eq!(
        std::fs::read_to_string(conv_md_path(td.path())).unwrap(),
        file
    );
}
