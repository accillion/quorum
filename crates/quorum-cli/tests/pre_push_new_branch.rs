//! v0.6 spec AC 277a step 2 — pushing a branch the remote does not have.
//!
//! The base is the merge-base with a tracking ref of the destination
//! remote that the remote still advertises (`git ls-remote`), preferring
//! `<remote>/HEAD`, else the closest. With no such ref, the full tree is
//! reviewed against the empty tree, with a notice naming why. Never the
//! tip commit only.

mod support;

use std::path::Path;

use support::{new_branch_tuple, Fixture, Lippa, DEFECT};

/// Content every scenario's `main` carries, which the remote already has.
/// It reaches the bundle only when the full tree is reviewed.
const ON_REMOTE: &str = "QUORUM_ALREADY_ON_REMOTE";

/// A linked repo whose remote `remote` (a bare repo) holds `main` at one
/// commit, with `refs/remotes/<remote>/main` tracking it and no
/// `<remote>/HEAD` (a push never creates one). Returns the bare path.
fn with_remote(remote: &str) -> (Fixture, Lippa, String) {
    let fx = Fixture::new();
    let lippa = Lippa::new();
    fx.link(&lippa, "");
    let bare = fx.add_bare_remote(remote, remote.replace('/', "-").as_str());
    fx.commit_file("readme.txt", &format!("{ON_REMOTE}\n"), "base");
    fx.git(&["push", "-q", remote, "main"]);
    assert!(
        !has_ref(&fx, &format!("refs/remotes/{remote}/HEAD")),
        "the fixture must start without {remote}/HEAD"
    );
    (fx, lippa, bare.to_str().unwrap().to_string())
}

fn has_ref(fx: &Fixture, name: &str) -> bool {
    let mut cmd = std::process::Command::new("git");
    cmd.args(["rev-parse", "--verify", "-q", name])
        .current_dir(&fx.work);
    fx.git_env(&mut cmd);
    cmd.output().unwrap().status.success()
}

/// `--remote=<name> --url=<url>`, as the v4 hook passes them.
fn named(remote: &str, url: &str) -> [String; 2] {
    [format!("--remote={remote}"), format!("--url={url}")]
}

fn args(a: &[String; 2]) -> [&str; 2] {
    [a[0].as_str(), a[1].as_str()]
}

/// A new branch `feat` off `main`: a defect in its first commit, still at
/// the tip after two more. Returns the tip.
fn three_commit_branch(fx: &Fixture) -> String {
    fx.git(&["checkout", "-q", "-b", "feat"]);
    fx.commit_file("defect.txt", &format!("{DEFECT}\n"), "one");
    fx.commit_file("two.txt", "QUORUM_SECOND_COMMIT\n", "two");
    fx.commit_file("three.txt", "QUORUM_THIRD_COMMIT\n", "three")
}

fn only_prompt(lippa: &Lippa) -> String {
    let prompts = lippa.prompts();
    assert_eq!(prompts.len(), 1, "exactly one review for the one ref");
    prompts.into_iter().next().unwrap()
}

/// The 0.5.0 gap: no `origin/HEAD`, so the tip commit alone was reviewed
/// and a defect from the first commit passed. Now `origin/main`, verified
/// against the remote, gives the merge-base, and the whole branch is
/// reviewed (and nothing the remote already has).
#[test]
fn a_new_three_commit_branch_without_origin_head_is_reviewed_from_its_merge_base() {
    let (fx, lippa, url) = with_remote("origin");
    let tip = three_commit_branch(&fx);
    let (code, stderr) = fx.pre_push(
        &lippa,
        &args(&named("origin", &url)),
        &new_branch_tuple("feat", &tip),
    );
    assert_eq!(code, Some(1), "the first commit's defect must block");
    let prompt = only_prompt(&lippa);
    for marker in [DEFECT, "QUORUM_SECOND_COMMIT", "QUORUM_THIRD_COMMIT"] {
        assert!(prompt.contains(marker), "the bundle lacks {marker:?}");
    }
    assert!(!prompt.contains(ON_REMOTE), "reviewed from the merge-base");
    assert!(stderr.contains("refs/remotes/origin/main"), "{stderr}");
}

