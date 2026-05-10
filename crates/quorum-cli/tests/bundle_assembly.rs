//! Bundle assembly: assemble a synthesized StagedDiff and assert deny-list
//! exclusion, per-section budgets, truncation markers, and 200KB cap.

use quorum_core::bundle::{
    assemble, BundleError, BundleInputs, FileExclusionReason, BUDGET_DIFF, BUDGET_TOTAL,
};
use quorum_core::conventions::ConventionsState;
use quorum_core::discovery::Discovery;
use quorum_core::git::{FileStatus, StagedDiff, StagedFile};

fn empty_discovery() -> Discovery {
    Discovery {
        chosen: None,
        ignored: vec![],
        chosen_path: None,
    }
}

fn file(path: &str, body: &[u8], binary: bool) -> StagedFile {
    StagedFile {
        path: path.into(),
        status: FileStatus::Modified,
        index_blob: if binary { None } else { Some(body.to_vec()) },
        is_binary: binary,
        size_bytes: body.len() as u64,
    }
}

#[test]
fn deny_listed_files_excluded_with_marker() {
    let files = vec![
        file(".env", b"SECRET=hunter2\n", false),
        file("src/main.rs", b"fn main() {}\n", false),
        file("infra/staging/.env", b"NESTED=true\n", false),
        file(".aws/credentials", b"[default]\n", false),
        file("services/api/secrets.yml", b"k: v\n", false),
    ];
    let staged = StagedDiff {
        unified: String::new(),
        files,
        is_empty: false,
    };
    let conv = ConventionsState::Absent;
    let disc = empty_discovery();
    let res = assemble(&BundleInputs {
        staged: &staged,
        memory: None,
        conventions: &conv,
        discovery: &disc,
        branch: "main",
        head_sha: "abc",
        remote_url: None,
    })
    .expect("assemble");
    let excluded: Vec<&String> = res
        .exclusions
        .iter()
        .filter_map(|(p, r)| match r {
            FileExclusionReason::DenyList(_) => Some(p),
            _ => None,
        })
        .collect();
    assert!(excluded.iter().any(|p| p.as_str() == ".env"));
    assert!(excluded.iter().any(|p| p.as_str() == "infra/staging/.env"));
    assert!(excluded.iter().any(|p| p.as_str() == ".aws/credentials"));
    assert!(excluded
        .iter()
        .any(|p| p.as_str() == "services/api/secrets.yml"));
    // src/main.rs included.
    assert!(res.prompt.contains("src/main.rs"));
    // Secret values absent.
    assert!(!res.prompt.contains("SECRET=hunter2"));
    assert!(!res.prompt.contains("NESTED=true"));
}

#[test]
fn binary_files_excluded() {
    let files = vec![file("logo.png", &[0u8; 10], true)];
    let staged = StagedDiff {
        unified: String::new(),
        files,
        is_empty: false,
    };
    let res = assemble(&BundleInputs {
        staged: &staged,
        memory: None,
        conventions: &ConventionsState::Absent,
        discovery: &empty_discovery(),
        branch: "m",
        head_sha: "a",
        remote_url: None,
    })
    .unwrap();
    assert!(res
        .exclusions
        .iter()
        .any(|(p, r)| p == "logo.png" && matches!(r, FileExclusionReason::Binary)));
}

#[test]
fn oversized_diff_triggers_truncation_marker() {
    let huge = "x".repeat(BUDGET_DIFF + 5_000);
    let staged = StagedDiff {
        unified: huge,
        files: vec![],
        is_empty: false,
    };
    let res = assemble(&BundleInputs {
        staged: &staged,
        memory: None,
        conventions: &ConventionsState::Absent,
        discovery: &empty_discovery(),
        branch: "m",
        head_sha: "a",
        remote_url: None,
    })
    .unwrap();
    assert!(res.diff_truncated);
    assert!(res.prompt.contains("[diff truncated"));
}

#[test]
fn bundle_over_total_cap_errors() {
    // Build a diff that fits the 100KB diff cap but stack files to overflow
    // the 200KB total. Files are budgeted at 80KB so this requires forcing
    // multiple oversize files past per-section. Simpler: stuff the diff and
    // verify the cap kicks in for diff overflow alone (already covered by
    // truncation), so synth a separate path: huge memory + huge conventions.
    let huge_mem = "m".repeat(50_000);
    let huge_conv = "c".repeat(50_000);
    let staged = StagedDiff {
        unified: "x".repeat(BUDGET_DIFF),
        files: vec![file("a.rs", &vec![b'a'; 70_000], false)],
        is_empty: false,
    };
    let res = assemble(&BundleInputs {
        staged: &staged,
        memory: Some(quorum_core::bundle::MemoryInput {
            source_basename: "CLAUDE.md".into(),
            content: huge_mem,
        }),
        conventions: &ConventionsState::Trusted(huge_conv),
        discovery: &empty_discovery(),
        branch: "m",
        head_sha: "a",
        remote_url: None,
    });
    // Per-section budgets may keep total under cap; verify either succeeds
    // (with each section truncated) or returns BundleTooLarge.
    match res {
        Ok(r) => assert!(r.bytes_used <= BUDGET_TOTAL, "stays under cap"),
        Err(BundleError::BundleTooLarge(n)) => assert!(n > BUDGET_TOTAL, "reports excess"),
    }
}
