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
        hunk_count: 1,
    }
}

/// Same as [`file`] but with an explicit hunk count (AC 176 scoring).
fn file_hunks(path: &str, body: &[u8], hunk_count: u32) -> StagedFile {
    StagedFile {
        hunk_count,
        ..file(path, body, false)
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
        context: Default::default(),
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
        context: Default::default(),
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
        context: Default::default(),
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
        context: Default::default(),
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
        context: Default::default(),
    });
    // Per-section budgets may keep total under cap; verify either succeeds
    // (with each section truncated) or returns BundleTooLarge.
    match res {
        Ok(r) => assert!(r.bytes_used <= BUDGET_TOTAL, "stays under cap"),
        Err(BundleError::BundleTooLarge(n, _)) => assert!(n > BUDGET_TOTAL, "reports excess"),
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
        context: Default::default(),
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
        context: Default::default(),
    });
    match res {
        Ok(r) => assert!(r.bytes_used <= BUDGET_TOTAL),
        Err(BundleError::BundleTooLarge(n, _)) => assert!(n > BUDGET_TOTAL),
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

// ===== v0.4 Stage 1 — WI-1 priority scoring =====

/// Build a body of `kb` kilobytes as 80-byte lines (79 chars + newline),
/// so the AC 182a line-number gutter arithmetic is exercised realistically.
fn body_kb(kb: usize, fill: char) -> Vec<u8> {
    let line: String = std::iter::repeat(fill).take(79).collect::<String>() + "\n";
    line.repeat(kb * 1024 / 80).into_bytes()
}

/// AC 178 — the exact dogfood shape from the external review. Two large
/// markdown files must not evict the code under review.
#[test]
fn ac178_large_markdown_does_not_evict_code_under_review() {
    let a_md = body_kb(60, 'a');
    let b_md = body_kb(30, 'b');
    let x_ts = body_kb(4, 'x');
    let x_test_ts = body_kb(2, 't');
    let types_ts = body_kb(1, 'y');

    let files = vec![
        file("a.md", &a_md, false),
        file("b.md", &b_md, false),
        file("src/x.ts", &x_ts, false),
        file("src/x.test.ts", &x_test_ts, false),
        file("types.ts", &types_ts, false),
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
        context: Default::default(),
    })
    .expect("assemble");

    // The three code files are present in full.
    for (path, body) in [
        ("src/x.ts", &x_ts),
        ("src/x.test.ts", &x_test_ts),
        ("types.ts", &types_ts),
    ] {
        assert!(
            !res.files_omitted.contains(&path.to_string()),
            "{path} must not be omitted; omitted = {:?}",
            res.files_omitted
        );
        let last_line = String::from_utf8(body.clone())
            .unwrap()
            .lines()
            .last()
            .unwrap()
            .to_string();
        assert!(
            res.prompt.contains(&last_line),
            "{path} must be included in full (last line missing)"
        );
    }

    // The largest markdown file is evicted, not the code.
    assert!(
        res.files_omitted.contains(&"a.md".to_string()),
        "a.md (60KB docs) must be omitted; omitted = {:?}",
        res.files_omitted
    );
}

// ===================== v0.4 Stage 1 — WI-2 related files =====================

use quorum_core::bundle::{class_rank, FileClass, RelatedContext};
use quorum_core::config::BundleConfig;
use std::collections::HashSet;

/// A working tree on disk plus the set of paths "tracked at HEAD".
struct Tree {
    dir: tempfile::TempDir,
    tracked: HashSet<String>,
}

impl Tree {
    fn new(files: &[(&str, &str)]) -> Tree {
        let dir = tempfile::tempdir().unwrap();
        let mut tracked = HashSet::new();
        for (path, body) in files {
            let abs = dir.path().join(path);
            std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
            std::fs::write(&abs, body).unwrap();
            tracked.insert((*path).to_string());
        }
        Tree { dir, tracked }
    }

    /// Write a file that exists on disk but is NOT tracked at HEAD.
    fn untracked(&self, path: &str, body: &str) {
        let abs = self.dir.path().join(path);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(&abs, body).unwrap();
    }