/// The same, to a remote named `up` with no `HEAD`: the destination is
/// whatever the push names, not `origin`.
#[test]
fn a_new_branch_to_a_remote_not_called_origin_uses_that_remotes_tracking_refs() {
    let (fx, lippa, url) = with_remote("up");
    let tip = three_commit_branch(&fx);
    let (code, _) = fx.pre_push(
        &lippa,
        &args(&named("up", &url)),
        &new_branch_tuple("feat", &tip),
    );
    assert_eq!(code, Some(1));
    let prompt = only_prompt(&lippa);
    assert!(prompt.contains(DEFECT));
    assert!(!prompt.contains(ON_REMOTE));
}

/// Remotes `a` and `a/b`: `refs/remotes/a/b/side` matches `a`'s fetch
/// refspec destination pattern, but remote `a` does not have it. It is
/// the closest base, and trusting it would skip the defect in `side`.
/// Only refs `a` itself advertises count. (`git remote add` now refuses
/// `a/b` beside `a`, but older Git allowed it and a hand-edited config
/// still does, so the fixture writes the config directly.)
#[test]
fn remotes_named_a_and_a_slash_b_do_not_share_tracking_refs() {
    let (fx, lippa, url_a) = with_remote("a");
    let ab = fx.path("ab.git");
    fx.git_in(
        &fx.path(""),
        &["init", "-q", "--bare", ab.to_str().unwrap()],
    );
    fx.git(&["config", "remote.a/b.url", ab.to_str().unwrap()]);
    fx.git(&[
        "config",
        "remote.a/b.fetch",
        "+refs/heads/*:refs/remotes/a/b/*",
    ]);
    fx.git(&["push", "-q", "a/b", "main"]);
    fx.git(&["checkout", "-q", "-b", "side"]);
    fx.commit_file("side1.txt", &format!("{DEFECT}\n"), "s1");
    fx.commit_file("side2.txt", "QUORUM_SIDE_TWO\n", "s2");
    fx.git(&["push", "-q", "a/b", "side"]);
    assert!(has_ref(&fx, "refs/remotes/a/b/side"));
    fx.git(&["checkout", "-q", "-b", "feat"]);
    let tip = fx.commit_file("f3.txt", "QUORUM_FEATURE_THREE\n", "f3");

    let (code, stderr) = fx.pre_push(
        &lippa,
        &args(&named("a", &url_a)),
        &new_branch_tuple("feat", &tip),
    );
    assert_eq!(code, Some(1), "the defect in side must be reviewed");
    let prompt = only_prompt(&lippa);
    assert!(prompt.contains(DEFECT));
    assert!(prompt.contains("QUORUM_FEATURE_THREE"));
    assert!(stderr.contains("refs/remotes/a/main"), "{stderr}");
}

/// A push to a raw URL (`$1` equals `$2`) has no tracking refs, so the
/// full tree is reviewed, with a notice saying why.
#[test]
fn a_url_push_reviews_the_full_tree() {
    let (fx, lippa, url) = with_remote("origin");
    let tip = three_commit_branch(&fx);
    let (code, stderr) = fx.pre_push(
        &lippa,
        &args(&named(&url, &url)),
        &new_branch_tuple("feat", &tip),
    );
    assert_eq!(code, Some(1));
    let prompt = only_prompt(&lippa);
    assert!(prompt.contains(DEFECT));
    assert!(prompt.contains(ON_REMOTE), "the full tree is reviewed");
    assert!(
        stderr.contains("full tree") && stderr.contains("URL"),
        "the notice must name the reason: {stderr}"
    );
    assert!(!stderr.contains(&url), "the URL is never printed: {stderr}");
}

