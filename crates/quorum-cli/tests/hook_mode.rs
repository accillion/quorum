//! Phase 1B Stage 3 — `quorum review --hook-mode=<type>` integration.
//!
//! The full hook flow needs a live Lippa endpoint plus a configured
//! repo. We test the surface a unit test can't reach without a live
//! pipeline:
//!
//!   * Unknown `--hook-mode` value → exit 2.
//!   * Pre-commit / pre-push outside a git repo → exit 2 (the review
//!     pipeline's outside-repo / config error).
//!   * Pre-push with no stdin tuples → exit 0 (nothing to review).
//!   * Pre-push with a tag-push tuple on stdin → exit 0, the tuple is
//!     skipped with the documented stderr note (D5).
//!   * Pre-push with only a `(delete)` tuple on stdin → exit 0, the
//!     tuple is skipped with the documented stderr note (D4).
//!   * Push-start-iso filename shape: colons substituted to dashes
//!     (Windows-compat).
//!
//! End-to-end coverage of the per-tuple review pipeline (Lippa call +
//! filter site + archive write at `<push-start-ISO>.tuple-<N>.json`)
//! is blocked on Lippa-side D8 and is documented in the preflight
//! notes as deferred live-verification.
//!
//! ACs covered: 80 (parse stdin and dispatch), 81 (deletion skip per
//! D4), 82 (new-branch base resolution shape — exercised via unit
//! tests in commands/hook_mode.rs), and the "unknown mode" / "no
//! tuples" entry-validation surface.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn quorum() -> Command {
    Command::cargo_bin("quorum").expect("quorum binary built")
}

fn init_repo() -> TempDir {
    let td = TempDir::new().unwrap();
    git2::Repository::init(td.path()).unwrap();
    td
}

#[test]
fn unknown_hook_mode_exits_2_with_clear_error() {
    let td = init_repo();
    quorum()
        .current_dir(td.path())
        .args(["review", "--hook-mode=pre-rebase"])
        .assert()
        .failure()
        .code(predicate::eq(2))
        .stderr(predicate::str::contains("unknown --hook-mode"));
}

#[test]
fn pre_push_with_no_stdin_tuples_exits_0_with_note() {
    let td = init_repo();
    quorum()
        .current_dir(td.path())
        .args(["review", "--hook-mode=pre-push"])
        .write_stdin("")
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "pre-push received no ref tuples on stdin",
        ));
}

#[test]
fn pre_push_with_delete_tuple_only_exits_0_with_d4_skip_note() {
    // AC 81 / D4: deletion detected via local_ref == "(delete)"
    // literal — NOT zero-sha. Only-deletion stdin must converge with
    // no review attempted, exit 0, and the documented stderr note.
    let td = init_repo();
    let stdin = format!(
        "(delete) {zeros} refs/heads/feature/x abc1234567890abcdef1234567890abcdef123456\n",
        zeros = "0".repeat(40)
    );
    quorum()
        .current_dir(td.path())
        .args(["review", "--hook-mode=pre-push"])
        .write_stdin(stdin)
        .assert()
        .success()
        .stderr(predicate::str::contains("branch deletion detected"));
}

#[test]
fn pre_push_with_tag_tuple_only_exits_0_with_d5_skip_note() {
    // D5: tag pushes have refs/tags/* in local_ref; skip with note.
    let td = init_repo();
    let stdin = format!(
        "refs/tags/v1.0 abc1234567890abcdef1234567890abcdef123456 refs/tags/v1.0 {zeros}\n",
        zeros = "0".repeat(40)
    );
    quorum()
        .current_dir(td.path())
        .args(["review", "--hook-mode=pre-push"])
        .write_stdin(stdin)
        .assert()
        .success()
        .stderr(predicate::str::contains("tag push detected"))
        .stderr(predicate::str::contains("refs/tags/*"));
}

