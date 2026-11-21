//! Phase 1B Stage 4 — non-interactive auth integration tests.
//!
//! Drives the `quorum` binary against a `mockito` Lippa server so we
//! exercise the full env-var → login → keyring round-trip without
//! touching real Lippa. The keyring is the `--no-keyring` file
//! fallback; we redirect its base dir (`APPDATA` on Windows,
//! `XDG_CONFIG_HOME` on Linux) into a tempdir so the tests are
//! hermetic against the host's OS keychain.
//!
//! ACs covered: 88, 89, 90, 91, 92, 127, 129.

use std::fs;
use std::path::Path;

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

/// Build a command rooted at `repo` with both the storage-base-dir
/// envs pointed into `cfg_dir` so the `--no-keyring` file fallback is
/// hermetic. Inherited `QUORUM_LIPPA_*` env vars are cleared so each
/// test runs from a known-empty base.
fn quorum_in(repo: &Path, cfg_dir: &Path) -> Command {
    let mut c = quorum();
    c.current_dir(repo)
        .env("APPDATA", cfg_dir) // Windows config dir
        .env("XDG_CONFIG_HOME", cfg_dir) // Linux config dir
        .env_remove("QUORUM_LIPPA_SESSION")
        .env_remove("QUORUM_LIPPA_BASE_URL")
        .env_remove("QUORUM_LIPPA_EMAIL")
        .env_remove("QUORUM_LIPPA_PASSWORD");
    c
}

#[test]
fn login_non_interactive_missing_email_exits_2() {
    // AC 89: missing required env var → exit 2 with named var.
    let td = init_repo();
    let cfg = TempDir::new().unwrap();
    let url = "http://127.0.0.1:1".to_string(); // unreachable; never contacted
    quorum_in(td.path(), cfg.path())
        .args([
            "auth",
            "login",
            "--non-interactive",
            "--no-keyring",
            "--url",
        ])
        .arg(&url)
        .env("QUORUM_LIPPA_PASSWORD", "ignored")
        // QUORUM_LIPPA_EMAIL deliberately unset
        .assert()
        .failure()
        .code(predicate::eq(2))
        .stderr(predicate::str::contains(
            "missing required env var: QUORUM_LIPPA_EMAIL",
        ));
}

#[test]
fn login_non_interactive_missing_password_exits_2() {
    // AC 89.
    let td = init_repo();
    let cfg = TempDir::new().unwrap();
    let url = "http://127.0.0.1:1".to_string();
    quorum_in(td.path(), cfg.path())
        .args([
            "auth",
            "login",
            "--non-interactive",
            "--no-keyring",
            "--url",
        ])
        .arg(&url)
        .env("QUORUM_LIPPA_EMAIL", "x@example.com")
        // QUORUM_LIPPA_PASSWORD deliberately unset
        .assert()
        .failure()
        .code(predicate::eq(2))
        .stderr(predicate::str::contains(
            "missing required env var: QUORUM_LIPPA_PASSWORD",
        ));
}

#[test]
fn login_non_interactive_empty_password_treated_as_missing() {
    // Defensive: empty-string env var still counts as missing.
    let td = init_repo();
    let cfg = TempDir::new().unwrap();
    let url = "http://127.0.0.1:1".to_string();
    quorum_in(td.path(), cfg.path())
        .args([
            "auth",
            "login",
            "--non-interactive",
            "--no-keyring",
            "--url",
        ])
        .arg(&url)
        .env("QUORUM_LIPPA_EMAIL", "x@example.com")
        .env("QUORUM_LIPPA_PASSWORD", "")
        .assert()
        .failure()
        .code(predicate::eq(2));
}

