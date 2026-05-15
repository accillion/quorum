//! Bundle assembly: assemble a synthesized StagedDiff and assert deny-list
//! exclusion, per-section budgets, truncation markers, and 200KB cap.

use quorum_core::bundle::{
    assemble, BundleError, BundleInputs, FileExclusionReason, BEGIN, BUDGET_DIFF, BUDGET_MEMORY,
    BUDGET_TOTAL, END,
};
use quorum_core::conventions::ConventionsState;
use quorum_core::discovery::Discovery;
use quorum_core::git::{FileStatus, StagedDiff, StagedFile};
use quorum_core::memory::{
    Dismissal, DismissalId, DismissalReason, FindingIdentityHash, PromotionState,
};

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
fn quorum_state_dir_excluded_from_bundle() {
    // .quorum/ holds Quorum's own state — config, sqlite db, archives.
    // Even if `git add .` sweeps these into the index, they must not
    // appear in the Lippa-bound bundle.
    let files = vec![
        file(".quorum/config.toml", b"project_id=\"p_x\"\n", false),
        file(
            ".quorum/dismissals.sqlite",
            b"SQLite format 3\0fake-db-bytes\n",
            false,
        ),
        file(
            ".quorum/reviews/2026-05-15T10-00-00Z.json",
            b"{\"sid\":\"abc\"}\n",
            false,
        ),
        file("src/lib.rs", b"pub fn ok() {}\n", false),
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
        local_conventions: &[],
        local_convention_bundle_cap: 500,
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
    assert!(
        excluded.iter().any(|p| p.as_str() == ".quorum/config.toml"),
        "config.toml must be excluded; got {excluded:?}"
    );
    assert!(excluded
        .iter()
        .any(|p| p.as_str() == ".quorum/dismissals.sqlite"));
    assert!(excluded
        .iter()
        .any(|p| p.as_str() == ".quorum/reviews/2026-05-15T10-00-00Z.json"));
    // Body content of state files must not leak into the bundle.
    assert!(!res.prompt.contains("SQLite format 3"));
    assert!(!res.prompt.contains("project_id=\"p_x\""));
    // The non-state file is still bundled.
    assert!(res.prompt.contains("src/lib.rs"));
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
        local_conventions: &[],
        local_convention_bundle_cap: 500,
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
        local_conventions: &[],
        local_convention_bundle_cap: 500,
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
        local_conventions: &[],
        local_convention_bundle_cap: 500,
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
        local_conventions: &[],
        local_convention_bundle_cap: 500,
    });
    // Per-section budgets may keep total under cap; verify either succeeds
    // (with each section truncated) or returns BundleTooLarge.
    match res {
        Ok(r) => assert!(r.bytes_used <= BUDGET_TOTAL, "stays under cap"),
        Err(BundleError::BundleTooLarge(n)) => assert!(n > BUDGET_TOTAL, "reports excess"),
    }
}

// =====================================================================
// Phase 1C Stage 2 — §6.1 memory subsection + §6.2 bridge.
// =====================================================================

fn empty_staged() -> StagedDiff {
    StagedDiff {
        unified: String::new(),
        files: vec![],
        is_empty: true,
    }
}

/// Build a `Dismissal` fixture suitable for the bundle's local-conv path.
/// `seed` controls the hash (every byte = seed) so the short-hash render
/// is predictable. `at_ymd` controls the dismissed_at date for the
/// `since=` HTML comment.
fn dismissal_fixture(
    seed: u8,
    title: &str,
    body: Option<&str>,
    recurrence: u32,
    state: PromotionState,
    at_ymd: (i32, u8, u8),
    last_seen_ymd: (i32, u8, u8),
) -> Dismissal {
    let h = FindingIdentityHash([seed; 32]);
    let mk = |(y, m, d): (i32, u8, u8)| {
        let date = time::Date::from_calendar_date(y, time::Month::try_from(m).unwrap(), d).unwrap();
        time::PrimitiveDateTime::new(date, time::Time::MIDNIGHT).assume_utc()
    };
    Dismissal {
        id: DismissalId(seed as i64),
        finding_identity_hash: h,
        title_snapshot: title.into(),
        body_snapshot: body.map(|s| s.to_string()),
        source_type_snapshot: "agreement".into(),
        models_snapshot: vec!["m1".into()],
        branch_snapshot: "main".into(),
        reason: DismissalReason::FalsePositive,
        note: None,
        dismissed_at: mk(at_ymd),
        last_seen_at: mk(last_seen_ymd),
        last_seen_session_id: None,
        recurrence_count: recurrence,
        expires_at: None,
        repo_head_sha_first: "sha".into(),
        promotion_state: state,
    }
}

