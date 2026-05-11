//! Phase 1B Stage 3 — `quorum install` / `uninstall` integration tests.
//!
//! Drives the binary via `assert_cmd` against tempdir-rooted git repos.
//! Covers the command-handler surface (the unit tests in
//! `src/hooks/mod.rs` already cover the lower-level logic).
//!
//! ACs covered: 72, 73, 74, 75, 76, 77, 78, 131 (0755 on Unix).

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

fn quorum() -> Command {
    Command::cargo_bin("quorum").expect("quorum binary built")
}

fn init_repo() -> TempDir {
    let td = TempDir::new().unwrap();
    git2::Repository::init(td.path()).unwrap();
    td
}

fn hook_dir(repo_root: &Path) -> std::path::PathBuf {
    let repo = git2::Repository::discover(repo_root).unwrap();
    repo.path().join("hooks")
}

#[test]
fn install_pre_commit_writes_hook_with_marker_in_first_5_lines() {
    // AC 72.
    let td = init_repo();
    quorum()
        .current_dir(td.path())
        .args(["install", "--hook=pre-commit"])
        .assert()
        .success();
    let path = hook_dir(td.path()).join("pre-commit");
    let body = fs::read_to_string(&path).expect("hook file written");
    let first_five = body.lines().take(5).collect::<Vec<_>>().join("\n");
    assert!(first_five.contains("# quorum-managed-hook"));
    assert!(body.starts_with("#!/bin/sh"));
    // Bypass message for pre-commit references `git commit`.
    assert!(body.contains("QUORUM_SKIP=1 git commit"));
}

#[test]
fn install_pre_push_writes_hook_with_marker_and_push_bypass() {
    // AC 73.
    let td = init_repo();
    quorum()
        .current_dir(td.path())
        .args(["install", "--hook=pre-push"])
        .assert()
        .success();
    let body = fs::read_to_string(hook_dir(td.path()).join("pre-push")).unwrap();
    let first_five = body.lines().take(5).collect::<Vec<_>>().join("\n");
    assert!(first_five.contains("# quorum-managed-hook"));
    assert!(body.contains("QUORUM_SKIP=1 git push"));
    assert!(body.contains("quorum review --hook-mode=pre-push"));
}

#[test]
fn install_refuses_existing_non_managed_hook_exit_2() {
    // AC 74: install refuses to overwrite a hook whose first 5 lines
    // do not contain the marker. Exit 2.
    let td = init_repo();
    let dir = hook_dir(td.path());
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("pre-commit"),
        "#!/bin/sh\n# pre-existing foreign hook\necho hi\nexit 0\n",
    )
    .unwrap();

    let out = quorum()
        .current_dir(td.path())
        .args(["install", "--hook=pre-commit"])
        .assert()
        .failure();
    out.code(predicates::ord::eq(2));

    // File must be untouched.
    let body = fs::read_to_string(dir.join("pre-commit")).unwrap();
    assert!(body.contains("foreign hook"));
}

#[test]
fn install_is_idempotent_for_managed_hook() {
    // AC 75.
    let td = init_repo();
    quorum()
        .current_dir(td.path())
        .args(["install", "--hook=pre-commit"])
        .assert()
        .success();
    // Re-install — must succeed (overwrite Quorum-managed file).
    quorum()
        .current_dir(td.path())
        .args(["install", "--hook=pre-commit"])
        .assert()
        .success();
    let body = fs::read_to_string(hook_dir(td.path()).join("pre-commit")).unwrap();
    assert!(body.contains("# quorum-managed-hook"));
}

#[test]
fn install_resolves_hook_path_via_repo_dot_path_under_worktree() {
    // AC 76: spec §4.5.1 uses repo.path().join("hooks").join(<name>);
    // this must produce the right location even when the cwd is a
    // subdirectory of the working tree.
    let td = init_repo();
    let sub = td.path().join("subdir/nested");
    fs::create_dir_all(&sub).unwrap();
    quorum()
        .current_dir(&sub)
        .args(["install", "--hook=pre-commit"])
        .assert()
        .success();
    // The hook file must land under the repo's .git/hooks, NOT under
    // the cwd subdir.
    assert!(hook_dir(td.path()).join("pre-commit").exists());
    assert!(!sub.join(".git").exists());
}

#[test]
fn install_outside_a_git_repo_exits_2() {
    // AC 77.
    let td = TempDir::new().unwrap(); // NOT git-init'd
    let out = quorum()
        .current_dir(td.path())
        .args(["install", "--hook=pre-commit"])
        .assert()
        .failure();
    out.code(predicates::ord::eq(2));
}

#[test]
fn uninstall_removes_only_managed_idempotent_on_absent() {
    // AC 78.
    let td = init_repo();
    // Absent → idempotent success.
    quorum()
        .current_dir(td.path())
        .args(["uninstall", "--hook=pre-commit"])
        .assert()
        .success();
    // Install then uninstall → removes.
    quorum()
        .current_dir(td.path())
        .args(["install", "--hook=pre-commit"])
        .assert()
        .success();
    let path = hook_dir(td.path()).join("pre-commit");
    assert!(path.exists());
    quorum()
        .current_dir(td.path())
        .args(["uninstall", "--hook=pre-commit"])
        .assert()
        .success();
    assert!(!path.exists());

    // Non-managed hook present → refuses with exit 2; file untouched.
    fs::create_dir_all(hook_dir(td.path())).unwrap();
    fs::write(&path, "#!/bin/sh\necho other\nexit 0\n").unwrap();
    let out = quorum()
        .current_dir(td.path())
        .args(["uninstall", "--hook=pre-commit"])
        .assert()
        .failure();
    out.code(predicates::ord::eq(2));
    let body = fs::read_to_string(&path).unwrap();
    assert!(body.contains("echo other"));
}

#[test]
fn unknown_hook_kind_exits_2() {
    let td = init_repo();
    let out = quorum()
        .current_dir(td.path())
        .args(["install", "--hook=pre-merge-commit"])
        .assert()
        .failure();
    out.code(predicates::ord::eq(2));
}

#[cfg(unix)]
#[test]
fn install_sets_mode_0755_on_unix() {
    // AC 131.
    use std::os::unix::fs::PermissionsExt;
    let td = init_repo();
    quorum()
        .current_dir(td.path())
        .args(["install", "--hook=pre-commit"])
        .assert()
        .success();
    let path = hook_dir(td.path()).join("pre-commit");
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755);
}
