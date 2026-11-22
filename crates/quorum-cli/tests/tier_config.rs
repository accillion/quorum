//! The Consensus tier: the `tier` key in `.quorum/config.toml` and the
//! `quorum review --tier` flag.
//!
//! A bad value is refused before any network call, with exit 2 and a
//! message listing the three tiers. The flag is for manual reviews only:
//! hook mode reads the config and takes no flag.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn quorum() -> Command {
    Command::cargo_bin("quorum").expect("quorum binary built")
}

/// A git repo whose `.quorum/config.toml` has `extra` appended.
fn linked_repo(extra: &str) -> TempDir {
    let td = TempDir::new().unwrap();
    git2::Repository::init(td.path()).unwrap();
    let qd = td.path().join(".quorum");
    std::fs::create_dir_all(&qd).unwrap();
    std::fs::write(
        qd.join("config.toml"),
        format!("project_id = \"p_1\"\nbase_url = \"https://app.lippa.ai\"\n{extra}"),
    )
    .unwrap();
    td
}

fn lists_the_tiers() -> impl Predicate<str> {
    predicate::str::contains("low, medium, high")
}

/// An unknown `tier` in the config is a config error at load: exit 2, the
/// value named, the allowed values listed. It is never sent to Lippa.
#[test]
fn an_invalid_config_tier_exits_2_and_lists_the_tiers() {
    for bad in ["\"bogus\"", "\"HIGH\"", "\"\"", "3"] {
        let td = linked_repo(&format!("tier = {bad}\n"));
        quorum()
            .current_dir(td.path())
            .env("QUORUM_LIPPA_SESSION", "fake-cookie")
            .env("QUORUM_LIPPA_BASE_URL", "https://app.lippa.ai")
            .args(["review"])
            .assert()
            .failure()
            .code(predicate::eq(2))
            .stderr(predicate::str::contains("tier").and(lists_the_tiers()));
    }
}

/// An unknown `--tier` value is refused by argument parsing, exit 2, with
/// the three tiers listed.
#[test]
fn an_invalid_tier_flag_exits_2_and_lists_the_tiers() {
    let td = linked_repo("");
    quorum()
        .current_dir(td.path())
        .args(["review", "--tier", "ultra"])
        .assert()
        .failure()
        .code(predicate::eq(2))
        .stderr(lists_the_tiers());
}

/// Each tier is a valid `--tier` value: parsing gets past the flag and the
/// run stops at the next gate (here, no `quorum link`).
#[test]
fn each_tier_flag_value_is_accepted() {
    for tier in ["low", "medium", "high"] {
        let td = TempDir::new().unwrap();
        git2::Repository::init(td.path()).unwrap();
        quorum()
            .current_dir(td.path())
            .args(["review", "--tier", tier])
            .assert()
            .failure()
            .code(predicate::eq(2))
            .stderr(predicate::str::contains("quorum link"));
    }
}

/// Hook mode reads the config only; `--tier` with `--hook-mode` is refused.
#[test]
fn the_tier_flag_is_refused_in_hook_mode() {
    let td = linked_repo("");
    for mode in ["--hook-mode=pre-commit", "--hook-mode=pre-push"] {
        quorum()
            .current_dir(td.path())
            .args(["review", mode, "--tier", "low"])
            .write_stdin("")
            .assert()
            .failure()
            .code(predicate::eq(2))
            .stderr(predicate::str::contains("cannot be used with"));
    }
}