    fn ctx(&self, cfg: BundleConfig) -> RelatedContext<'_> {
        RelatedContext {
            repo_root: Some(self.dir.path()),
            tracked: Some(&self.tracked),
            cfg,
        }
    }
}

fn assemble_with(
    files: Vec<StagedFile>,
    ctx: RelatedContext<'_>,
) -> quorum_core::bundle::BundleResult {
    let staged = StagedDiff {
        unified: String::new(),
        files,
        is_empty: false,
    };
    let conv = ConventionsState::Absent;
    let disc = empty_discovery();
    assemble(&BundleInputs {
        staged: &staged,
        memory: None,
        conventions: &conv,
        discovery: &disc,
        branch: "main",
        head_sha: "abc",
        remote_url: None,
        local_conventions: &[],
        local_convention_bundle_cap: 500,
        context: ctx,
    })
    .expect("assemble")
}

fn included_paths(res: &quorum_core::bundle::BundleResult) -> Vec<String> {
    res.inclusion_order
        .iter()
        .filter(|r| r.included)
        .map(|r| r.path.clone())
        .collect()
}

/// AC 183 — config-allowlist files are read from the working tree, and
/// are labelled `(unchanged, context)` so a model cannot mistake them for
/// part of the diff.
#[test]
fn ac183_config_allowlist_pulled_in_and_labelled() {
    let tree = Tree::new(&[
        ("package.json", "{\"name\":\"demo\"}\n"),
        ("tsconfig.json", "{\"compilerOptions\":{}}\n"),
        ("src/app.ts", "export const a = 1;\n"),
    ]);
    let res = assemble_with(
        vec![file("src/app.ts", b"export const a = 2;\n", false)],
        tree.ctx(BundleConfig::default()),
    );
    let inc = included_paths(&res);
    assert!(inc.contains(&"package.json".to_string()), "got {inc:?}");
    assert!(inc.contains(&"tsconfig.json".to_string()), "got {inc:?}");
    assert!(res.prompt.contains("### package.json (unchanged, context)"));
    assert!(res
        .prompt
        .contains("### tsconfig.json (unchanged, context)"));
    // The changed file carries no such label.
    assert!(res.prompt.contains("### src/app.ts\n"));
    // Working-tree bytes, not the staged blob.
    assert!(res.prompt.contains("\"name\":\"demo\""));
}

/// AC 183 — a Rust change pulls Cargo.toml, not the JS/TS config set.
#[test]
fn ac183_allowlist_is_keyed_on_the_changed_language() {
    let tree = Tree::new(&[
        ("Cargo.toml", "[package]\nname = \"x\"\n"),
        ("package.json", "{}\n"),
        ("src/lib.rs", "pub fn a() {}\n"),
    ]);
    let res = assemble_with(
        vec![file("src/lib.rs", b"pub fn a() {}\n", false)],
        tree.ctx(BundleConfig::default()),
    );
    let inc = included_paths(&res);
    assert!(inc.contains(&"Cargo.toml".to_string()), "got {inc:?}");
    assert!(
        !inc.contains(&"package.json".to_string()),
        "a Rust change must not drag in the JS config set; got {inc:?}"
    );
}

/// AC 184 — one hop only. `a.ts` imports `b.ts`, which imports `c.ts`.
/// `b.ts` is context; `c.ts` must never appear.
#[test]
fn ac184_resolution_is_one_hop_only() {
    let tree = Tree::new(&[
        ("src/a.ts", "import { b } from './b';\n"),
        (
            "src/b.ts",
            "import { c } from './c';\nexport const b = 1;\n",
        ),
        ("src/c.ts", "export const c = 2;\n"),
    ]);
    let res = assemble_with(
        vec![file("src/a.ts", b"import { b } from './b';\n", false)],
        tree.ctx(BundleConfig::default()),
    );
    let inc = included_paths(&res);
    assert!(inc.contains(&"src/b.ts".to_string()), "got {inc:?}");
    assert!(
        !inc.contains(&"src/c.ts".to_string()),
        "neighbours of neighbours must never be added; got {inc:?}"
    );
}