/// The remote's `main` was rewound after the last fetch, so the local
/// `origin/main` (and `origin/HEAD` with it) names a commit the remote no
/// longer has, carrying a defect. Trusting it would skip the defect; the
/// stale ref is not verified, and the full tree is reviewed.
#[test]
fn a_stale_tracking_ref_is_not_trusted() {
    let (fx, lippa, url) = with_remote("origin");
    let base = fx.rev("HEAD");
    fx.commit_file("m1.txt", &format!("{DEFECT}\n"), "m1");
    fx.git(&["push", "-q", "origin", "main"]);
    fx.git(&["remote", "set-head", "origin", "main"]);
    // Rewind the remote behind this clone's back.
    let bare = Path::new(&url);
    fx.git_in(bare, &["update-ref", "refs/heads/main", &base]);
    fx.git(&["checkout", "-q", "-b", "feat"]);
    let tip = fx.commit_file("f1.txt", "QUORUM_FEATURE_ONE\n", "f1");

    let (code, stderr) = fx.pre_push(
        &lippa,
        &args(&named("origin", &url)),
        &new_branch_tuple("feat", &tip),
    );
    assert_eq!(code, Some(1), "the defect the remote lacks must block");
    let prompt = only_prompt(&lippa);
    assert!(prompt.contains(DEFECT));
    assert!(prompt.contains(ON_REMOTE), "the full tree is reviewed");
    assert!(stderr.contains("git fetch origin"), "{stderr}");
}

/// A new branch at a commit `origin/main` already contains adds nothing
/// to the remote: it passes with a note and no Lippa session.
#[test]
fn a_new_branch_at_a_commit_a_tracking_ref_contains_needs_no_session() {
    let (fx, lippa, url) = with_remote("origin");
    let older = fx.commit_file("m1.txt", "QUORUM_MAIN_ONE\n", "m1");
    fx.commit_file("m2.txt", "QUORUM_MAIN_TWO\n", "m2");
    fx.git(&["push", "-q", "origin", "main"]);
    fx.git(&["branch", "old", &older]);

    let (code, stderr) = fx.pre_push(
        &lippa,
        &args(&named("origin", &url)),
        &new_branch_tuple("old", &older),
    );
    assert_eq!(code, Some(0), "{stderr}");
    assert!(lippa.prompts().is_empty(), "no Lippa session, no spend");
    assert!(stderr.contains("nothing to review"), "{stderr}");
}

/// An orphan branch shares no history with the remote: its full tree is
/// reviewed, with a notice.
#[test]
fn an_orphan_branch_reviews_its_full_tree() {
    let (fx, lippa, url) = with_remote("origin");
    fx.git(&["checkout", "-q", "--orphan", "solo"]);
    fx.git(&["rm", "-rq", "--cached", "."]);
    std::fs::remove_file(fx.work.join("readme.txt")).unwrap();
    fx.commit_file("solo1.txt", &format!("{DEFECT}\n"), "solo1");
    let tip = fx.commit_file("solo2.txt", "QUORUM_SOLO_TWO\n", "solo2");

    let (code, stderr) = fx.pre_push(
        &lippa,
        &args(&named("origin", &url)),
        &new_branch_tuple("solo", &tip),
    );
    assert_eq!(code, Some(1));
    let prompt = only_prompt(&lippa);
    assert!(prompt.contains(DEFECT) && prompt.contains("QUORUM_SOLO_TWO"));
    assert!(stderr.contains("orphan"), "{stderr}");
}

/// Criss-cross history: `feat` and `origin/main` have two merge-bases.
/// No single base is safe, so the full tree is reviewed.
#[test]
fn criss_cross_merge_bases_fall_back_to_the_full_tree() {
    let (fx, lippa, url) = with_remote("origin");
    let a = fx.rev("HEAD");
    let b1 = fx.commit_file("b1.txt", "QUORUM_B1\n", "b1");
    fx.git(&["checkout", "-q", "-b", "y", &a]);
    let c1 = fx.commit_file("c1.txt", "QUORUM_C1\n", "c1");
    fx.git(&["checkout", "-q", "main"]);
    fx.git(&["merge", "-q", "--no-ff", "--no-edit", &c1]);
    fx.git(&["push", "-q", "origin", "main"]);
    fx.git(&["checkout", "-q", "y"]);
    fx.git(&["merge", "-q", "--no-ff", "--no-edit", &b1]);
    let bases = fx.git(&["merge-base", "--all", "HEAD", "origin/main"]);
    assert_eq!(bases.lines().count(), 2, "the fixture must be criss-cross");
    fx.git(&["checkout", "-q", "-b", "feat"]);
    let tip = fx.commit_file("f.txt", &format!("{DEFECT}\n"), "f");

    let (code, stderr) = fx.pre_push(
        &lippa,
        &args(&named("origin", &url)),
        &new_branch_tuple("feat", &tip),
    );
    assert_eq!(code, Some(1));
    let prompt = only_prompt(&lippa);
    assert!(prompt.contains(ON_REMOTE), "the full tree is reviewed");
    assert!(stderr.contains("criss-cross"), "{stderr}");
}

