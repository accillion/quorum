//! Quorum always sends the Consensus tier, and a 422 that names `tier`
//! gets a clear message.
//!
//! Drives the real binary against a `mockito` Lippa. The session-create
//! mock matches only a multipart body carrying the expected `tier` field;
//! any other body gets mockito's 501 and the review fails.

use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

const SESSIONS: &str = "/api/v1/consensus/sessions";

/// A repo with one commit and a staged change, linked to `base_url`, with
/// `extra` appended to `.quorum/config.toml`.
fn staged_repo(base_url: &str, extra: &str) -> TempDir {
    let td = TempDir::new().unwrap();
    let repo = git2::Repository::init(td.path()).unwrap();
    let sig = git2::Signature::now("t", "t@e").unwrap();
    let mut idx = repo.index().unwrap();
    std::fs::write(td.path().join("lib.rs"), "pub fn a() {}\n").unwrap();
    idx.add_path(Path::new("lib.rs")).unwrap();
    idx.write().unwrap();
    let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "c", &tree, &[])
        .unwrap();
    std::fs::write(td.path().join("lib.rs"), "pub fn a() -> u8 { 1 }\n").unwrap();
    idx.add_path(Path::new("lib.rs")).unwrap();
    idx.write().unwrap();
    let qd = td.path().join(".quorum");
    std::fs::create_dir_all(&qd).unwrap();
    std::fs::write(
        qd.join("config.toml"),
        format!("project_id = \"p_1\"\nbase_url = \"{base_url}\"\n{extra}"),
    )
    .unwrap();
    td
}

/// The multipart part for `tier = <value>`, as reqwest writes it.
fn tier_part(value: &str) -> mockito::Matcher {
    mockito::Matcher::Regex(format!("name=\"tier\"\r\n\r\n{value}\r\n"))
}

/// Mocks a session that converges with no findings, created only by a
/// body carrying `tier = <want>`. Returns the create mock and the rest.
fn mock_clean_session(server: &mut mockito::Server, want: &str) -> Vec<mockito::Mock> {
    vec![
        server
            .mock("POST", SESSIONS)
            .match_body(tier_part(want))
            .with_status(201)
            .with_body(
                r#"{"id":"sess_1","status":"pending","credits_reserved":3,"credits_remaining":197}"#,
            )
            .expect(1)
            .create(),
        server
            .mock("GET", format!("{SESSIONS}/sess_1/status").as_str())
            .with_status(200)
            .with_body(r#"{"status":"converged","rounds_completed":3,"rounds_total":3,"models_active":3,"models_dropped":0,"elapsed_seconds":4.0,"follow_up_questions":null}"#)
            .create(),
        server
            .mock("GET", format!("{SESSIONS}/sess_1").as_str())
            .with_status(200)
            .with_body(r#"{"id":"sess_1","status":"converged","summary_text":"x","agreement":[],"divergence":[],"assumptions":[],"models":[]}"#)
            .create(),
    ]
}

fn run(td: &TempDir, server: &mockito::Server, args: &[&str]) -> std::process::Output {
    let cfg = TempDir::new().unwrap();
    Command::cargo_bin("quorum")
        .unwrap()
        .current_dir(td.path())
        .env("APPDATA", cfg.path())
        .env("XDG_CONFIG_HOME", cfg.path())
        .env_remove("QUORUM_LIPPA_EMAIL")
        .env_remove("QUORUM_LIPPA_PASSWORD")
        .env("QUORUM_LIPPA_SESSION", "SESSION-CANARY")
        .env("QUORUM_LIPPA_BASE_URL", server.url())
        .args(args)
        .output()
        .unwrap()
}

/// The create body always carries `tier`: `high` when the config has no
/// key, the config's value otherwise, and `--tier` over the config. Hook
/// mode sends the config's value.
#[test]
fn the_create_body_always_carries_the_tier() {
    let cases: [(&str, &[&str], &str); 5] = [
        ("", &["review"], "high"),
        ("tier = \"medium\"\n", &["review"], "medium"),
        ("tier = \"low\"\n", &["review"], "low"),
        ("tier = \"medium\"\n", &["review", "--tier", "low"], "low"),
        (
            "tier = \"low\"\n",
            &["review", "--hook-mode=pre-commit"],
            "low",
        ),
    ];
    let mut wrong = Vec::new();
    for (extra, args, want) in cases {
        let mut server = mockito::Server::new();
        let mocks = mock_clean_session(&mut server, want);
        let td = staged_repo(&server.url(), extra);
        let out = run(&td, &server, args);
        if !mocks[0].matched() || out.status.code() != Some(0) {
            wrong.push(format!(
                "config {extra:?}, {args:?}: want tier={want}; create matched: {}, exit {:?}\nstderr: {}",
                mocks[0].matched(),
                out.status.code(),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n\n"));
}

/// Mocks a session create that answers 422 with `body`.
fn mock_create_422(server: &mut mockito::Server, body: &str) -> mockito::Mock {
    server
        .mock("POST", SESSIONS)
        .with_status(422)
        .with_body(body)
        .expect(1)
        .create()
}

/// A 422 whose body names `tier` says Lippa rejected the tier and shows
/// what Quorum sent. Exit 2, the same as a config error.
#[test]
fn a_422_naming_tier_says_lippa_rejected_the_tier_sent() {
    let mut server = mockito::Server::new();
    let create = mock_create_422(
        &mut server,
        r#"{"detail":"tier must be one of: low, medium, high."}"#,
    );
    let td = staged_repo(&server.url(), "tier = \"medium\"\n");
    let out = run(&td, &server, &["review"]);
    create.assert();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "stderr: {stderr}");
    assert!(
        stderr.contains("Lippa rejected the Consensus tier")
            && stderr.contains("tier = \"medium\"")
            && stderr.contains("tier must be one of: low, medium, high."),
        "stderr: {stderr}"
    );
}

/// A 422 whose body can't be parsed, or doesn't name `tier`, keeps
/// today's generic HTTP-status message.
#[test]
fn other_422_bodies_keep_the_generic_message() {
    for body in ["not json", r#"{"detail":"Prompt is too long."}"#, ""] {
        let mut server = mockito::Server::new();
        let create = mock_create_422(&mut server, body);
        let td = staged_repo(&server.url(), "");
        let out = run(&td, &server, &["review"]);
        create.assert();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{body:?}: stderr: {stderr}");
        assert!(
            stderr.contains("Lippa returned HTTP 422") && !stderr.contains("Consensus tier"),
            "{body:?}: stderr: {stderr}"
        );
    }
}