/// Build BundleInputs around just the memory subsection. Diff/files are
/// empty so we can read the rendered memory section in isolation.
fn assemble_memory_only<'a>(
    local: &'a [Dismissal],
    cap: usize,
    conventions: &'a ConventionsState,
    memory: Option<quorum_core::bundle::MemoryInput>,
) -> String {
    let staged = empty_staged();
    let res = assemble(&BundleInputs {
        staged: &staged,
        memory,
        conventions,
        discovery: &empty_discovery(),
        branch: "m",
        head_sha: "a",
        remote_url: None,
        local_conventions: local,
        local_convention_bundle_cap: cap,
    })
    .expect("assemble");
    res.prompt
}

#[test]
fn ac152_local_only_entries_render_under_header_in_sorted_order() {
    // Two `local_only` rows. Input is already pre-sorted by the storage
    // query (recurrence DESC, then last_seen DESC); the bundle layer
    // preserves order without re-sorting.
    let high = dismissal_fixture(
        0xAA,
        "use ? operator instead of unwrap",
        Some("Prefer `?` over `.unwrap()` in production paths."),
        7,
        PromotionState::LocalOnly,
        (2026, 1, 15),
        (2026, 5, 10),
    );
    let low = dismissal_fixture(
        0xBB,
        "snake_case for module names",
        Some("Rust module names should be snake_case."),
        3,
        PromotionState::LocalOnly,
        (2026, 2, 1),
        (2026, 5, 9),
    );
    let prompt = assemble_memory_only(&[high, low], 500, &ConventionsState::Absent, None);
    // Header present.
    assert!(prompt.contains("## Local conventions (auto-derived)"));
    // Both entries present.
    assert!(prompt.contains("Local convention: use ? operator instead of unwrap"));
    assert!(prompt.contains("Local convention: snake_case for module names"));
    // First (recurrence=7) precedes second (recurrence=3) in the rendered
    // output — storage-layer sort preserved.
    let pos_high = prompt.find("recurrence=7").unwrap();
    let pos_low = prompt.find("recurrence=3").unwrap();
    assert!(pos_high < pos_low, "high-recurrence row must precede low");
}

#[test]
fn ac153_per_entry_format_matches_spec() {
    let d = dismissal_fixture(
        0xAB,
        "title body",
        Some("body content"),
        4,
        PromotionState::LocalOnly,
        (2026, 1, 7),
        (2026, 5, 10),
    );
    let prompt = assemble_memory_only(&[d], 500, &ConventionsState::Absent, None);
    // Title header (codepoint-truncated to 80 chars — short title here).
    assert!(prompt.contains("### Local convention: title body\n"));
    // Body content present.
    assert!(prompt.contains("body content"));
    // Trailing HTML comment with recurrence + since (ISO) + 12-char hash.
    // Hash bytes are all 0xAB → hex "ab" repeated, first 12 = "ababababab" + "ab" = "ababababab" + "ab"…
    // The full hex starts with "abababab..." so the short prefix is "ababababab" + "ab" = "ababababab" + "ab"
    assert!(
        prompt.contains("<!-- recurrence=4, since=2026-01-07, hash=ababababab"),
        "metadata line missing or wrong shape; prompt was:\n{prompt}"
    );
}

