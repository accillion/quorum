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

// ============ Pre-push aggregation: a blocking verdict always wins ============
//
// The fail-open hook template blocks only on exit 1. One tuple's error
// must not discard another tuple's high-severity verdict, and must not
// stop the remaining tuples from being reviewed.

const SESSIONS: &str = "/api/v1/consensus/sessions";

/// What the mock Lippa answers for one tuple's review.
#[derive(Clone, Copy, Debug)]
enum Verdict {
    /// Converges with one divergence finding: high severity, exit 1.
    High,
    /// Converges with no findings: exit 0.
    Clean,
    /// Session create answers 502: a tooling error, exit 2.
    BadGateway,
    /// Session create answers 401: an auth error, exit 3.
    Unauthorized,
}

/// A linked repo whose `main` branch has one commit, plus two side
/// branches `a` and `b` off it. Each adds a file carrying its own marker,
/// so a mock can tell the two tuples' bundles apart. Returns the repo and
/// the pre-push stdin pushing `a` then `b`.
fn two_branch_repo(base_url: &str) -> (TempDir, String) {
    use std::path::Path;
    let td = TempDir::new().unwrap();
    let repo = git2::Repository::init(td.path()).unwrap();
    let sig = git2::Signature::now("t", "t@e").unwrap();
    let mut idx = repo.index().unwrap();
    std::fs::write(td.path().join("lib.rs"), "pub fn a() {}\n").unwrap();
    idx.add_path(Path::new("lib.rs")).unwrap();
    let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
    let base = repo
        .commit(Some("HEAD"), &sig, &sig, "base", &tree, &[])
        .unwrap();
    let base_commit = repo.find_commit(base).unwrap();
    let mut stdin = String::new();
    for (branch, file) in [("a", "alpha.txt"), ("b", "bravo.txt")] {
        let blob = repo
            .blob(format!("{} marker\n", marker(branch)).as_bytes())
            .unwrap();
        let mut tb = repo.treebuilder(Some(&tree)).unwrap();
        tb.insert(file, blob, 0o100644).unwrap();
        let t = repo.find_tree(tb.write().unwrap()).unwrap();
        let tip = repo
            .commit(
                Some(&format!("refs/heads/{branch}")),
                &sig,
                &sig,
                branch,
                &t,
                &[&base_commit],
            )
            .unwrap();
        stdin.push_str(&format!(
            "refs/heads/{branch} {tip} refs/heads/{branch} {base}\n"
        ));
    }
    let qd = td.path().join(".quorum");
    std::fs::create_dir_all(&qd).unwrap();
    std::fs::write(
        qd.join("config.toml"),
        format!("project_id = \"p_1\"\nbase_url = \"{base_url}\"\n"),
    )
    .unwrap();
    (td, stdin)
}

fn marker(branch: &str) -> String {
    format!("QUORUM_TUPLE_MARKER_{}", branch.to_uppercase())
}