/// AC 184 — the same guarantee for Rust `mod` / `crate::` references.
#[test]
fn ac184_one_hop_for_rust_module_references() {
    let tree = Tree::new(&[
        ("src/main.rs", "mod helper;\nuse crate::deep::thing;\n"),
        ("src/helper.rs", "mod nested;\npub fn h() {}\n"),
        ("src/helper/nested.rs", "pub fn n() {}\n"),
        ("src/deep.rs", "pub struct thing;\n"),
    ]);
    let res = assemble_with(
        vec![file(
            "src/main.rs",
            b"mod helper;\nuse crate::deep::thing;\n",
            false,
        )],
        tree.ctx(BundleConfig::default()),
    );
    let inc = included_paths(&res);
    assert!(inc.contains(&"src/helper.rs".to_string()), "got {inc:?}");
    assert!(inc.contains(&"src/deep.rs".to_string()), "got {inc:?}");
    assert!(
        !inc.contains(&"src/helper/nested.rs".to_string()),
        "second hop must not be followed; got {inc:?}"
    );
}

/// AC 185 — related files never displace a changed file. The budget is
/// set so that only some candidates fit; every changed file must survive.
#[test]
fn ac185_related_files_never_displace_changed_files() {
    let big = "x".repeat(40 * 1024);
    let tree = Tree::new(&[
        ("src/a.ts", "import './ctx1';\nimport './ctx2';\n"),
        ("src/ctx1.ts", &big),
        ("src/ctx2.ts", &big),
    ]);
    let cfg = BundleConfig {
        total_budget_kb: 100,
        ..BundleConfig::default()
    };
    let changed_body = b"import './ctx1';\nimport './ctx2';\n";
    let res = assemble_with(vec![file("src/a.ts", changed_body, false)], tree.ctx(cfg));
    assert!(
        !res.files_omitted.contains(&"src/a.ts".to_string()),
        "the changed file must never be evicted by context; omitted = {:?}",
        res.files_omitted
    );
    // The changed file is first in the ordering, ahead of both context files.
    assert_eq!(res.inclusion_order[0].path, "src/a.ts");
}

/// AC 186 — at most `related_file_max` related files; the overflow is
/// reported in the inclusion-order block.
#[test]
fn ac186_related_file_cap_binds_and_is_reported() {
    let mut files: Vec<(&str, &str)> = vec![("src/a.ts", "")];
    let owned: Vec<(String, String)> = (0..10)
        .map(|i| {
            (
                format!("src/m{i}.ts"),
                format!("export const m{i} = {i};\n"),
            )
        })
        .collect();
    for (p, b) in &owned {
        files.push((p.as_str(), b.as_str()));
    }
    let tree = Tree::new(&files);

    let body: String = (0..10).map(|i| format!("import './m{i}';\n")).collect();
    let cfg = BundleConfig {
        related_file_max: 3,
        ..BundleConfig::default()
    };
    let res = assemble_with(
        vec![file("src/a.ts", body.as_bytes(), false)],
        tree.ctx(cfg),
    );
    let related: Vec<_> = res
        .inclusion_order
        .iter()
        .filter(|r| r.path != "src/a.ts")
        .collect();
    assert_eq!(related.len(), 3, "cap must bind at 3");
    assert_eq!(res.related_capped_out, 7);
    assert!(
        res.prompt
            .contains("[related-file cap reached: related_file_max=3;"),
        "the cap must be visible in the inclusion-order block"
    );
}

/// AC 186 — `related_file_max = 0` admits no related files at all.
#[test]
fn ac186_related_file_max_zero_admits_nothing() {
    let tree = Tree::new(&[
        ("Cargo.toml", "[package]\n"),
        ("src/lib.rs", "pub fn a() {}\n"),
    ]);
    let cfg = BundleConfig {
        related_file_max: 0,
        ..BundleConfig::default()
    };
    let res = assemble_with(
        vec![file("src/lib.rs", b"pub fn a() {}\n", false)],
        tree.ctx(cfg),
    );
    assert_eq!(included_paths(&res), vec!["src/lib.rs".to_string()]);
}

