//! Embed the git short SHA into the `quorum` binary so
//! `quorum --version` reports a useful build identity.
//!
//! Source of truth (in order):
//!   1. `QUORUM_BUILD_SHA` env at build time — distribution pipelines
//!      can pin a canonical SHA without re-running `git`.
//!   2. `git rev-parse --short HEAD` from the workspace.
//!   3. The literal string `unknown` — used by crates.io tarball builds
//!      (no `.git/` shipped in the published source archive).
//!
//! We also tell Cargo to invalidate the build when HEAD or its
//! references change, so re-running `cargo build` after a commit
//! produces an updated SHA without a manual `cargo clean`.

use std::process::Command;

fn main() {
    let sha = std::env::var("QUORUM_BUILD_SHA")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            Command::new("git")
                .args(["rev-parse", "--short", "HEAD"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=GIT_SHORT_SHA={sha}");

    // Re-run on commit changes so a fresh HEAD picks up a new SHA
    // without `cargo clean`. These paths may not exist in a crates.io
    // tarball; Cargo treats missing paths as "no change".
    println!("cargo:rerun-if-env-changed=QUORUM_BUILD_SHA");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs");
}
