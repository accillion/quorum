//! SEC-1 — `base_url` must be https, and `QUORUM_LIPPA_SESSION` must not
//! follow a repo-chosen host.
//!
//! `base_url` comes only from the repository's own `.quorum/config.toml`.
//! The keyring path already fails closed (entries are host-scoped), but the
//! env-var cookie was seeded into the client's jar at whatever host the repo
//! named. A repo committing `base_url = "http://evil.example"` received the
//! live session plus the whole bundle in cleartext, and hook mode fires
//! that on `git push`. https alone does not close it (a certificate for an
//! attacker's domain is free), so the env cookie also requires
//! `QUORUM_LIPPA_BASE_URL` to name the same host: no pin, no env cookie.
//!
//! Drives the real binary against a `mockito` Lippa. Storage is the
//! `--no-keyring` file fallback with its base dir redirected into a tempdir
//! (same pattern as `non_interactive_auth.rs`).

use std::path::Path;

use assert_cmd::Command;
use mockito::Matcher;
use predicates::prelude::*;
use tempfile::TempDir;

const SESSIONS: &str = "/api/v1/consensus/sessions";

/// A `quorum` command rooted at `repo`, hermetic against the host: file
/// storage under `cfg_dir`, and every inherited `QUORUM_LIPPA_*` cleared.
fn quorum_in(repo: &Path, cfg_dir: &Path) -> Command {
    let mut c = Command::cargo_bin("quorum").expect("quorum binary built");
    c.current_dir(repo)
        .env("APPDATA", cfg_dir)
        .env("XDG_CONFIG_HOME", cfg_dir)
        .env_remove("QUORUM_LIPPA_SESSION")
        .env_remove("QUORUM_LIPPA_BASE_URL")
        .env_remove("QUORUM_LIPPA_EMAIL")
        .env_remove("QUORUM_LIPPA_PASSWORD");
    c
}

/// A repo with one commit, one staged change (so `quorum review` gets as
/// far as the network), and `.quorum/config.toml` naming `base_url`.
/// The config is written directly, as a cloned repo would carry it.
fn linked_repo(base_url: &str) -> TempDir {
    let td = TempDir::new().unwrap();
    let repo = git2::Repository::init(td.path()).unwrap();
    std::fs::write(td.path().join("lib.rs"), "pub fn a() {}\n").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(Path::new("lib.rs")).unwrap();
    idx.write().unwrap();
    let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("t", "t@e").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "base", &tree, &[])
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

/// Layer 1 — an `http://` base_url on a non-loopback host is a config error
/// (exit 2) at load, for every command that reads the config, before any
/// network is attempted. Under a hook it is still exit 2, which the shell
/// template fails open on by default: the push proceeds, nothing is sent.
#[test]
fn sec1_http_non_loopback_base_url_is_rejected_for_every_command() {
    for bad in ["http://evil.example", "http://app.lippa.ai:8080/"] {
        let td = linked_repo(bad);
        let cfg = TempDir::new().unwrap();
        quorum_in(td.path(), cfg.path())
            .args(["review"])
            .env("QUORUM_LIPPA_SESSION", "SESSION-CANARY")
            .assert()
            .failure()
            .code(2)
            .stderr(
                predicate::str::contains("config error")
                    .and(predicate::str::contains("https"))
                    .and(predicate::str::contains("network error").not()),
            );
        quorum_in(td.path(), cfg.path())
            .args(["review", "--hook-mode=pre-commit"])
            .env("QUORUM_LIPPA_SESSION", "SESSION-CANARY")
            .assert()
            .code(2)
            .stderr(predicate::str::contains("config error"));
        quorum_in(td.path(), cfg.path())
            .args(["link", "--show"])
            .assert()
            .failure()
            .code(2)
            .stderr(predicate::str::contains("https"));
    }
}