/// Mock Lippa's answer for the tuple pushing `branch`. The returned
/// session-create mock expects exactly one hit: asserting it proves the
/// tuple was attempted. The other mocks only need to outlive the run.
fn mock_tuple(
    server: &mut mockito::Server,
    branch: &str,
    verdict: Verdict,
) -> (mockito::Mock, Vec<mockito::Mock>) {
    let sess = format!("sess_{branch}");
    let create = server
        .mock("POST", SESSIONS)
        .match_body(mockito::Matcher::Regex(marker(branch)));
    let create = match verdict {
        Verdict::High | Verdict::Clean => create.with_status(201).with_body(format!(
            r#"{{"id":"{sess}","status":"pending","credits_reserved":3,"credits_remaining":197}}"#
        )),
        Verdict::BadGateway => create.with_status(502).with_body("bad gateway"),
        Verdict::Unauthorized => create
            .with_status(401)
            .with_body(r#"{"detail":"unauthorized"}"#),
    }
    .expect(1)
    .create();
    let mut rest = Vec::new();
    if matches!(verdict, Verdict::High | Verdict::Clean) {
        rest.push(
            server
                .mock("GET", format!("{SESSIONS}/{sess}/status").as_str())
                .with_status(200)
                .with_body(r#"{"status":"converged","rounds_completed":3,"rounds_total":3,"models_active":3,"models_dropped":0,"elapsed_seconds":4.0,"follow_up_questions":null}"#)
                .create(),
        );
        let divergence = if matches!(verdict, Verdict::High) {
            r#"[{"claim_text":"Hard-coded credential in alpha","confidence":0.9,"supported_by":["m1"]}]"#
        } else {
            "[]"
        };
        rest.push(
            server
                .mock("GET", format!("{SESSIONS}/{sess}").as_str())
                .with_status(200)
                .with_body(format!(
                    r#"{{"id":"{sess}","status":"converged","summary_text":"x","agreement":[],"divergence":{divergence},"assumptions":[],"models":[]}}"#
                ))
                .create(),
        );
    }
    (create, rest)
}

/// Push `a` then `b` through `--hook-mode=pre-push` with the given
/// verdicts. Asserts both tuples were attempted; returns the exit code
/// and stderr.
fn push_two(first: Verdict, second: Verdict) -> (Option<i32>, String) {
    let mut server = mockito::Server::new();
    let (create_a, _keep_a) = mock_tuple(&mut server, "a", first);
    let (create_b, _keep_b) = mock_tuple(&mut server, "b", second);
    let (td, stdin) = two_branch_repo(&server.url());
    let cfg = TempDir::new().unwrap();
    let out = quorum()
        .current_dir(td.path())
        .env("APPDATA", cfg.path())
        .env("XDG_CONFIG_HOME", cfg.path())
        .env_remove("QUORUM_LIPPA_EMAIL")
        .env_remove("QUORUM_LIPPA_PASSWORD")
        .env("QUORUM_LIPPA_SESSION", "SESSION-CANARY")
        .env("QUORUM_LIPPA_BASE_URL", server.url())
        .args(["review", "--hook-mode=pre-push"])
        .write_stdin(stdin)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    println!(
        "{first:?} + {second:?}: exit {:?}\n{stderr}",
        out.status.code()
    );
    create_a.assert();
    create_b.assert();
    (out.status.code(), stderr)
}

/// The ref named in a failed tuple's one-line warning.
fn warned_about(stderr: &str, branch: &str) -> bool {
    stderr
        .lines()
        .any(|l| l.contains("failed") && l.contains(&format!("refs/heads/{branch}")))
}

#[test]
fn pre_push_high_findings_then_error_blocks_with_exit_1() {
    let (code, stderr) = push_two(Verdict::High, Verdict::BadGateway);
    assert_eq!(code, Some(1), "high-severity verdict must win");
    assert!(warned_about(&stderr, "b"), "no warning naming refs/heads/b");
}

#[test]
fn pre_push_error_then_high_findings_blocks_with_exit_1() {
    let (code, stderr) = push_two(Verdict::BadGateway, Verdict::High);
    assert_eq!(code, Some(1), "high-severity verdict must win");
    assert!(warned_about(&stderr, "a"), "no warning naming refs/heads/a");
}

#[test]
fn pre_push_error_then_clean_exits_2() {
    let (code, stderr) = push_two(Verdict::BadGateway, Verdict::Clean);
    assert_eq!(code, Some(2));
    assert!(warned_about(&stderr, "a"), "no warning naming refs/heads/a");
}

#[test]
fn pre_push_auth_then_error_exits_3() {
    let (code, stderr) = push_two(Verdict::Unauthorized, Verdict::BadGateway);
    assert_eq!(code, Some(3), "auth outranks a tooling error");
    assert!(warned_about(&stderr, "a"), "no warning naming refs/heads/a");
    assert!(warned_about(&stderr, "b"), "no warning naming refs/heads/b");
}

// ===== A new-branch tuple whose merge-base lookup fails is a tooling error =====

/// Push `a` (reviewed by the mock with `verdict`) plus a new-branch tuple
/// `refs/heads/ghost` whose merge-base against `origin/HEAD` fails: its
/// local sha names no object. Returns the exit code and stderr.
fn push_with_failing_new_branch(verdict: Verdict) -> (Option<i32>, String) {
    let mut server = mockito::Server::new();
    let (create_a, _keep_a) = mock_tuple(&mut server, "a", verdict);
    let (td, stdin) = two_branch_repo(&server.url());
    let repo = git2::Repository::open(td.path()).unwrap();
    let base = repo.head().unwrap().target().unwrap();
    repo.reference("refs/remotes/origin/HEAD", base, true, "test")
        .unwrap();
    let tuple_a = stdin.lines().next().unwrap();
    let stdin = format!(
        "{tuple_a}\nrefs/heads/ghost {missing} refs/heads/ghost {zeros}\n",
        missing = "1".repeat(40),
        zeros = "0".repeat(40)
    );
    let cfg = TempDir::new().unwrap();
    let out = quorum()
        .current_dir(td.path())
        .env("APPDATA", cfg.path())
        .env("XDG_CONFIG_HOME", cfg.path())
        .env_remove("QUORUM_LIPPA_EMAIL")
        .env_remove("QUORUM_LIPPA_PASSWORD")
        .env("QUORUM_LIPPA_SESSION", "SESSION-CANARY")
        .env("QUORUM_LIPPA_BASE_URL", server.url())
        .args(["review", "--hook-mode=pre-push"])
        .write_stdin(stdin)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    println!(
        "ghost + {verdict:?}: exit {:?}\n{stderr}",
        out.status.code()
    );
    create_a.assert();
    (out.status.code(), stderr)
}

fn warned_merge_base(stderr: &str) -> bool {
    stderr
        .lines()
        .any(|l| l.contains("new-branch base resolution failed") && l.contains("refs/heads/ghost"))
}

/// The failed lookup is no longer skipped silently: it counts as a
/// tooling error (exit 2), with its warning kept. Fail-closed then
/// blocks the push; fail-open lets it through.
#[test]
fn a_failed_new_branch_merge_base_plus_an_ok_tuple_exits_2() {
    let (code, stderr) = push_with_failing_new_branch(Verdict::Clean);
    assert_eq!(code, Some(2), "a failed merge-base is a tooling error");
    assert!(
        warned_merge_base(&stderr),
        "no warning naming refs/heads/ghost"
    );
}

/// A blocking verdict from another tuple still wins.
#[test]
fn a_failed_new_branch_merge_base_plus_a_high_tuple_exits_1() {
    let (code, stderr) = push_with_failing_new_branch(Verdict::High);
    assert_eq!(code, Some(1), "high-severity verdict must win");
    assert!(
        warned_merge_base(&stderr),
        "no warning naming refs/heads/ghost"
    );
}
