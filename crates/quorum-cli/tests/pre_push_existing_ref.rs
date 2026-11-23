//! v0.6 spec AC 277a step 1 — pushing to a branch the remote already has.
//!
//! The ref is reviewed as its exact tree delta, `remote_sha` tree →
//! `local_sha` tree. A `remote_sha` missing from the local object store
//! is an error naming `git fetch <remote>`; identical trees pass with no
//! Lippa session.

mod support;

use support::{update_tuple, Fixture, Lippa, DEFECT};

/// A linked repo with remote `origin` (a bare repo) holding `main` at one
/// base commit.
fn with_origin() -> (Fixture, Lippa) {
    let fx = Fixture::new();
    let lippa = Lippa::new();
    fx.link(&lippa, "");
    fx.add_bare_remote("origin", "origin");
    fx.commit_file("readme.txt", "base\n", "base");
    fx.git(&["push", "-q", "origin", "main"]);
    (fx, lippa)
}

/// A force push over a remote commit this clone never fetched: the
/// delta cannot be computed, so the ref is not reviewed and the push
/// exits 2 telling the user to fetch. No Lippa session is opened.
#[test]
fn a_force_push_over_an_unfetched_remote_commit_says_to_fetch() {
    let (fx, lippa) = with_origin();
    let other = fx.path("other");
    let bare = fx.path("origin.git");
    fx.git_in(
        &fx.path(""),
        &[
            "clone",
            "-q",
            bare.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    std::fs::write(other.join("theirs.txt"), "theirs\n").unwrap();
    fx.git_in(&other, &["add", "theirs.txt"]);
    fx.git_in(&other, &["commit", "-q", "-m", "theirs"]);
    fx.git_in(&other, &["push", "-q", "origin", "main"]);
    let theirs = fx.git_in(&other, &["rev-parse", "HEAD"]);

    let mine = fx.commit_file("mine.txt", "mine\n", "mine");
    let (code, stderr) = fx.pre_push(
        &lippa,
        &["--remote=origin", "--url=x"],
        &update_tuple("main", &mine, &theirs),
    );
    assert_eq!(code, Some(2), "an unreviewable ref is a tooling error");
    assert!(
        stderr.contains("git fetch origin"),
        "the error must say to run `git fetch origin`: {stderr}"
    );
    assert!(lippa.prompts().is_empty(), "no Lippa session");
}

/// A rewind whose tree equals the remote's (the remote's last commit
/// reverted the one before it): nothing changes on the remote, so the
/// ref passes with a note and no Lippa session.
#[test]
fn a_tree_unchanged_rewind_passes_without_a_lippa_session() {
    let (fx, lippa) = with_origin();
    let base = fx.rev("HEAD");
    fx.commit_file("defect.txt", &format!("{DEFECT}\n"), "defect");
    let reverted = fx.remove_file("defect.txt", "revert defect");
    fx.git(&["push", "-q", "origin", "main"]);
    fx.git(&["reset", "-q", "--hard", &base]);

    let (code, stderr) = fx.pre_push(
        &lippa,
        &["--remote=origin", "--url=x"],
        &update_tuple("main", &base, &reverted),
    );
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stderr.contains("tree unchanged"),
        "the skip must be stated: {stderr}"
    );
    assert!(lippa.prompts().is_empty(), "no Lippa session, no spend");
}

/// A rewind past a fix brings the fixed defect back. The delta from the
/// remote's tree shows it as added again, and it blocks.
#[test]
fn a_rewind_that_reintroduces_a_fixed_defect_blocks() {
    let (fx, lippa) = with_origin();
    let broken = fx.commit_file("code.txt", &format!("{DEFECT}\n"), "broken");
    let fixed = fx.commit_file("code.txt", "fixed\n", "fix");
    fx.git(&["push", "-q", "origin", "main"]);
    fx.git(&["reset", "-q", "--hard", &broken]);

    let (code, _) = fx.pre_push(
        &lippa,
        &["--remote=origin", "--url=x"],
        &update_tuple("main", &broken, &fixed),
    );
    assert_eq!(code, Some(1), "the reintroduced defect must block");
    let prompts = lippa.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains(DEFECT));
}

/// A feature branch with `main` merged in: the delta from the remote's
/// feature tip covers the merged-in `main` change (over-review is
/// allowed) and the new feature commit.
#[test]
fn a_feature_branch_with_main_merged_in_reviews_the_whole_delta() {
    let (fx, lippa) = with_origin();
    fx.git(&["checkout", "-q", "-b", "feat"]);
    let f1 = fx.commit_file("f1.txt", "QUORUM_FEATURE_ONE\n", "f1");
    fx.git(&["push", "-q", "origin", "feat"]);
    fx.git(&["checkout", "-q", "main"]);
    fx.commit_file("m1.txt", "QUORUM_MAIN_ADVANCE\n", "m1");
    fx.git(&["checkout", "-q", "feat"]);
    fx.git(&["merge", "-q", "--no-edit", "main"]);
    let f2 = fx.commit_file("f2.txt", &format!("{DEFECT}\n"), "f2");

    let (code, _) = fx.pre_push(
        &lippa,
        &["--remote=origin", "--url=x"],
        &update_tuple("feat", &f2, &f1),
    );
    assert_eq!(code, Some(1), "the new feature commit's defect blocks");
    let prompts = lippa.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains(DEFECT));
    assert!(
        prompts[0].contains("QUORUM_MAIN_ADVANCE"),
        "the merged-in change is part of the delta"
    );
    assert!(
        !prompts[0].contains("QUORUM_FEATURE_ONE"),
        "content the remote already has is not in the delta"
    );
}

/// A force push after a rebase onto an advanced `main`, with the rebased
/// commit amended to add a defect: the delta from the old remote tip
/// re-reviews `main`'s advance and catches the amendment.
#[test]
fn a_rebase_onto_an_advanced_main_reviews_the_delta_from_the_old_tip() {
    let (fx, lippa) = with_origin();
    fx.git(&["checkout", "-q", "-b", "feat"]);
    let old_tip = fx.commit_file("f1.txt", "QUORUM_FEATURE_ONE\n", "f1");
    fx.git(&["push", "-q", "origin", "feat"]);
    fx.git(&["checkout", "-q", "main"]);
    fx.commit_file("m1.txt", "QUORUM_MAIN_ADVANCE\n", "m1");
    fx.git(&["checkout", "-q", "feat"]);
    fx.git(&["rebase", "-q", "main"]);
    std::fs::write(
        fx.work.join("f1.txt"),
        format!("QUORUM_FEATURE_ONE\n{DEFECT}\n"),
    )
    .unwrap();
    fx.git(&["commit", "-q", "-a", "--amend", "--no-edit"]);
    let new_tip = fx.rev("HEAD");

    let (code, _) = fx.pre_push(
        &lippa,
        &["--remote=origin", "--url=x"],
        &update_tuple("feat", &new_tip, &old_tip),
    );
    assert_eq!(code, Some(1), "the amended defect blocks");
    let prompts = lippa.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains(DEFECT));
    assert!(prompts[0].contains("QUORUM_MAIN_ADVANCE"));
}
