//! A failed archive write cannot discard a review's verdict.
//!
//! The archive write (`.quorum/reviews/<name>.json`) happens after the
//! review has printed its findings. It used `?`, so on a full disk or a
//! read-only mount a review with high-severity findings exited 2 instead
//! of 1, and under the fail-open hook template the commit went through.
//! A failed write now warns on stderr, naming the path and the error, and
//! the exit code stays the review's in every mode.
//!
//! `.quorum/reviews` is made a *file*, so the directory cannot be
//! created. Drives the real binary against a `mockito` Lippa.

use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

const SESSIONS: &str = "/api/v1/consensus/sessions";

/// A repo with two commits and a staged change on top, linked to
/// `base_url`, with `.quorum/reviews` a plain file. Returns the repo and
/// the pre-push stdin pushing the second commit over the first.
fn repo_with_unwritable_archive(base_url: &str) -> (TempDir, String) {
    let td = TempDir::new().unwrap();
    let repo = git2::Repository::init(td.path()).unwrap();
    let sig = git2::Signature::now("t", "t@e").unwrap();
    let mut idx = repo.index().unwrap();
    let mut parents: Vec<git2::Oid> = Vec::new();
    for body in ["pub fn a() {}\n", "pub fn a() -> u8 { 1 }\n"] {
        std::fs::write(td.path().join("lib.rs"), body).unwrap();
        idx.add_path(Path::new("lib.rs")).unwrap();
        idx.write().unwrap();
        let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
        let parent = parents.last().map(|p| repo.find_commit(*p).unwrap());
        let oid = repo
            .commit(
                Some("HEAD"),
                &sig,
                &sig,
                "c",
                &tree,
                &parent.iter().collect::<Vec<_>>(),
            )
            .unwrap();
        parents.push(oid);
    }
    std::fs::write(td.path().join("lib.rs"), "pub fn a() -> u8 { 2 }\n").unwrap();
    idx.add_path(Path::new("lib.rs")).unwrap();
    idx.write().unwrap();
    let qd = td.path().join(".quorum");
    std::fs::create_dir_all(&qd).unwrap();
    std::fs::write(
        qd.join("config.toml"),
        format!("project_id = \"p_1\"\nbase_url = \"{base_url}\"\n"),
    )
    .unwrap();
    std::fs::write(qd.join("reviews"), "not a directory\n").unwrap();
    let stdin = format!(
        "refs/heads/main {} refs/heads/main {}\n",
        parents[1], parents[0]
    );
    (td, stdin)
}

/// A mock Lippa whose every session converges with one divergence
/// finding, which is high severity.
fn mock_high_severity(server: &mut mockito::Server) -> Vec<mockito::Mock> {
    vec![
        server
            .mock("POST", SESSIONS)
            .with_status(201)
            .with_body(
                r#"{"id":"sess_1","status":"pending","credits_reserved":3,"credits_remaining":197}"#,
            )
            .expect_at_least(1)
            .create(),
        server
            .mock("GET", format!("{SESSIONS}/sess_1/status").as_str())
            .with_status(200)
            .with_body(r#"{"status":"converged","rounds_completed":3,"rounds_total":3,"models_active":3,"models_dropped":0,"elapsed_seconds":4.0,"follow_up_questions":null}"#)
            .create(),
        server
            .mock("GET", format!("{SESSIONS}/sess_1").as_str())
            .with_status(200)
            .with_body(r#"{"id":"sess_1","status":"converged","summary_text":"x","agreement":[],"divergence":[{"claim_text":"Hard-coded credential","confidence":0.9,"supported_by":["m1"]}],"assumptions":[],"models":[]}"#)
            .create(),
    ]
}

#[test]
fn a_failed_archive_write_keeps_the_high_severity_exit_in_every_mode() {
    let modes: [&[&str]; 4] = [
        &["review"],
        &["review", "--json"],
        &["review", "--hook-mode=pre-commit"],
        &["review", "--hook-mode=pre-push"],
    ];
    let mut wrong = Vec::new();
    for args in modes {
        let mut server = mockito::Server::new();
        let mocks = mock_high_severity(&mut server);
        let (td, stdin) = repo_with_unwritable_archive(&server.url());
        let cfg = TempDir::new().unwrap();
        let out = Command::cargo_bin("quorum")
            .unwrap()
            .current_dir(td.path())
            .env("APPDATA", cfg.path())
            .env("XDG_CONFIG_HOME", cfg.path())
            .env_remove("QUORUM_LIPPA_EMAIL")
            .env_remove("QUORUM_LIPPA_PASSWORD")
            .env("QUORUM_LIPPA_SESSION", "SESSION-CANARY")
            .env("QUORUM_LIPPA_BASE_URL", server.url())
            .args(args)
            .write_stdin(stdin)
            .output()
            .unwrap();
        mocks[0].assert();
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let warned = stderr.lines().any(|l| {
            l.contains("warning")
                && l.contains("archive")
                && l.contains(".quorum")
                && l.contains("reviews")
        });
        if out.status.code() != Some(1) || !warned {
            wrong.push(format!(
                "{args:?}: exit {:?}, archive warning: {warned}\nstderr: {stderr}",
                out.status.code()
            ));
        }
        // `--json` still prints the archive buffer to stdout.
        if args.contains(&"--json") && !stdout.contains(r#""session_id": "sess_1""#) {
            wrong.push(format!("{args:?}: no JSON buffer on stdout:\n{stdout}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n\n"));
}
