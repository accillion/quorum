//! v0.6 spec AC 277a — the pre-push hook passes the destination remote.
//!
//! Git calls the pre-push hook with the remote's name and URL as `$1` and
//! `$2`. The v4 template forwards both (`--remote`, `--url`), so Quorum
//! resolves a new branch's base against the remote the push goes to,
//! never a hardcoded `origin`.

mod support;

use support::{Fixture, Lippa, DEFECT};

/// A real `git push` of a new three-commit branch to a remote named
/// `upstream` (there is no `origin`), through the installed hook. The
/// blocking defect lands in the branch's first commit and is still at the
/// tip, so it blocks only if the hook reviewed the whole branch. 0.5.0
/// reviewed the tip commit only, missed it, and let the push through.
#[test]
fn a_new_branch_pushed_to_a_remote_not_called_origin_is_reviewed_in_full() {
    let fx = Fixture::new();
    let lippa = Lippa::new();
    fx.link(&lippa, "");
    fx.add_bare_remote("upstream", "upstream");
    fx.commit_file("readme.txt", "base\n", "base");
    fx.git(&["push", "-q", "upstream", "main"]);
    fx.git(&["remote", "set-head", "upstream", "main"]);

    fx.git(&["checkout", "-q", "-b", "feat"]);
    fx.commit_file("defect.txt", &format!("{DEFECT}\n"), "one");
    fx.commit_file("two.txt", "QUORUM_SECOND_COMMIT\n", "two");
    fx.commit_file("three.txt", "QUORUM_THIRD_COMMIT\n", "three");

    let install = std::process::Command::new(support::quorum_bin())
        .current_dir(&fx.work)
        .args(["install", "--hook=pre-push"])
        .output()
        .unwrap();
    assert!(install.status.success(), "{install:?}");

    let (pushed, stderr) = fx.git_push(&lippa, &["upstream", "feat"]);
    assert!(
        !pushed,
        "the defect in the first commit must block the push"
    );
    assert!(stderr.contains("high-severity findings present; push blocked"));
    let prompts = lippa.prompts();
    assert_eq!(prompts.len(), 1, "one review for the one pushed ref");
    for marker in [DEFECT, "QUORUM_SECOND_COMMIT", "QUORUM_THIRD_COMMIT"] {
        assert!(
            prompts[0].contains(marker),
            "the bundle lacks {marker:?}: earlier commits were not reviewed"
        );
    }
}

/// `--remote` and `--url` exist only for the hook: without
/// `--hook-mode` they are refused (exit 2).
#[test]
fn remote_and_url_flags_require_hook_mode() {
    for flag in ["--remote=origin", "--url=https://example.invalid/r.git"] {
        let out = std::process::Command::new(support::quorum_bin())
            .args(["review", flag])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{flag}: {out:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("--hook-mode"), "{flag}: {stderr}");
    }
}
