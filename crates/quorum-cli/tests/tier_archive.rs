//! The tier Quorum sent is recorded in the review archive and shown in
//! the markdown header beside the model list, as `(tier: high)`.
//!
//! Drives the real binary against a `mockito` Lippa. Archives written
//! before the tier existed have no `tier` key. Nothing in Quorum reads
//! archives back, so the no-tier rendering is covered by the unit tests
//! in `render.rs` and `quorum-core`'s `archive.rs`.

use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

const SESSIONS: &str = "/api/v1/consensus/sessions";

fn staged_repo(base_url: &str) -> TempDir {
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
        format!("project_id = \"p_1\"\nbase_url = \"{base_url}\"\n"),
    )
    .unwrap();
    td
}

fn mock_session(server: &mut mockito::Server) -> Vec<mockito::Mock> {
    vec![
        server
            .mock("POST", SESSIONS)
            .with_status(201)
            .with_body(
                r#"{"id":"sess_1","status":"pending","credits_reserved":3,"credits_remaining":197}"#,
            )
            .create(),
        server
            .mock("GET", format!("{SESSIONS}/sess_1/status").as_str())
            .with_status(200)
            .with_body(r#"{"status":"converged","rounds_completed":3,"rounds_total":3,"models_active":3,"models_dropped":0,"elapsed_seconds":4.0,"follow_up_questions":null}"#)
            .create(),
        server
            .mock("GET", format!("{SESSIONS}/sess_1").as_str())
            .with_status(200)
            .with_body(r#"{"id":"sess_1","status":"converged","tier":"high","summary_text":"x","agreement":[],"divergence":[],"assumptions":[],"models":[{"id":"m-a","vendor":"openai"},{"id":"m-b","vendor":"claude"},{"id":"m-c","vendor":"google"}]}"#)
            .create(),
    ]
}

/// Runs `quorum <args>`; returns stdout and the one archive written.
fn review(args: &[&str]) -> (String, serde_json::Value) {
    let mut server = mockito::Server::new();
    let _mocks = mock_session(&mut server);
    let td = staged_repo(&server.url());
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
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{args:?}: stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let archives: Vec<_> = std::fs::read_dir(td.path().join(".quorum").join("reviews"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(archives.len(), 1, "{archives:?}");
    let archive = serde_json::from_slice(&std::fs::read(&archives[0]).unwrap()).unwrap();
    (stdout, archive)
}

/// The archive's `tier` is the tier sent (`high` by default, the flag's
/// value with `--tier`), and the header shows it after the models.
#[test]
fn the_archive_and_header_record_the_tier_sent() {
    let mut wrong = Vec::new();
    for (args, want) in [
        (&["review"][..], "high"),
        (&["review", "--tier", "low"][..], "low"),
        (&["review", "--tier", "medium"][..], "medium"),
    ] {
        let (stdout, archive) = review(args);
        if archive.get("tier").and_then(|t| t.as_str()) != Some(want) {
            wrong.push(format!(
                "{args:?}: archive tier {:?}, want {want:?}",
                archive.get("tier")
            ));
        }
        let header = stdout.lines().nth(1).unwrap_or_default();
        let expected = format!("Reviewed by claude, google, openai (tier: {want}) in ");
        if !header.starts_with(&expected) {
            wrong.push(format!(
                "{args:?}: header {header:?}, want it to start {expected:?}"
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// `--json` prints the same buffer, so it carries `tier` too.
#[test]
fn json_stdout_carries_the_tier() {
    let (stdout, _archive) = review(&["review", "--json", "--tier", "low"]);
    assert!(stdout.contains(r#""tier": "low""#), "{stdout}");
}