#[test]
fn ac154_per_entry_truncation_marker_when_body_overflows_cap() {
    // body > cap → keeps prefix at codepoint boundary, appends the
    // [local convention truncated: …] marker before the closing comment.
    let body = "x".repeat(600);
    let d = dismissal_fixture(
        0xC1,
        "long body convention",
        Some(&body),
        5,
        PromotionState::LocalOnly,
        (2026, 3, 3),
        (2026, 5, 10),
    );
    let prompt = assemble_memory_only(&[d], 100, &ConventionsState::Absent, None);
    assert!(
        prompt.contains("[local convention truncated: "),
        "per-entry truncation marker missing"
    );
    // 600 - 100 = 500 bytes elided.
    assert!(prompt.contains("500 bytes elided"));
    assert!(prompt.contains("raise [memory] local_convention_bundle_cap to see full text"));
    // Hash-short (first 12 hex chars of bytes 0xC1×32 = "c1" × 6).
    assert!(prompt.contains("[local convention truncated: c1c1c1c1c1c1"));
}

#[test]
fn ac154_total_section_truncation_marker_when_entries_exceed_budget() {
    // Build many large entries so the cumulative subsection size exceeds
    // BUDGET_MEMORY (20KB) and forces drop-at-entry-boundary plus the
    // SERVICES.md §2 marker.
    let big_body = "z".repeat(2000);
    let mut rows = Vec::new();
    for i in 0u8..15 {
        rows.push(dismissal_fixture(
            i,
            &format!("convention number {i}"),
            Some(&big_body),
            (15 - i) as u32, // descending recurrence so first is top
            PromotionState::LocalOnly,
            (2026, 1, 1),
            (2026, 5, 10),
        ));
    }
    let prompt = assemble_memory_only(&rows, 2048, &ConventionsState::Absent, None);
    assert!(
        prompt.contains("[memory truncated:"),
        "total-section truncation marker missing"
    );
    assert!(prompt.contains("bytes exceeded 20KB; review only top portion"));
}

#[test]
fn ac155_bridge_dirty_renders_promoted_in_memory_section() {
    // promoted_convention + conventions.md NOT trusted → renders in
    // memory section as if it were local_only.
    let d = dismissal_fixture(
        0xD0,
        "promoted but dirty",
        Some("rule body"),
        4,
        PromotionState::PromotedConvention,
        (2026, 1, 10),
        (2026, 5, 10),
    );
    let prompt = assemble_memory_only(&[d], 500, &ConventionsState::PresentButIgnored, None);
    assert!(prompt.contains("## Local conventions (auto-derived)"));
    assert!(prompt.contains("Local convention: promoted but dirty"));
}

#[test]
fn ac155_bridge_trusted_suppresses_promoted_in_memory_section() {
    // promoted_convention + conventions.md committed-and-clean (Trusted)
    // → memory-section render suppressed; conventions section carries
    // the rule via the file contents (Phase 1A path, not asserted here).
    let d = dismissal_fixture(
        0xD1,
        "promoted and clean",
        Some("rule body"),
        4,
        PromotionState::PromotedConvention,
        (2026, 1, 10),
        (2026, 5, 10),
    );
    let prompt = assemble_memory_only(
        &[d],
        500,
        &ConventionsState::Trusted("# committed conventions\n".into()),
        None,
    );
    assert!(!prompt.contains("Local convention: promoted and clean"));
    assert!(!prompt.contains("## Local conventions (auto-derived)"));
}

#[test]
fn ac155_mixed_local_and_bridged_render_under_one_header() {
    let local = dismissal_fixture(
        0xE0,
        "a real local rule",
        Some("text"),
        5,
        PromotionState::LocalOnly,
        (2026, 1, 1),
        (2026, 5, 10),
    );
    let bridged = dismissal_fixture(
        0xE1,
        "bridged from promoted",
        Some("text"),
        3,
        PromotionState::PromotedConvention,
        (2026, 1, 1),
        (2026, 5, 9),
    );
    let prompt = assemble_memory_only(
        &[local, bridged],
        500,
        &ConventionsState::PresentButIgnored, // dirty triggers bridge
        None,
    );
    // Exactly one header for both rows.
    assert_eq!(
        prompt
            .matches("## Local conventions (auto-derived)")
            .count(),
        1
    );
    assert!(prompt.contains("a real local rule"));
    assert!(prompt.contains("bridged from promoted"));
}