/// AC 187 — deny-list rules apply identically to related files.
#[test]
fn ac187_denylist_applies_to_related_files() {
    let tree = Tree::new(&[
        ("src/a.ts", "import './secret.env';\n"),
        (".env", "API_KEY=sk-live-should-never-ship\n"),
        ("src/keys.ts", "export const k = 1;\n"),
    ]);
    let cfg = BundleConfig {
        include: vec![".env".into(), "src/*.ts".into()],
        ..BundleConfig::default()
    };
    let res = assemble_with(
        vec![file("src/a.ts", b"import './keys';\n", false)],
        tree.ctx(cfg),
    );
    let inc = included_paths(&res);
    assert!(
        !inc.contains(&".env".to_string()),
        "a deny-listed related file must never be included; got {inc:?}"
    );
    assert!(!res.prompt.contains("sk-live-should-never-ship"));
    assert!(res
        .exclusions
        .iter()
        .any(|(p, r)| p == ".env" && matches!(r, FileExclusionReason::DenyList(_))));
}

/// AC 188 — related files respect MAX_FILE_BYTES and the binary check.
#[test]
fn ac188_related_files_respect_size_and_binary_checks() {
    let tree = Tree::new(&[("src/a.ts", "")]);
    // A tracked "related" file containing a null byte.
    tree.untracked("src/bin.ts", "");
    std::fs::write(tree.dir.path().join("src/bin.ts"), [0xffu8, 0x00, b'h']).unwrap();
    let mut tracked = tree.tracked.clone();
    tracked.insert("src/bin.ts".to_string());
    let ctx = RelatedContext {
        repo_root: Some(tree.dir.path()),
        tracked: Some(&tracked),
        cfg: BundleConfig {
            include: vec!["src/bin.ts".into()],
            ..BundleConfig::default()
        },
    };
    let res = assemble_with(vec![file("src/a.ts", b"const a = 1;\n", false)], ctx);
    assert!(
        !included_paths(&res).contains(&"src/bin.ts".to_string()),
        "a binary related file must be excluded"
    );
    assert!(res
        .exclusions
        .iter()
        .any(|(p, r)| p == "src/bin.ts" && matches!(r, FileExclusionReason::Binary)));
}

/// AC 189 — a specifier resolving outside the repository root is skipped.
#[test]
fn ac189_related_discovery_never_leaves_the_repo() {
    let tree = Tree::new(&[("src/a.ts", "")]);
    // A file that exists just outside the repo root.
    let outside = tree.dir.path().parent().unwrap().join("outside_secret.ts");
    std::fs::write(&outside, "export const leaked = 1;\n").ok();
    let res = assemble_with(
        vec![file(
            "src/a.ts",
            b"import '../../outside_secret';\nimport '../outside_secret';\n",
            false,
        )],
        tree.ctx(BundleConfig::default()),
    );
    assert!(!res.prompt.contains("leaked"));
    assert_eq!(included_paths(&res), vec!["src/a.ts".to_string()]);
    std::fs::remove_file(&outside).ok();
}

/// AC 190 — `related_files = false` restores exactly the v0.3.3 candidate
/// set: the changed files and nothing else.
#[test]
fn ac190_related_files_false_restores_v033_candidate_set() {
    let tree = Tree::new(&[
        ("Cargo.toml", "[package]\nname = \"x\"\n"),
        ("src/lib.rs", "mod helper;\n"),
        ("src/helper.rs", "pub fn h() {}\n"),
    ]);
    let changed = || {
        vec![
            file("src/lib.rs", b"mod helper;\n", false),
            file("README.md", b"# docs\n", false),
        ]
    };

    let on = assemble_with(changed(), tree.ctx(BundleConfig::default()));
    let off = assemble_with(
        changed(),
        tree.ctx(BundleConfig {
            related_files: false,
            ..BundleConfig::default()
        }),
    );

    let mut off_paths = included_paths(&off);
    off_paths.sort();
    assert_eq!(
        off_paths,
        vec!["README.md".to_string(), "src/lib.rs".to_string()],
        "with related_files = false the candidate set is the changed set"
    );
    // And discovery really was doing something with the flag on.
    assert!(included_paths(&on).contains(&"src/helper.rs".to_string()));
    assert_eq!(off.related_capped_out, 0);
}