#[test]
fn login_non_interactive_happy_path_writes_session_file() {
    // AC 88 + AC 90: env-var capture → login → cookie persisted.
    // Also implicitly verifies AC 90 (TTY check skipped) since this
    // test process is non-TTY and the call must succeed regardless.
    let mut server = mockito::Server::new();
    let mock = server
        .mock("POST", "/api/v1/auth/login")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_header(
            "set-cookie",
            "session=preflight-token-xyz; Path=/; HttpOnly",
        )
        .with_body(r#"{"ok":true}"#)
        .create();

    let td = init_repo();
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
        .env("QUORUM_LIPPA_EMAIL", "alice@example.com")
        .env("QUORUM_LIPPA_PASSWORD", "hunter2-very-secret")
        .assert()
        .success();
    mock.assert();

    // Session file present at <cfg>/quorum/sessions/<host>.session.
    let host = url::Url::parse(&server.url())
        .unwrap()
        .host_str()
        .unwrap()
        .to_string();
    let port = url::Url::parse(&server.url()).unwrap().port().unwrap();
    let file_root = cfg.path().join("quorum").join("sessions");
    let files: Vec<_> = fs::read_dir(&file_root).unwrap().collect();
    assert!(
        !files.is_empty(),
        "session file written under {:?}",
        file_root
    );
    let body = fs::read_to_string(files[0].as_ref().unwrap().path()).unwrap();
    assert!(
        body.contains("preflight-token-xyz"),
        "session file should hold the cookie value; got {body:?}"
    );
    // The password value must NOT appear in any artifact.
    assert!(!body.contains("hunter2"));
    let _ = host;
    let _ = port;
}

#[test]
fn show_session_non_tty_without_y_exits_2() {
    // AC 92.
    let td = init_repo();
    let cfg = TempDir::new().unwrap();
    // Need a stored session first; spin a mock login.
    let mut server = mockito::Server::new();
    server
        .mock("POST", "/api/v1/auth/login")
        .with_status(200)
        .with_header("set-cookie", "session=visible-cookie-value; Path=/")
        .with_body("{}")
        .create();
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

    // Now try --show-session without -y; non-TTY context (test).
    quorum_in(td.path(), cfg.path())
        .args(["auth", "status", "--show-session", "--no-keyring", "--url"])
        .arg(server.url())
        .assert()
        .failure()
        .code(predicate::eq(2))
        .stderr(predicate::str::contains("non-TTY"));
}

#[test]
fn show_session_with_y_prints_cookie_value_to_stdout() {
    // AC 92.
    let td = init_repo();
    let cfg = TempDir::new().unwrap();
    let mut server = mockito::Server::new();
    server
        .mock("POST", "/api/v1/auth/login")
        .with_status(200)
        .with_header("set-cookie", "session=visible-cookie-value-123; Path=/")
        .with_body("{}")
        .create();
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

    quorum_in(td.path(), cfg.path())
        .args([
            "auth",
            "status",
            "--show-session",
            "-y",
            "--no-keyring",
            "--url",
        ])
        .arg(server.url())
        .assert()
        .success()
        .stdout(predicate::str::contains("visible-cookie-value-123"));
}

#[test]
fn show_session_no_auth_stored_exits_2() {
    let td = init_repo();
    let cfg = TempDir::new().unwrap();
    let url = "http://127.0.0.1:1".to_string();
    quorum_in(td.path(), cfg.path())
        .args([
            "auth",
            "status",
            "--show-session",
            "-y",
            "--no-keyring",
            "--url",
        ])
        .arg(&url)
        .assert()
        .failure()
        .code(predicate::eq(2))
        .stderr(predicate::str::contains("no session stored"));
}

#[test]
fn session_env_var_precedence_note_suppressed_under_hook_mode() {
    // AC 91 + P31: when BOTH the env var AND a keyring entry exist,
    // the env var wins. The precedence note appears only in
    // interactive review; suppressed under --hook-mode=*.
    let td = init_repo();
    let cfg = TempDir::new().unwrap();

    // Spin up mockito so config has a valid URL.
    let mut server = mockito::Server::new();
    // Login endpoint to populate the keyring entry.
    server
        .mock("POST", "/api/v1/auth/login")
        .with_status(200)
        .with_header("set-cookie", "session=keyring-cookie; Path=/")
        .with_body("{}")
        .create();
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

    // Link the repo to a project so review can proceed to the auth
    // step (otherwise NotLinked fires before SESSION resolution).
    quorum_in(td.path(), cfg.path())
        .args(["link", "--project", "p_x", "--url"])
        .arg(server.url())
        .assert()
        .success();

    // Mock /sessions so review tries to fire — we expect failure
    // afterward (it's fine; we just want to assert on stderr shape
    // BEFORE the network failure point).
    server
        .mock("POST", "/api/v1/consensus/sessions")
        .with_status(401)
        .create();

    // Under --hook-mode=pre-commit: no precedence note, even though
    // both auth sources are present.
    quorum_in(td.path(), cfg.path())
        .args(["review", "--hook-mode=pre-commit", "--no-keyring"])
        .env("QUORUM_LIPPA_SESSION", "env-cookie-value")
        // SEC-1: the env session needs its host pinned to be used at all.
        .env("QUORUM_LIPPA_BASE_URL", server.url())
        // Stage staged content so the bundle actually has something
        // to review (otherwise it short-circuits with "no staged
        // changes — nothing to review"). Easy: skip; the auth
        // resolution still runs and we can grep stderr for the note.
        .assert()
        // Exit may be anything; we only assert stderr shape:
        .stderr(predicate::str::contains("env var takes precedence").not());
}

#[test]
fn session_env_var_precedence_note_emitted_in_interactive_review() {
    // AC 91 / P31: interactive review surfaces the note.
    let td = init_repo();
    let cfg = TempDir::new().unwrap();

    let mut server = mockito::Server::new();
    server
        .mock("POST", "/api/v1/auth/login")
        .with_status(200)
        .with_header("set-cookie", "session=keyring-cookie; Path=/")
        .with_body("{}")
        .create();
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
    quorum_in(td.path(), cfg.path())
        .args(["link", "--project", "p_x", "--url"])
        .arg(server.url())
        .assert()
        .success();
    server
        .mock("POST", "/api/v1/consensus/sessions")
        .with_status(401)
        .create();

    quorum_in(td.path(), cfg.path())
        .args(["review", "--no-keyring"])
        .env("QUORUM_LIPPA_SESSION", "env-cookie-value")
        // SEC-1: the env session needs its host pinned to be used at all.
        .env("QUORUM_LIPPA_BASE_URL", server.url())
        .assert()
        .stderr(predicate::str::contains("env var takes precedence"));
}

#[test]
fn tracing_at_trace_level_does_not_leak_session_env() {
    // AC 127: env var values never appear in `tracing` output at any
    // RUST_LOG level. We invoke `quorum auth status --show-session
    // -y` (which prints the cookie to stdout intentionally) under
    // `RUST_LOG=trace` and assert the SECRET literal never appears
    // in stderr (where tracing writes by default).
    let td = init_repo();
    let cfg = TempDir::new().unwrap();
    let mut server = mockito::Server::new();
    server
        .mock("POST", "/api/v1/auth/login")
        .with_status(200)
        .with_header("set-cookie", "session=TRACE-LEAK-CANARY-XYZ; Path=/")
        .with_body("{}")
        .create();
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
        .env("QUORUM_LIPPA_PASSWORD", "PASSWORD-LEAK-CANARY-XYZ")
        .env("RUST_LOG", "trace")
        .assert()
        .success()
        // The canaries must not appear in stderr (tracing target).
        .stderr(predicate::str::contains("TRACE-LEAK-CANARY-XYZ").not())
        .stderr(predicate::str::contains("PASSWORD-LEAK-CANARY-XYZ").not());
}

// ---------------------------------------------------------------------------
// A refused (403) login says what went wrong, and the cookie that came with
// it is never kept. Lippa's two-factor challenge answers 403 `mfa_required`
// AND sets a pending session cookie with no `user_id` in it: signed out by
// construction, so storing it would leave a login that fails on first use.
// Bodies are Lippa's own shapes (routes/api_v1/auth.py:802-824).
// ---------------------------------------------------------------------------

const PENDING_COOKIE: &str = "PENDING-MFA-COOKIE-CANARY";

const BODY_MFA_REQUIRED: &str = r#"{"detail":{"error":"mfa_required"}}"#;
const BODY_EMAIL_NOT_VERIFIED: &str = r#"{"detail":{"error":"email_not_verified","email":"x***@example.com","message":"Please verify your email address before signing in."}}"#;
const BODY_SUSPENDED: &str = r#"{"detail":"Your account has been suspended. Contact support if you believe this is an error."}"#;
const BODY_OTHER_CODE: &str =
    r#"{"error":"csrf_origin_rejected","message":"Cross-site request refused."}"#;
const BODY_HTML: &str = "<html><body><h1>403 Forbidden</h1></body></html>";

/// `quorum auth login --non-interactive --no-keyring` against a mock whose
/// login answers 403 with `body` and sets the pending cookie. Returns the
/// assertion and the storage base dir so callers can inspect what was kept.
fn refused_login(body: &str) -> (assert_cmd::assert::Assert, TempDir) {
    let mut server = mockito::Server::new();
    let mock = server
        .mock("POST", "/api/v1/auth/login")
        .with_status(403)
        .with_header("content-type", "application/json")
        .with_header(
            "set-cookie",
            &format!("session={PENDING_COOKIE}; path=/; Max-Age=1209600; httponly; samesite=lax"),
        )
        .with_body(body)
        .create();
    let td = init_repo();
    let cfg = TempDir::new().unwrap();
    let assert = quorum_in(td.path(), cfg.path())
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
        .env("RUST_LOG", "trace")
        .assert();
    mock.assert();
    (assert, cfg)
}

/// Every file under `root` whose bytes contain `needle`.
fn files_containing(root: &Path, needle: &str) -> Vec<std::path::PathBuf> {
    let mut hits = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if String::from_utf8_lossy(&fs::read(&path).unwrap()).contains(needle) {
                hits.push(path);
            }
        }
    }
    hits
}