/// A v1–v3 hook runs `quorum review --hook-mode=pre-push` with no remote
/// argument. Quorum does not guess `origin`, even with `origin/HEAD`
/// present: it reviews the full tree and says to reinstall the hook.
#[test]
fn an_old_hook_without_the_remote_argument_reviews_the_full_tree_with_a_notice() {
    let (fx, lippa, _) = with_remote("origin");
    fx.git(&["remote", "set-head", "origin", "main"]);
    let tip = three_commit_branch(&fx);

    let (code, stderr) = fx.pre_push(&lippa, &[], &new_branch_tuple("feat", &tip));
    assert_eq!(code, Some(1));
    let prompt = only_prompt(&lippa);
    assert!(prompt.contains(DEFECT));
    assert!(prompt.contains(ON_REMOTE), "the full tree is reviewed");
    assert!(
        stderr.contains("reinstall the hook (`quorum install --hook=pre-push`) for exact coverage"),
        "{stderr}"
    );
}

/// The remote's tracking refs cannot be checked (`git ls-remote` fails:
/// the remote is gone). Nothing is verified, so the full tree is reviewed.
#[test]
fn an_unreachable_remote_leaves_nothing_verified() {
    let (fx, lippa, _) = with_remote("origin");
    let gone = fx.path("gone.git");
    fx.git(&["remote", "set-url", "origin", gone.to_str().unwrap()]);
    let tip = three_commit_branch(&fx);

    let (code, stderr) = fx.pre_push(
        &lippa,
        &args(&named("origin", gone.to_str().unwrap())),
        &new_branch_tuple("feat", &tip),
    );
    assert_eq!(code, Some(1));
    assert!(only_prompt(&lippa).contains(ON_REMOTE));
    assert!(stderr.contains("ls-remote"), "{stderr}");
}

/// In a shallow clone merge-bases are unreliable: the full tree is
/// reviewed. (A `.git/shallow` naming the root commit is what a depth-1
/// clone of it writes.)
#[test]
fn a_shallow_clone_reviews_the_full_tree() {
    let (fx, lippa, url) = with_remote("origin");
    let root = fx.rev("HEAD");
    let tip = three_commit_branch(&fx);
    std::fs::write(fx.work.join(".git").join("shallow"), format!("{root}\n")).unwrap();

    let (code, stderr) = fx.pre_push(
        &lippa,
        &args(&named("origin", &url)),
        &new_branch_tuple("feat", &tip),
    );
    assert_eq!(code, Some(1));
    assert!(only_prompt(&lippa).contains(ON_REMOTE));
    assert!(stderr.contains("shallow"), "{stderr}");
}

/// Two verified tracking refs, `main` and `dev` (`feat` branches from
/// `dev`). With `origin/HEAD` → `main` eligible, `main` is the base, so
/// `dev`'s own commit is over-reviewed.
#[test]
fn origin_head_is_preferred_when_eligible() {
    let (fx, lippa, url) = with_remote("origin");
    fx.git(&["remote", "set-head", "origin", "main"]);
    let tip = branch_off_dev(&fx);
    let (code, stderr) = fx.pre_push(
        &lippa,
        &args(&named("origin", &url)),
        &new_branch_tuple("feat", &tip),
    );
    assert_eq!(code, Some(0), "{stderr}");
    let prompt = only_prompt(&lippa);
    assert!(prompt.contains("QUORUM_DEV_ONLY"), "based on main");
    assert!(prompt.contains("QUORUM_FEATURE_ONE"));
    assert!(stderr.contains("refs/remotes/origin/main"), "{stderr}");
}