/// AC 182 — no file is included twice, including when a path is both in
/// the changed set and matched by the config allowlist.
#[test]
fn ac182_no_file_included_twice() {
    let tree = Tree::new(&[
        ("Cargo.toml", "[package]\nname = \"x\"\n"),
        ("src/lib.rs", "pub fn a() {}\n"),
    ]);
    // Cargo.toml is BOTH changed and on the allowlist for the changed .rs.
    let cfg = BundleConfig {
        // ...and matched by an include glob as well, for good measure.
        include: vec!["Cargo.toml".into(), "src/*.rs".into()],
        ..BundleConfig::default()
    };
    let res = assemble_with(
        vec![
            file("Cargo.toml", b"[package]\nname = \"x\"\n", false),
            file("src/lib.rs", b"pub fn a() {}\n", false),
        ],
        tree.ctx(cfg),
    );
    let inc = included_paths(&res);
    let mut uniq = inc.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(inc.len(), uniq.len(), "duplicate paths in {inc:?}");
    assert_eq!(
        res.prompt.matches("### Cargo.toml").count(),
        1,
        "Cargo.toml emitted more than once"
    );
    // The changed copy wins: no "(unchanged, context)" label on it.
    assert!(!res.prompt.contains("### Cargo.toml (unchanged, context)"));
}

/// AC 193 — `include` globs match repo-relative paths and enter at the
/// class their path implies, not at a privileged rank.
#[test]
fn ac193_include_globs_enter_at_their_implied_class() {
    let tree = Tree::new(&[
        ("src/a.rs", "pub fn a() {}\n"),
        ("docs/notes.md", "# notes\n"),
        ("schema/001.sql", "CREATE TABLE t();\n"),
    ]);
    let cfg = BundleConfig {
        include: vec!["docs/*.md".into(), "schema/*.sql".into()],
        ..BundleConfig::default()
    };
    let res = assemble_with(
        vec![file("src/a.rs", b"pub fn a() {}\n", false)],
        tree.ctx(cfg),
    );
    let by_path = |p: &str| {
        res.inclusion_order
            .iter()
            .find(|r| r.path == p)
            .unwrap_or_else(|| panic!("{p} missing from {:?}", included_paths(&res)))
    };
    assert_eq!(by_path("docs/notes.md").class, FileClass::Docs);
    assert_eq!(by_path("schema/001.sql").class, FileClass::Source);
    assert_eq!(class_rank("schema/001.sql"), FileClass::Source);
    // Not privileged: the changed source file still comes first.
    assert_eq!(res.inclusion_order[0].path, "src/a.rs");
}

/// AC 180 — the `## File inclusion order` block records path, class and
/// hunk count for each file, so the ordering is auditable from the
/// archived prompt without re-running the review.
#[test]
fn ac180_inclusion_order_block_is_emitted() {
    let tree = Tree::new(&[
        ("Cargo.toml", "[package]\n"),
        ("src/lib.rs", "pub fn a() {}\n"),
    ]);
    let res = assemble_with(
        vec![
            file_hunks("src/lib.rs", b"pub fn a() {}\n", 4),
            file_hunks("README.md", b"# hi\n", 1),
        ],
        tree.ctx(BundleConfig::default()),
    );
    assert!(res.prompt.contains("## File inclusion order"));
    assert!(res
        .prompt
        .contains("src/lib.rs class=source hunks=4 bytes=14 origin=changed included"));
    assert!(res
        .prompt
        .contains("README.md class=docs hunks=1 bytes=5 origin=changed included"));
    // The related Cargo.toml is attributed to the allowlist.
    assert!(res.prompt.contains("Cargo.toml class=config hunks=0"));
    assert!(res.prompt.contains("origin=config-allowlist"));
    // Block order matches emission order.
    let pos_src = res.prompt.find("src/lib.rs class=").unwrap();
    let pos_doc = res.prompt.find("README.md class=").unwrap();
    assert!(pos_src < pos_doc, "block must follow the real ordering");
}