/// Layer 1 — plain http stays legal on an explicit loopback host, so local
/// Lippa dev servers and test doubles keep working.
#[test]
fn sec1_http_loopback_base_url_is_accepted() {
    for ok in [
        "http://localhost:8000",
        "http://127.0.0.1:1",
        "http://[::1]:1",
        "https://staging.lippa.example",
    ] {
        let td = linked_repo(ok);
        let cfg = TempDir::new().unwrap();
        quorum_in(td.path(), cfg.path())
            .args(["link", "--show"])
            .assert()
            .success()
            .stdout(predicate::str::contains(ok));
    }
}

/// Layer 2 — with `QUORUM_LIPPA_BASE_URL` set, a repo whose base_url names a
/// different host gets nothing: exit 2, both hosts named, server untouched.
#[test]
fn sec1_mismatched_pin_refuses_the_env_cookie() {
    let mut server = mockito::Server::new();
    let create = server
        .mock("POST", SESSIONS)
        .with_status(401)
        .expect(0)
        .create();
    let td = linked_repo(&server.url());
    let cfg = TempDir::new().unwrap();
    quorum_in(td.path(), cfg.path())
        .args(["review"])
        .env("QUORUM_LIPPA_SESSION", "SESSION-CANARY")
        .env("QUORUM_LIPPA_BASE_URL", "https://app.lippa.ai")
        .assert()
        .failure()
        .code(2)
        .stderr(
            predicate::str::contains("127.0.0.1")
                .and(predicate::str::contains("app.lippa.ai"))
                .and(predicate::str::contains("QUORUM_LIPPA_BASE_URL")),
        );
    create.assert();
}

/// Layer 2 — a matching pin lets the env cookie through, silently.
#[test]
fn sec1_matching_pin_allows_the_env_cookie() {
    let mut server = mockito::Server::new();
    let create = server
        .mock("POST", SESSIONS)
        .match_header("cookie", Matcher::Regex("session=SESSION-CANARY".into()))
        .with_status(401)
        .expect(1)
        .create();
    let td = linked_repo(&server.url());
    let cfg = TempDir::new().unwrap();
    quorum_in(td.path(), cfg.path())
        .args(["review"])
        .env("QUORUM_LIPPA_SESSION", "SESSION-CANARY")
        // Scheme and path are irrelevant to the pin; only the host is.
        .env("QUORUM_LIPPA_BASE_URL", format!("{}/ignored", server.url()))
        .assert()
        // The mock answers 401, so reaching it at all means exit 3.
        .code(3)
        .stderr(predicate::str::contains("QUORUM_LIPPA_BASE_URL").not());
    create.assert();
}

/// Layer 2 — no pin, no env cookie. An unset `QUORUM_LIPPA_BASE_URL` is
/// refused exactly like a mismatched one (exit 2, the variable and the
/// repo-named host in the message, server untouched), and in every mode.
/// Hook mode is the attack path: a hostile clone's config fires on
/// `git push` with no human reading stderr, so a warning is no defence.
/// An empty pin, as CI renders an unset secret, counts as unset.
#[test]
fn sec1_unpinned_env_cookie_is_refused_in_every_mode() {
    let mut server = mockito::Server::new();
    let create = server
        .mock("POST", SESSIONS)
        .with_status(401)
        .expect(0)
        .create();
    let td = linked_repo(&server.url());
    let push_tuple = commit_staged_then_stage_again(td.path());
    let cfg = TempDir::new().unwrap();
    let mut not_refused = Vec::new();
    for pin in [None, Some("")] {
        for mode in [
            None,
            Some("--hook-mode=pre-commit"),
            Some("--hook-mode=pre-push"),
        ] {
            let mut cmd = quorum_in(td.path(), cfg.path());
            cmd.arg("review")
                .args(mode)
                .env("QUORUM_LIPPA_SESSION", "SESSION-CANARY");
            if let Some(p) = pin {
                cmd.env("QUORUM_LIPPA_BASE_URL", p);
            }
            if mode == Some("--hook-mode=pre-push") {
                cmd.write_stdin(push_tuple.clone());
            }
            let out = cmd.output().unwrap();
            let stderr = String::from_utf8_lossy(&out.stderr);
            let refused = out.status.code() == Some(2)
                && stderr.contains("QUORUM_LIPPA_BASE_URL is not set")
                && stderr.contains("127.0.0.1")
                && !stderr.contains("SESSION-CANARY");
            if !refused {
                not_refused.push(format!(
                    "pin={pin:?} mode={mode:?}: exit {:?}, stderr: {stderr}",
                    out.status.code()
                ));
            }
        }
    }
    // Print every miss before the mock's request count, so a failure shows
    // both what each mode did and how many sessions reached the server.
    for miss in &not_refused {
        println!("{miss}");
    }
    create.assert();
    assert!(not_refused.is_empty(), "unpinned env session not refused");
}