/// The same refs without `origin/HEAD`: the closest eligible ref, `dev`,
/// is the base. Both of `feat`'s commits are reviewed, `dev`'s is not.
#[test]
fn without_origin_head_the_closest_verified_ref_is_the_base() {
    let (fx, lippa, url) = with_remote("origin");
    let tip = branch_off_dev(&fx);
    let (code, stderr) = fx.pre_push(
        &lippa,
        &args(&named("origin", &url)),
        &new_branch_tuple("feat", &tip),
    );
    assert_eq!(code, Some(0), "{stderr}");
    let prompt = only_prompt(&lippa);
    assert!(!prompt.contains("QUORUM_DEV_ONLY"), "based on dev");
    assert!(prompt.contains("QUORUM_FEATURE_ONE"), "not tip-only");
    assert!(prompt.contains("QUORUM_FEATURE_TWO"));
    assert!(stderr.contains("refs/remotes/origin/dev"), "{stderr}");
}

/// `dev` = `main` + one commit, pushed; `feat` = `dev` + two commits.
fn branch_off_dev(fx: &Fixture) -> String {
    fx.git(&["checkout", "-q", "-b", "dev"]);
    fx.commit_file("dev.txt", "QUORUM_DEV_ONLY\n", "dev");
    fx.git(&["push", "-q", "origin", "dev"]);
    fx.git(&["checkout", "-q", "-b", "feat"]);
    fx.commit_file("f1.txt", "QUORUM_FEATURE_ONE\n", "f1");
    fx.commit_file("f2.txt", "QUORUM_FEATURE_TWO\n", "f2")
}

/// A full-tree review cut by the bundle budget would pass content nobody
/// reviewed. In 0.5.1 it exits 2 before any Lippa session, saying how to
/// review the branch instead.
#[test]
fn a_truncated_full_tree_review_exits_2_without_a_session() {
    let (fx, lippa, url) = with_remote("origin");
    fx.git(&["checkout", "-q", "--orphan", "big"]);
    let line = "let value = compute_something_reasonably_long(argument_one, argument_two);\n";
    let tip = fx.commit_file("big.txt", &line.repeat(4000), "big");

    let (code, stderr) = fx.pre_push(
        &lippa,
        &args(&named("origin", &url)),
        &new_branch_tuple("big", &tip),
    );
    assert_eq!(code, Some(2), "{stderr}");
    assert!(lippa.prompts().is_empty(), "no Lippa session, no spend");
    assert!(
        stderr.contains(
            "new-branch review exceeded the bundle budget; push a smaller branch or \
             review it with `quorum review --range <base>..<head>`"
        ),
        "{stderr}"
    );
}

/// Two verified tracking refs, `b-lane` (created first) and `a-lane`, are
/// equally close: `feat` merges both lanes and adds one commit, so each
/// lane's merge-base is three commits from the tip. With no `origin/HEAD`
/// the lowest ref name, `refs/remotes/origin/a-lane`, is the base. Its
/// own commit is on the remote and stays out of the bundle; `b-lane`'s
/// arrives through the merge and is reviewed.
#[test]
fn equally_close_tracking_refs_tie_break_by_the_lowest_ref_name() {
    let (fx, lippa, url) = with_remote("origin");
    for (lane, marker) in [("b-lane", "QUORUM_LANE_B"), ("a-lane", "QUORUM_LANE_A")] {
        fx.git(&["checkout", "-q", "-b", lane, "main"]);
        fx.commit_file(&format!("{lane}.txt"), &format!("{marker}\n"), lane);
        fx.git(&["push", "-q", "origin", lane]);
    }
    fx.git(&["checkout", "-q", "-b", "feat", "b-lane"]);
    fx.git(&["merge", "-q", "--no-ff", "--no-edit", "a-lane"]);
    let tip = fx.commit_file("f.txt", "QUORUM_FEATURE_ONE\n", "f");
    for lane in ["a-lane", "b-lane"] {
        let distance = fx.git(&["rev-list", "--count", &format!("origin/{lane}..feat")]);
        assert_eq!(distance, "3", "{lane}: the fixture must be a tie");
    }

    let (code, stderr) = fx.pre_push(
        &lippa,
        &args(&named("origin", &url)),
        &new_branch_tuple("feat", &tip),
    );
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stderr.contains("refs/remotes/origin/a-lane"), "{stderr}");
    let prompt = only_prompt(&lippa);
    assert!(!prompt.contains("QUORUM_LANE_A"), "based on a-lane");
    assert!(
        prompt.contains("QUORUM_LANE_B"),
        "b-lane's commit is reviewed"
    );
    assert!(prompt.contains("QUORUM_FEATURE_ONE"));
}