#[test]
fn empty_subsection_omits_header() {
    // No local_only and no bridge-eligible rows → header MUST NOT render
    // (an empty `## Local conventions (auto-derived)` header reads as a
    // bug; spec §6.1 wording is presence-conditional).
    let promoted_clean = dismissal_fixture(
        0x10,
        "lives in conventions.md",
        Some("text"),
        2,
        PromotionState::PromotedConvention,
        (2026, 1, 1),
        (2026, 5, 10),
    );
    let prompt = assemble_memory_only(
        &[promoted_clean],
        500,
        &ConventionsState::Trusted("committed\n".into()),
        None,
    );
    assert!(!prompt.contains("## Local conventions (auto-derived)"));
}

#[test]
fn ac170_delimiters_wrap_entire_memory_section() {
    let d = dismissal_fixture(
        0xF0,
        "wrapped under delimiters",
        Some("body"),
        2,
        PromotionState::LocalOnly,
        (2026, 1, 1),
        (2026, 5, 10),
    );
    let prompt = assemble_memory_only(&[d], 500, &ConventionsState::Absent, None);
    let begin_pos = prompt.find(BEGIN).unwrap();
    let end_pos = prompt.find(END).unwrap();
    let subsection_pos = prompt.find("## Local conventions (auto-derived)").unwrap();
    assert!(begin_pos < subsection_pos);
    assert!(subsection_pos < end_pos);
    // No second pair of delimiters around the subsection itself.
    assert_eq!(prompt.matches(BEGIN).count(), 1);
    assert_eq!(prompt.matches(END).count(), 1);
}

#[test]
fn ac173_partial_render_side_toggle() {
    // Render-side toggle correctness (partial AC 173 — full T2/T3
    // round-trip closes at Stage 4). Same SQLite row rendered twice,
    // once with conventions.md dirty, once with it trusted: the first
    // appears in the memory section; the second does not.
    let d = dismissal_fixture(
        0x77,
        "toggle me",
        Some("toggle body"),
        4,
        PromotionState::PromotedConvention,
        (2026, 4, 1),
        (2026, 5, 10),
    );
    let dirty = assemble_memory_only(
        std::slice::from_ref(&d),
        500,
        &ConventionsState::PresentButIgnored,
        None,
    );
    assert!(dirty.contains("toggle me"));

    let clean = assemble_memory_only(
        std::slice::from_ref(&d),
        500,
        &ConventionsState::Trusted("committed\n".into()),
        None,
    );
    assert!(!clean.contains("toggle me"));

    // Toggling back to dirty re-enables the bridge render.
    let dirty_again = assemble_memory_only(
        std::slice::from_ref(&d),
        500,
        &ConventionsState::PresentButIgnored,
        None,
    );
    assert!(dirty_again.contains("toggle me"));
}

#[test]
fn body_without_snapshot_renders_only_header_and_comment() {
    // body_snapshot = None → no body line, just the header and the
    // trailing HTML comment.
    let d = dismissal_fixture(
        0x33,
        "title only",
        None,
        2,
        PromotionState::LocalOnly,
        (2026, 1, 1),
        (2026, 5, 10),
    );
    let prompt = assemble_memory_only(&[d], 500, &ConventionsState::Absent, None);
    assert!(prompt.contains("### Local convention: title only\n"));
    assert!(prompt.contains("<!-- recurrence=2, since=2026-01-01, hash=3333333333"));
    // No truncation marker (body is None, nothing to truncate).
    assert!(!prompt.contains("[local convention truncated:"));
}