/// Commit what `linked_repo` staged, then stage a further change, so every
/// mode has something to send: a staged diff for interactive and
/// pre-commit, a pushed commit for pre-push. Returns the pre-push stdin
/// tuple for pushing that commit over its parent.
fn commit_staged_then_stage_again(dir: &Path) -> String {
    let repo = git2::Repository::open(dir).unwrap();
    let parent = repo.head().unwrap().peel_to_commit().unwrap();
    let mut idx = repo.index().unwrap();
    let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("t", "t@e").unwrap();
    let head = repo
        .commit(Some("HEAD"), &sig, &sig, "change", &tree, &[&parent])
        .unwrap();
    std::fs::write(dir.join("lib.rs"), "pub fn a() -> u8 { 2 }\n").unwrap();
    idx.add_path(Path::new("lib.rs")).unwrap();
    idx.write().unwrap();
    format!("refs/heads/main {head} refs/heads/main {}\n", parent.id())
}

/// Layer 2 — a pin that is not a URL is an error, never silently ignored.
#[test]
fn sec1_unparseable_pin_is_an_error() {
    let mut server = mockito::Server::new();
    let create = server.mock("POST", SESSIONS).expect(0).create();
    let td = linked_repo(&server.url());
    let cfg = TempDir::new().unwrap();
    quorum_in(td.path(), cfg.path())
        .args(["review"])
        .env("QUORUM_LIPPA_SESSION", "SESSION-CANARY")
        .env("QUORUM_LIPPA_BASE_URL", "app.lippa.ai")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("QUORUM_LIPPA_BASE_URL"));
    create.assert();
}

/// The keyring path is unchanged: it was already host-scoped, and the pin
/// governs only the env-var cookie, so a pin naming another host does not
/// stop a stored session for the configured host.
#[test]
fn sec1_keyring_path_is_unchanged_by_the_pin() {
    let mut server = mockito::Server::new();
    let _login = server
        .mock("POST", "/api/v1/auth/login")
        .with_status(200)
        .with_header("set-cookie", "session=KEYRING-COOKIE; Path=/")
        .with_body("{}")
        .create();
    let td = linked_repo(&server.url());
    let cfg = TempDir::new().unwrap();
    quorum_in(td.path(), cfg.path())
        .args([
            "auth",
            "login",
            "--non-interactive",
            "--no-keyring",
            "--url",
        ])
        .arg(server.url())
        .env("QUORUM_LIPPA_EMAIL", "x@example.com")
        .env("QUORUM_LIPPA_PASSWORD", "p")
        .assert()
        .success();
    let create = server
        .mock("POST", SESSIONS)
        .match_header("cookie", Matcher::Regex("session=KEYRING-COOKIE".into()))
        .with_status(401)
        .expect(1)
        .create();
    quorum_in(td.path(), cfg.path())
        .args(["review", "--no-keyring"])
        .env("QUORUM_LIPPA_BASE_URL", "https://app.lippa.ai")
        .assert()
        .code(3)
        .stderr(predicate::str::contains("QUORUM_LIPPA_BASE_URL").not());
    create.assert();
}