#[test]
fn login_non_interactive_403_mfa_required_names_the_session_pair() {
    let (assert, _cfg) = refused_login(BODY_MFA_REQUIRED);
    assert
        .failure()
        .code(predicate::eq(3))
        .stderr(predicate::str::contains(
            "This account has two-factor authentication enabled. \
             Use QUORUM_LIPPA_SESSION together with QUORUM_LIPPA_BASE_URL.",
        ));
}

#[test]
fn login_non_interactive_403_email_not_verified_says_so() {
    let (assert, _cfg) = refused_login(BODY_EMAIL_NOT_VERIFIED);
    assert
        .failure()
        .code(predicate::eq(3))
        .stderr(predicate::str::contains(
            "This account's email address is not verified.",
        ));
}

#[test]
fn login_non_interactive_403_suspended_says_so() {
    let (assert, _cfg) = refused_login(BODY_SUSPENDED);
    assert
        .failure()
        .code(predicate::eq(3))
        .stderr(predicate::str::contains("This Lippa account is suspended."));
}

#[test]
fn login_non_interactive_other_403_appends_lippas_error_code() {
    let (assert, _cfg) = refused_login(BODY_OTHER_CODE);
    assert
        .failure()
        .code(predicate::eq(3))
        .stderr(predicate::str::contains(
            "Login rejected: 403 Forbidden (csrf_origin_rejected)\n",
        ));
}