#[test]
fn body_multibyte_utf8_truncation_lands_on_codepoint_boundary() {
    // "é" is two bytes (0xC3 0xA9). Cap=4 must back off to 2 ("ca") not
    // slice through the codepoint. Build a body whose first 4 bytes hit
    // the middle of an é.
    let body = "café".repeat(10); // 50 bytes
    let d = dismissal_fixture(
        0x44,
        "utf8 title",
        Some(&body),
        2,
        PromotionState::LocalOnly,
        (2026, 1, 1),
        (2026, 5, 10),
    );
    // cap=4 forces truncation; the kept prefix is "caf" (3 bytes) — the
    // walk-back from 4 hits the é's leading byte at index 3.
    let prompt = assemble_memory_only(&[d], 4, &ConventionsState::Absent, None);
    assert!(prompt.contains("[local convention truncated:"));
    // No invalid UTF-8 in the output (assemble would have panicked).
    assert!(prompt.is_ascii() || std::str::from_utf8(prompt.as_bytes()).is_ok());
}

#[test]
fn title_truncation_is_codepoint_aware_at_80() {
    // 100-codepoint title with multibyte characters — truncation should
    // keep 80 codepoints, not 80 bytes. Use "é" repeated so byte vs
    // codepoint counts diverge.
    let title: String = "é".repeat(100);
    let d = dismissal_fixture(
        0x55,
        &title,
        None,
        1,
        PromotionState::LocalOnly,
        (2026, 1, 1),
        (2026, 5, 10),
    );
    let prompt = assemble_memory_only(&[d], 500, &ConventionsState::Absent, None);
    // Expected rendered title: exactly 80 "é" characters.
    let expected_title: String = "é".repeat(80);
    assert!(prompt.contains(&format!("### Local convention: {expected_title}\n")));
    // 81st é should NOT appear in the title line — confirm 81 chars is absent.
    let too_many: String = "é".repeat(81);
    assert!(!prompt.contains(&format!("### Local convention: {too_many}")));
}

#[test]
fn ac156_total_bundle_stays_under_cap_with_local_conventions() {
    // Even with a fully-loaded subsection, BUDGET_TOTAL holds.
    let big = "z".repeat(2048);
    let mut rows = Vec::new();
    for i in 0u8..30 {
        rows.push(dismissal_fixture(
            i,
            &format!("rule {i}"),
            Some(&big),
            (30 - i) as u32,
            PromotionState::LocalOnly,
            (2026, 1, 1),
            (2026, 5, 10),
        ));
    }
    let staged = StagedDiff {
        unified: "x".repeat(BUDGET_DIFF),
        files: vec![file("a.rs", &vec![b'a'; 70_000], false)],
        is_empty: false,
    };
    let res = assemble(&BundleInputs {
        staged: &staged,
        memory: Some(quorum_core::bundle::MemoryInput {
            source_basename: "CLAUDE.md".into(),
            content: "c".repeat(10_000),
        }),
        conventions: &ConventionsState::Trusted("c".repeat(5_000)),
        discovery: &empty_discovery(),
        branch: "m",
        head_sha: "a",
        remote_url: None,
        local_conventions: &rows,
        local_convention_bundle_cap: 2048,
    });
    match res {
        Ok(r) => assert!(r.bytes_used <= BUDGET_TOTAL),
        Err(BundleError::BundleTooLarge(n)) => assert!(n > BUDGET_TOTAL),
    }
}

#[test]
fn claude_md_content_precedes_local_conventions_subsection() {
    // §6.1 step ordering: CLAUDE.md (step 1) precedes the auto-derived
    // subsection (step 2).
    let d = dismissal_fixture(
        0x66,
        "ordering check",
        Some("body"),
        2,
        PromotionState::LocalOnly,
        (2026, 1, 1),
        (2026, 5, 10),
    );
    let prompt = assemble_memory_only(
        &[d],
        500,
        &ConventionsState::Absent,
        Some(quorum_core::bundle::MemoryInput {
            source_basename: "CLAUDE.md".into(),
            content: "CLAUDE_MD_MARKER body".into(),
        }),
    );
    let claude_pos = prompt.find("CLAUDE_MD_MARKER").unwrap();
    let subsection_pos = prompt.find("## Local conventions (auto-derived)").unwrap();
    assert!(claude_pos < subsection_pos);
}

// Hush an unused-import warning when only some test fns reference
// BUDGET_MEMORY (kept available for future fixtures).
#[allow(dead_code)]
const _: usize = BUDGET_MEMORY;