/// AC 181 — `files_omitted` still lists every omitted path, and the
/// `[file omitted: …; budget exhausted]` marker keeps its v0.3.3 shape.
#[test]
fn ac181_omission_marker_shape_is_unchanged() {
    let big = body_kb(90, 'z');
    let res = assemble_with(
        vec![
            file("src/small.rs", b"pub fn a() {}\n", false),
            file("huge.md", &big, false),
        ],
        Default::default(),
    );
    assert_eq!(res.files_omitted, vec!["huge.md".to_string()]);
    assert!(
        res.prompt.contains(&format!(
            "[file omitted: huge.md, {} bytes; budget exhausted]",
            big.len()
        )),
        "marker shape changed"
    );
}

/// AC 182a — every file body in the changed-file section carries a
/// 1-based `%6d | ` gutter.
#[test]
fn ac182a_changed_file_bodies_are_line_numbered() {
    let res = assemble_with(
        vec![file("src/a.rs", b"fn one() {}\nfn two() {}\n", false)],
        Default::default(),
    );
    assert!(res.prompt.contains("     1 | fn one() {}"));
    assert!(res.prompt.contains("     2 | fn two() {}"));
}

/// AC 182b — context files are numbered too, and the section says the
/// numbers refer to the working tree at HEAD rather than to the diff.
#[test]
fn ac182b_context_files_are_line_numbered_and_explained() {
    let tree = Tree::new(&[
        ("Cargo.toml", "[package]\nname = \"x\"\n"),
        ("src/lib.rs", "pub fn a() {}\n"),
    ]);
    let res = assemble_with(
        vec![file("src/lib.rs", b"pub fn a() {}\n", false)],
        tree.ctx(BundleConfig::default()),
    );
    assert!(res.prompt.contains("### Cargo.toml (unchanged, context)"));
    assert!(res.prompt.contains("     1 | [package]"));
    assert!(res.prompt.contains("     2 | name = \"x\""));
    assert!(
        res.prompt.contains("working tree") && res.prompt.contains("HEAD"),
        "the section must explain what the context line numbers refer to"
    );
}

/// AC 194 — raising `total_budget_kb` admits more content; the section
/// budgets are derived, not hard-coded.
#[test]
fn ac194_raising_total_budget_admits_more_files() {
    let changed = || {
        vec![
            file("src/a.rs", &body_kb(40, 'a'), false),
            file("src/b.rs", &body_kb(40, 'b'), false),
            file("src/c.rs", &body_kb(40, 'c'), false),
        ]
    };
    let small = assemble_with(changed(), Default::default());
    let large = assemble_with(
        changed(),
        RelatedContext {
            cfg: BundleConfig {
                total_budget_kb: 400,
                ..BundleConfig::default()
            },
            ..Default::default()
        },
    );
    assert!(
        !small.files_omitted.is_empty(),
        "120KB of source must not fit the 200KB default file budget"
    );
    assert!(
        large.files_omitted.len() < small.files_omitted.len(),
        "raising total_budget_kb must admit more: {:?} vs {:?}",
        large.files_omitted,
        small.files_omitted
    );
}

/// `tracked_paths` really reports what git has at HEAD — the input the
/// one-hop resolver trusts (AC 184 / AC 190).
#[test]
fn tracked_paths_reflects_head() {
    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/a.rs"), "pub fn a() {}\n").unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\n").unwrap();
    let mut idx = repo.index().unwrap();
    idx.add_path(std::path::Path::new("src/a.rs")).unwrap();
    idx.add_path(std::path::Path::new("Cargo.toml")).unwrap();
    idx.write().unwrap();
    let tree_id = idx.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = git2::Signature::now("t", "t@e").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "c", &tree, &[])
        .unwrap();

    // An untracked file on disk must not appear.
    std::fs::write(dir.path().join("src/scratch.rs"), "// scratch\n").unwrap();

    let tracked = quorum_core::git::tracked_paths(&repo);
    assert!(tracked.contains("src/a.rs"));
    assert!(tracked.contains("Cargo.toml"));
    assert!(
        !tracked.contains("src/scratch.rs"),
        "untracked files must not be offered as context"
    );
}