#[test]
fn login_non_interactive_unreadable_403_keeps_todays_message() {
    // Never fail harder than before: HTML from an edge proxy, an empty
    // body, and a "code" that is not a plain identifier (here it carries
    // an ANSI escape) all fall back to the message Quorum printed before.
    for body in [
        BODY_HTML,
        "",
        r#"{"detail":{"error":"not a code \u001b[31m"}}"#,
    ] {
        let (assert, _cfg) = refused_login(body);
        assert
            .failure()
            .code(predicate::eq(3))
            .stderr(predicate::str::contains("Login rejected: 403 Forbidden\n"));
    }
}

#[test]
fn login_non_interactive_403_keeps_no_cookie_anywhere() {
    for body in [
        BODY_MFA_REQUIRED,
        BODY_EMAIL_NOT_VERIFIED,
        BODY_SUSPENDED,
        BODY_OTHER_CODE,
        BODY_HTML,
    ] {
        let (assert, cfg) = refused_login(body);
        // What was kept is checked before the exit code, so a regression
        // that stores the cookie fails on that property, not on a correlate.
        let out = assert.get_output().clone();
        for (name, stream) in [("stdout", &out.stdout), ("stderr", &out.stderr)] {
            assert!(
                !String::from_utf8_lossy(stream).contains(PENDING_COOKIE),
                "pending cookie printed to {name} for body {body:?}"
            );
        }
        let kept = files_containing(cfg.path(), PENDING_COOKIE);
        assert!(
            kept.is_empty(),
            "pending cookie written to {kept:?} for body {body:?}"
        );
        assert!(
            !cfg.path().join("quorum").join("sessions").exists(),
            "a session file was created for body {body:?}"
        );
        assert.failure().code(predicate::eq(3));
    }
}
