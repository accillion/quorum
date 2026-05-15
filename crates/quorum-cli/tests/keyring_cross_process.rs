//! BUG 1 regression guard — verifies that `Storage::OsKeyring` persists
//! cookies across processes.
//!
//! Without the keyring v3 platform feature flags (`apple-native`,
//! `windows-native`, `sync-secret-service`), the crate falls through to
//! its in-memory mock backend. A child process that writes to the mock
//! sees its store evaporate at exit, so the parent — a separate process
//! — cannot read the cookie back. This test exercises that exact
//! cross-process round-trip via the real `quorum` binary, and was the
//! gap that allowed BUG 1 (P0 session loss) to ship in v0.3.0.
//!
//! Linux-without-Secret-Service caveat: bare CI Linux runners often
//! lack a running `gnome-keyring` / `kwallet` daemon. We gate that
//! platform with `#[ignore]` so a missing Secret Service does not fail
//! the build; the macOS / Windows runners exercise it for real.

use assert_cmd::Command;
use quorum_lippa_client::keyring::{delete_cookie, load_cookie, Storage};
use tempfile::TempDir;

fn init_repo() -> TempDir {
    let td = TempDir::new().unwrap();
    git2::Repository::init(td.path()).unwrap();
    td
}

/// Drop guard that always removes the keyring entry on test exit so a
/// failed run never leaks a cookie into the developer's keychain.
struct KeyringCleanup(String);
impl Drop for KeyringCleanup {
    fn drop(&mut self) {
        let _ = delete_cookie(&Storage::OsKeyring, &self.0);
    }
}

#[cfg_attr(
    target_os = "linux",
    ignore = "requires a running Secret Service (gnome-keyring / kwallet); \
              enable manually with `cargo test -- --ignored`"
)]
#[test]
fn os_keyring_persists_cookie_across_processes() {
    // Mockito serves the login endpoint; mockito binds to a real
    // 127.0.0.1:port socket so the spawned child process can reach it.
    let mut server = mockito::Server::new();
    // Unique per-run cookie value so a leftover entry from a prior
    // crashed run cannot satisfy the assertion below.
    let unique_cookie = format!(
        "cross-process-cookie-{pid}-{nanos}",
        pid = std::process::id(),
        nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    server
        .mock("POST", "/api/v1/auth/login")
        .with_status(200)
        .with_header(
            "set-cookie",
            &format!("session={unique_cookie}; Path=/; HttpOnly"),
        )
        .with_body("{}")
        .create();

    let url = server.url();
    // Pre-clean any stale entry from a previously aborted run; arm
    // the post-test cleanup BEFORE any keyring write so a panic on
    // the spawn path still removes the entry.
    let _cleanup = KeyringCleanup(url.clone());
    let _ = delete_cookie(&Storage::OsKeyring, &url);

    let td = init_repo();

    // Step 1 — child process writes the cookie to the OS keyring. No
    // `--no-keyring`: the real `Storage::OsKeyring` path is exercised.
    Command::cargo_bin("quorum")
        .expect("quorum binary built")
        .current_dir(td.path())
        .env_remove("QUORUM_LIPPA_SESSION")
        .env_remove("QUORUM_LIPPA_EMAIL")
        .env_remove("QUORUM_LIPPA_PASSWORD")
        .args(["auth", "login", "--non-interactive", "--url"])
        .arg(&url)
        .env("QUORUM_LIPPA_EMAIL", "x@example.com")
        .env("QUORUM_LIPPA_PASSWORD", "p")
        .assert()
        .success();

    // Step 2 — parent (this) process reads the cookie back. Different
    // OS process → only the real backend will return the value. The
    // pre-fix mock backend lost the cookie when the child exited.
    let loaded = load_cookie(&Storage::OsKeyring, &url)
        .expect("keyring read must not error on the host platform");
    let secret = loaded.expect(
        "cookie must persist across processes; if this fails on a fixed build, the OS \
         keychain is not reachable for this user — see README build prerequisites \
         (BUG 1 regression guard)",
    );
    assert_eq!(
        secret.expose(),
        unique_cookie,
        "cross-process cookie value mismatch"
    );
}