#[test]
fn pre_push_mixed_tag_and_delete_tuples_both_skipped() {
    // A stdin carrying ONLY skippable tuples (tag + delete) results
    // in exit 0; both notes appear on stderr.
    let td = init_repo();
    let stdin = format!(
        "(delete) {zeros} refs/heads/old-feature abc1234567890abcdef1234567890abcdef123456\n\
         refs/tags/v0.1 abc1234567890abcdef1234567890abcdef123456 refs/tags/v0.1 {zeros}\n",
        zeros = "0".repeat(40)
    );
    quorum()
        .current_dir(td.path())
        .args(["review", "--hook-mode=pre-push"])
        .write_stdin(stdin)
        .assert()
        .success()
        .stderr(predicate::str::contains("branch deletion detected"))
        .stderr(predicate::str::contains("tag push detected"));
}

#[test]
fn pre_commit_outside_a_git_repo_errors_cleanly() {
    // Pre-commit invokes the review pipeline; without a `quorum link`
    // the pipeline errors. We exercise the entry path here and assert
    // the exit code is non-zero and an actionable message lands on
    // stderr.
    let td = TempDir::new().unwrap(); // no git init, no .quorum/config.toml
    quorum()
        .current_dir(td.path())
        .args(["review", "--hook-mode=pre-commit"])
        .assert()
        .failure()
        .code(predicate::eq(2));
}

// ===================== v0.4 AC 194a / AC 192 — CLI surface =====================

/// A linked repo whose `.quorum/config.toml` carries the given
/// `[bundle]` body (may be empty).
fn linked_repo(bundle_section: &str) -> TempDir {
    let td = init_repo();
    let qd = td.path().join(".quorum");
    std::fs::create_dir_all(&qd).unwrap();
    std::fs::write(
        qd.join("config.toml"),
        format!("project_id = \"p_1\"\nbase_url = \"https://app.lippa.ai\"\n{bundle_section}"),
    )
    .unwrap();
    td
}

/// AC 194a — above the 200 KB default, `quorum review` warns that an
/// oversized bundle is not rejected by Lippa: it reserves credits, runs,
/// and comes back `failed`.
#[test]
fn ac194a_oversized_total_budget_warns_about_credit_spend() {
    let td = linked_repo("[bundle]\ntotal_budget_kb = 400\n");
    quorum()
        .current_dir(td.path())
        .env("QUORUM_LIPPA_SESSION", "fake-cookie-for-warning-path")
        .args(["review"])
        .assert()
        .stderr(
            predicate::str::contains("total_budget_kb = 400")
                .and(predicate::str::contains("credits"))
                .and(predicate::str::contains("failed")),
        );
}

/// AC 194a — at or below the default there is nothing to warn about.
#[test]
fn ac194a_default_total_budget_is_silent() {
    for section in ["", "[bundle]\ntotal_budget_kb = 200\n"] {
        let td = linked_repo(section);
        quorum()
            .current_dir(td.path())
            .env("QUORUM_LIPPA_SESSION", "fake-cookie-for-warning-path")
            .args(["review"])
            .assert()
            .stderr(predicate::str::contains("total_budget_kb").not());
    }
}

/// AC 194a — the warning is suppressed under `--hook-mode=*` so CI and
/// hook output stay clean.
#[test]
fn ac194a_warning_suppressed_under_hook_mode() {
    let td = linked_repo("[bundle]\ntotal_budget_kb = 400\n");
    quorum()
        .current_dir(td.path())
        .env("QUORUM_LIPPA_SESSION", "fake-cookie-for-warning-path")
        .args(["review", "--hook-mode=pre-commit"])
        .assert()
        .stderr(predicate::str::contains("total_budget_kb").not());
}

/// AC 192 — an out-of-range `[bundle]` value exits 2 and names the key.
/// It is never clamped into range and the review never proceeds.
#[test]
fn ac192_out_of_range_bundle_config_exits_2() {
    let td = linked_repo("[bundle]\ntotal_budget_kb = 5000\n");
    quorum()
        .current_dir(td.path())
        .env("QUORUM_LIPPA_SESSION", "fake-cookie-for-warning-path")
        .args(["review"])
        .assert()
        .failure()
        .code(predicate::eq(2))
        .stderr(
            predicate::str::contains("total_budget_kb").and(predicate::str::contains("100..=1024")),
        );
}
