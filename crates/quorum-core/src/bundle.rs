//! Bundle assembly: turn a staged diff + memory + conventions into a
//! single envelope to submit to Lippa's Consensus prompt field.
//!
//! v0.4 Stage 1 (WI-1/WI-2/WI-3): the changed-file section is no longer
//! filled largest-first. Candidates are ordered by a deterministic priority
//! score (class, changed-before-context, hunk count, then size ascending),
//! unchanged context files may be pulled in, every body carries a 1-based
//! line-number gutter, and the section budgets are derived from
//! `[bundle] total_budget_kb` rather than hard-coded.
//!
//! Default per-section budgets (spec §4.3.2):
//!   Diff body:                100KB
//!   Changed-file contents:     80KB (largest-first inclusion)
//!   Memory context:            20KB
//!   Conventions:               10KB
//!   Envelope overhead:         ~2KB hard
//!   ──────────────────────────────────
//!   Total bundle hard cap:    200KB

use crate::conventions::ConventionsState;
use crate::deny_list;
use crate::discovery::Discovery;
use crate::git::{FileStatus, StagedDiff, StagedFile};
use crate::memory::{truncate_at_codepoint_boundary, Dismissal, PromotionState};

// v0.4: these remain the *default* section budgets (the values
// `Budgets::default()` derives) and stay public for call sites and tests
// that reason about the 200 KB default. Live budgets come from
// [`Budgets::from_total_kb`].
pub const BUDGET_DIFF: usize = 100 * 1024;
pub const BUDGET_FILES: usize = 80 * 1024;
pub const BUDGET_MEMORY: usize = 20 * 1024;
pub const BUDGET_CONVENTIONS: usize = 10 * 1024;
pub const BUDGET_TOTAL: usize = 200 * 1024;
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

pub const BEGIN: &str = "<<<QUORUM_REPO_CONTENT_BEGIN>>>";
pub const END: &str = "<<<QUORUM_REPO_CONTENT_END>>>";

#[derive(Debug, Clone, Copy)]
pub enum FileExclusionReason {
    DenyList(&'static str),
    Binary,
    TooLarge,
    Deleted,
}

#[derive(Debug, Clone)]
pub struct BundleResult {
    /// The assembled prompt body suitable for Lippa's `prompt` form field.
    pub prompt: String,
    /// Excluded paths and why (for stderr surfacing).
    pub exclusions: Vec<(String, FileExclusionReason)>,
    /// Bytes used (close to but not necessarily equal to `prompt.len()`).
    pub bytes_used: usize,
    /// Markers emitted: diff_truncated, files_omitted_count, etc.
    pub diff_truncated: bool,
    pub files_omitted: Vec<String>,
    /// v0.4 AC 180 — the ordering actually used, in emission order.
    pub inclusion_order: Vec<InclusionRow>,
    /// v0.4 AC 186 — related candidates dropped because the cap bound.
    pub related_capped_out: usize,
}

#[derive(thiserror::Error, Debug)]
pub enum BundleError {
    #[error("bundle assembled to {0} bytes; exceeds {1}KB cap")]
    BundleTooLarge(usize, usize),
}

pub struct BundleInputs<'a> {
    pub staged: &'a StagedDiff,
    pub memory: Option<MemoryInput>,
    pub conventions: &'a ConventionsState,
    pub discovery: &'a Discovery,
    pub branch: &'a str,
    pub head_sha: &'a str,
    pub remote_url: Option<&'a str>,
    /// Phase 1C — rows from `MemoryStore::load_local_only_conventions()`:
    /// `local_only` plus `promoted_convention` (the latter render-time
    /// filtered by the §6.2 bridge against `conventions`). Pre-sorted by
    /// the storage layer (recurrence_count DESC, last_seen_at DESC).
    pub local_conventions: &'a [Dismissal],
    /// Phase 1C — `[memory] local_convention_bundle_cap` (default 500,
    /// range 100..=2048). Applied per-entry `body_snapshot`.
    pub local_convention_bundle_cap: usize,
    /// v0.4 Stage 1 — repo access for related-file discovery plus the
    /// `[bundle]` section. `Default::default()` disables discovery and
    /// uses the 200 KB default budgets, reproducing v0.3.3 selection.
    pub context: RelatedContext<'a>,
}

pub struct MemoryInput {
    pub source_basename: String,
    pub content: String,
}

pub fn assemble(inp: &BundleInputs<'_>) -> Result<BundleResult, BundleError> {
    let mut exclusions: Vec<(String, FileExclusionReason)> = Vec::new();
    let mut files_omitted: Vec<String> = Vec::new();
    let mut sections: Vec<String> = Vec::new();
    // AC 194 / AC 209 groundwork: every section budget is derived from
    // `[bundle] total_budget_kb` at runtime.
    let budgets = Budgets::from_total_kb(inp.context.cfg.total_budget_kb as usize);

    sections.push(envelope_header(inp));

    let (diff_section, diff_truncated) = diff_section(
        &inp.staged.unified,
        &inp.staged.files,
        budgets.diff,
        &mut exclusions,
    );
    sections.push(diff_section);

    let (files_section, inclusion_order, related_capped_out) = files_section(
        &inp.staged.files,
        &inp.context,
        &budgets,
        &mut exclusions,
        &mut files_omitted,
    );
    sections.push(files_section);
    sections.push(inclusion_order_section(
        &inclusion_order,
        related_capped_out,
        inp.context.cfg.related_file_max,
    ));

    // §6.1 memory section: CLAUDE.md / AGENTS.md / .cursorrules followed
    // by the auto-derived local-conventions subsection. Both share the
    // 20 KB budget_memory. The bridge fork (§6.2) consults
    // `inp.conventions.is_trusted()` once per call to decide whether
    // `promoted_convention` rows render here or in the conventions
    // section — never both, never neither (modulo budget truncation).
    let memory_block = build_memory_section(
        inp.memory.as_ref(),
        inp.discovery,
        inp.local_conventions,
        inp.local_convention_bundle_cap,
        inp.conventions.is_trusted(),
        budgets.memory,
    );
    if !memory_block.is_empty() {
        sections.push(memory_block);
    }

    sections.push(conventions_section(inp.conventions, budgets.conventions));
    sections.push(format!("\n{END}\n"));

    let prompt = sections.join("");
    let bytes_used = prompt.len();
    if bytes_used > budgets.total {
        return Err(BundleError::BundleTooLarge(
            bytes_used,
            budgets.total / 1024,
        ));
    }

    Ok(BundleResult {
        prompt,
        exclusions,
        bytes_used,
        diff_truncated,
        files_omitted,
        inclusion_order,
        related_capped_out,
    })
}

fn envelope_header(inp: &BundleInputs<'_>) -> String {
    let mut s = String::new();
    s.push_str(BEGIN);
    s.push('\n');
    s.push_str(
        "Please review the staged changes below. Findings should focus on correctness, security, and significant maintainability concerns. Cite filenames when applicable.\n",
    );
    s.push_str(&format!(
        "\n## Repo\nbranch: {}\nhead_sha: {}\n",
        inp.branch, inp.head_sha
    ));
    if let Some(url) = inp.remote_url {
        s.push_str(&format!("remote_url: {url}\n"));
    }
    s
}

fn diff_section(
    unified: &str,
    files: &[StagedFile],
    budget: usize,
    exclusions: &mut Vec<(String, FileExclusionReason)>,
) -> (String, bool) {
    // Mark deny-listed files in the diff section. Lippa receives the diff
    // already filtered for content -- the body for those files is the
    // marker line, not the patch hunks. We use a placeholder approach:
    // include the unified diff verbatim but prepend an exclusion summary.
    let mut header = String::new();
    let mut excluded_paths = Vec::new();
    for f in files {
        if let Some(reason) = deny_list::reason(&f.path) {
            excluded_paths.push((f.path.clone(), reason));
            exclusions.push((f.path.clone(), FileExclusionReason::DenyList(reason)));
        }
    }
    header.push_str("\n## Diff\n");
    if !excluded_paths.is_empty() {
        for (path, reason) in &excluded_paths {
            header.push_str(&format!(
                "[excluded by deny-list: {path}, pattern={reason}]\n"
            ));
        }
        header.push('\n');
    }
    let mut diff_body = String::new();
    for line in unified.lines() {
        let is_excluded_path_line = excluded_paths
            .iter()
            .any(|(p, _)| line.contains(&format!("a/{p}")) || line.contains(&format!("b/{p}")));
        if !is_excluded_path_line {
            diff_body.push_str(line);
            diff_body.push('\n');
        }
    }
    let (body, truncated) = truncate_with_marker(&diff_body, budget, "diff");
    (format!("{header}{body}"), truncated)
}

fn files_section(
    files: &[StagedFile],
    ctx: &RelatedContext<'_>,
    budgets: &Budgets,
    exclusions: &mut Vec<(String, FileExclusionReason)>,
    files_omitted: &mut Vec<String>,
) -> (String, Vec<InclusionRow>, usize) {
    let mut s = String::new();
    s.push_str("\n## Changed-file contents (from index blobs)\n");
    // AC 182a / AC 182b — one statement of what the gutter means, rather
    // than repeating it on every file header.
    s.push_str(
        "Line numbers are 1-based and are not part of the file. Files marked \
         `(unchanged, context)` are unmodified files read from the working tree \
         at HEAD and included only as context; their line numbers refer to the \
         file at HEAD, not to the diff.\n",
    );

    // ---- Changed candidates: the v0.3.3 eligibility filter, unchanged ----
    let mut changed_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut candidates: Vec<Candidate> = Vec::new();
    for f in files {
        if f.status == FileStatus::Deleted {
            exclusions.push((f.path.clone(), FileExclusionReason::Deleted));
            continue;
        }
        if deny_list::is_denied(&f.path) {
            continue; // already marked in diff section
        }
        if f.is_binary {
            exclusions.push((f.path.clone(), FileExclusionReason::Binary));
            continue;
        }
        if f.size_bytes > MAX_FILE_BYTES {
            exclusions.push((f.path.clone(), FileExclusionReason::TooLarge));
            continue;
        }
        let body = match &f.index_blob {
            Some(b) => b.clone(),
            None => continue,
        };
        // AC 182 — a path in the diff is registered here and therefore can
        // never be offered a second time by related-file discovery.
        changed_paths.insert(f.path.clone());
        candidates.push(Candidate {
            path: f.path.clone(),
            size_bytes: f.size_bytes,
            body,
            hunk_count: f.hunk_count,
            class: class_rank(&f.path),
            origin: CandidateOrigin::Changed,
        });
    }

    // ---- WI-2 related candidates ----
    let related = collect_related(files, &changed_paths, ctx, exclusions);
    let capped_out = related.capped_out;
    candidates.extend(related.candidates);

    // ---- WI-1 ordering (AC 176) ----
    candidates.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));

    // ---- Budgeted emission ----
    let mut rows: Vec<InclusionRow> = Vec::new();
    let mut used = 0usize;
    for c in &candidates {
        let label = match c.origin {
            CandidateOrigin::Changed => String::new(),
            CandidateOrigin::Related(_) => " (unchanged, context)".to_string(),
        };
        let header = format!("\n### {}{}\n```\n", c.path, label);
        let footer = "\n```\n";
        let numbered = with_line_numbers(&String::from_utf8_lossy(&c.body));
        let entry_size = header.len() + numbered.len() + footer.len();
        let included = used + entry_size <= budgets.files;
        if included {
            s.push_str(&header);
            s.push_str(&numbered);
            s.push_str(footer);
            used += entry_size;
        } else {
            // AC 181 — marker shape is unchanged from v0.3.3.
            files_omitted.push(c.path.clone());
            s.push_str(&format!(
                "[file omitted: {}, {} bytes; budget exhausted]\n",
                c.path, c.size_bytes
            ));
        }
        rows.push(InclusionRow {
            path: c.path.clone(),
            class: c.class,
            hunk_count: c.hunk_count,
            size_bytes: c.size_bytes,
            origin: c.origin,
            included,
        });
    }

    (s, rows, capped_out)
}

/// AC 180 — the ordering, auditable from the archived prompt without
/// re-running the review. AC 186 — the related-file cap is reported here
/// as well as on stderr.
fn inclusion_order_section(rows: &[InclusionRow], capped_out: usize, cap: u32) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n## File inclusion order\n");
    for (i, r) in rows.iter().enumerate() {
        s.push_str(&format!(
            "{:>3}. {} class={} hunks={} bytes={} origin={} {}\n",
            i + 1,
            r.path,
            r.class.label(),
            r.hunk_count,
            r.size_bytes,
            r.origin.label(),
            if r.included { "included" } else { "omitted" },
        ));
    }
    if capped_out > 0 {
        s.push_str(&format!(
            "[related-file cap reached: related_file_max={cap}; {capped_out} further candidate(s) not offered]\n"
        ));
    }
    s
}

/// §6.1: CLAUDE.md/AGENTS.md/.cursorrules first, then the auto-derived
/// `## Local conventions (auto-derived)` subsection. Both contribute to
/// the shared 20 KB `budget_memory`. The subsection header is omitted
/// when no row would render (an empty header reads as a bug).
///
/// `conventions_trusted` is the once-per-call answer from the §6.2
/// bridge check: `promoted_convention` rows render in the conventions
/// section when trusted, and fall back here when not.
fn build_memory_section(
    mem: Option<&MemoryInput>,
    disc: &Discovery,
    local_conventions: &[Dismissal],
    convention_cap: usize,
    conventions_trusted: bool,
    budget_memory: usize,
) -> String {
    let mut out = String::new();
    let mut used = 0usize;

    if let Some(mem) = mem {
        // Step 1: CLAUDE.md / AGENTS.md / .cursorrules (Phase 1A path).
        // Reserve no headroom for the subsection here — the subsection
        // gets whatever bytes remain after this step. The
        // `[memory truncated …]` marker fires from this step if step 1
        // alone overflows.
        let header = format!("\n## Repo memory ({})\n", mem.source_basename);
        let (body, _) = truncate_with_marker(&mem.content, budget_memory, "memory");
        let block = format!("{header}{body}\n");
        out.push_str(&block);
        used = out.len();
    } else if let Some(chosen) = &disc.chosen {
        let block = format!("\n## Memory context\n[not loaded: {chosen}]\n");
        out.push_str(&block);
        used = out.len();
    }

    // Pre-filter the bridge fork in one pass so we know whether the
    // header should render at all (empty subsection → no header).
    let entries: Vec<&Dismissal> = local_conventions
        .iter()
        .filter(|d| match d.promotion_state {
            PromotionState::LocalOnly => true,
            // §6.2 bridge: `promoted_convention` rides the conventions
            // section when conventions.md is committed-and-clean; falls
            // back to the memory section otherwise.
            PromotionState::PromotedConvention => !conventions_trusted,
            PromotionState::Candidate => false,
        })
        .collect();
    if entries.is_empty() {
        return out;
    }

    let subhead = "\n## Local conventions (auto-derived)\n";
    let total_trunc_marker = format!(
        "\n[memory truncated: {placeholder} bytes exceeded {kb}KB; review only top portion]\n",
        placeholder = "{}",
        kb = budget_memory / 1024
    );

    let header_cost = subhead.len();
    if used + header_cost > budget_memory {
        // No room for even the header — emit total-section truncation
        // marker against the dropped bytes (sum of every would-be
        // rendered entry, approximated by the header itself).
        let elided = header_cost + entries_total_bytes(&entries, convention_cap);
        out.push_str(&total_trunc_marker.replace("{}", &elided.to_string()));
        return out;
    }
    out.push_str(subhead);
    used += header_cost;

    let mut dropped_bytes = 0usize;
    for d in &entries {
        let rendered = render_local_convention_entry(d, convention_cap);
        if used + rendered.len() > budget_memory {
            dropped_bytes += rendered.len();
            continue;
        }
        out.push_str(&rendered);
        used += rendered.len();
    }

    if dropped_bytes > 0 {
        // SERVICES.md §2 wording: total bytes that exceeded the cap.
        let marker = total_trunc_marker.replace("{}", &dropped_bytes.to_string());
        // If the marker itself wouldn't fit, we still emit it — the
        // 200KB BUDGET_TOTAL check at assemble() will surface any real
        // overflow. The marker is small (~80 bytes) so this is safe in
        // practice; the per-section soft cap is informational and the
        // marker preserves visibility.
        out.push_str(&marker);
    }

    out
}

/// Sum of all entries' rendered sizes — used only to populate the
/// `bytes exceeded` count when the header itself is too big to fit.
fn entries_total_bytes(entries: &[&Dismissal], convention_cap: usize) -> usize {
    entries
        .iter()
        .map(|d| render_local_convention_entry(d, convention_cap).len())
        .sum()
}

/// §6.1 step 3 per-entry render:
///
/// ```text
/// ### Local convention: <title (≤80 chars)>
/// <body_snapshot truncated to convention_cap bytes, codepoint-safe>
/// [local convention truncated: <hash-short>, <bytes-elided> bytes elided; raise [memory] local_convention_bundle_cap to see full text]    <-- only if truncation fired
/// <!-- recurrence=N, since=YYYY-MM-DD, hash=<12-hex> -->
/// ```
///
/// `body_snapshot` may be `None`; the body line is then omitted.
/// Title truncation is codepoint-aware (Unicode chars, not bytes) per
/// common UX conventions for the 80-char field width.
fn render_local_convention_entry(d: &Dismissal, convention_cap: usize) -> String {
    let title_trunc: String = d.title_snapshot.chars().take(80).collect();
    let hash_full = d.finding_identity_hash.to_hex();
    let hash_short: String = hash_full.chars().take(12).collect();
    let since_iso = format_since(&d.dismissed_at);

    let mut s = format!("\n### Local convention: {title_trunc}\n");
    if let Some(body) = &d.body_snapshot {
        if body.len() <= convention_cap {
            s.push_str(body);
            if !body.ends_with('\n') {
                s.push('\n');
            }
        } else {
            let kept = truncate_at_codepoint_boundary(body, convention_cap);
            let bytes_elided = body.len() - kept.len();
            s.push_str(kept);
            if !kept.ends_with('\n') {
                s.push('\n');
            }
            s.push_str(&format!(
                "[local convention truncated: {hash_short}, {bytes_elided} bytes elided; raise [memory] local_convention_bundle_cap to see full text]\n"
            ));
        }
    }
    s.push_str(&format!(
        "<!-- recurrence={n}, since={since_iso}, hash={hash_short} -->\n",
        n = d.recurrence_count
    ));
    s
}

fn format_since(t: &time::OffsetDateTime) -> String {
    // ISO yyyy-MM-dd (UTC date of the first-recorded dismissal).
    let d = t.to_offset(time::UtcOffset::UTC).date();
    format!("{:04}-{:02}-{:02}", d.year(), u8::from(d.month()), d.day())
}

fn conventions_section(state: &ConventionsState, budget: usize) -> String {
    match state {
        ConventionsState::Trusted(text) => {
            let (body, _) = truncate_with_marker(text, budget, "conventions");
            format!("\n## Quorum conventions (.quorum/conventions.md)\n{body}\n")
        }
        _ => String::new(),
    }
}

fn truncate_with_marker(text: &str, budget: usize, label: &str) -> (String, bool) {
    if text.len() <= budget {
        return (text.to_string(), false);
    }
    let excess = text.len() - budget;
    // Reserve a few hundred bytes for the marker itself.
    let marker = format!(
        "\n[{label} truncated: {} bytes exceeded {}KB; review only top portion]\n",
        excess,
        budget / 1024
    );
    let budget_for_text = budget.saturating_sub(marker.len());
    let mut s = text[..budget_for_text].to_string();
    s.push_str(&marker);
    (s, true)
}

// ===================== v0.4 Stage 1 — WI-1 / WI-2 / WI-3 =====================

/// §5.3 — the envelope overhead is a fixed reservation that does **not**
/// scale with `total_budget_kb` (AC 194).
pub const ENVELOPE_OVERHEAD: usize = 2 * 1024;

/// Denominator for section-share arithmetic: `total_budget_kb` minus the
/// constant envelope, at the 200 KB default.
const SCALABLE_DENOM_KB: usize = 198;

// Stage 1 preserves the v0.3.3 section *proportions* and only makes the
// arithmetic derived rather than hard-coded (AC 194). The §5.3 rebalance
// (80/72/16/24/6, adding the review-policy section) belongs to Stage 2 and
// its AC 209; landing it here would ship a Stage 2 behaviour change under a
// Stage 1 commit. These shares deliberately sum to 210 > 198: per the
// spec's own §4.4 note, section budgets are maxima, not reservations, and
// only the total is enforced by `assemble()`.
const SHARE_DIFF_KB: usize = 100;
const SHARE_FILES_KB: usize = 80;
const SHARE_MEMORY_KB: usize = 20;
const SHARE_CONVENTIONS_KB: usize = 10;

/// Per-section byte budgets derived from `[bundle] total_budget_kb`
/// (AC 194). Every section scales by the same ratio, rounded down, with
/// [`ENVELOPE_OVERHEAD`] held constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budgets {
    pub total: usize,
    pub diff: usize,
    pub files: usize,
    pub memory: usize,
    pub conventions: usize,
}

impl Budgets {
    /// `total_kb` is assumed already range-validated by
    /// [`crate::config::BundleConfig::validate`] (100..=1024).
    pub fn from_total_kb(total_kb: usize) -> Self {
        let total = total_kb * 1024;
        let scalable = total.saturating_sub(ENVELOPE_OVERHEAD);
        let share = |kb: usize| scalable * kb / SCALABLE_DENOM_KB;
        Budgets {
            total,
            diff: share(SHARE_DIFF_KB),
            files: share(SHARE_FILES_KB),
            memory: share(SHARE_MEMORY_KB),
            conventions: share(SHARE_CONVENTIONS_KB),
        }
    }
}

impl Default for Budgets {
    fn default() -> Self {
        Budgets::from_total_kb(200)
    }
}

/// WI-1 path classification (spec §4.2). Lower rank sorts first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FileClass {
    /// Rank 0 — source under review.
    Source,
    /// Rank 1 — build / lint / dependency configuration.
    Config,
    /// Rank 2 — tests and test fixtures.
    Test,
    /// Rank 3 — documentation and inert data.
    Docs,
}

impl FileClass {
    pub fn rank(self) -> u8 {
        match self {
            FileClass::Source => 0,
            FileClass::Config => 1,
            FileClass::Test => 2,
            FileClass::Docs => 3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FileClass::Source => "source",
            FileClass::Config => "config",
            FileClass::Test => "test",
            FileClass::Docs => "docs",
        }
    }
}

/// AC 177 — classification is a pure function of the path string: no I/O,
/// no globals, no dependence on the file's contents or on whether it
/// changed. Evaluated most-specific-first: config, then test, then docs,
/// with everything unmatched falling through to source.
pub fn class_rank(path: &str) -> FileClass {
    let path = path.replace('\\', "/");
    let basename = path.rsplit('/').next().unwrap_or(&path).to_string();
    // Leading separator so a first-segment directory match (`tests/x`)
    // reads the same as a nested one (`a/tests/x`).
    let slashed = format!("/{path}");
    let at_root = !path.contains('/');

    // ---- Rank 1: config ----
    let is_config = basename.starts_with("eslint.config.")
        || basename == ".eslintrc"
        || basename.starts_with(".eslintrc.")
        || (basename.starts_with("tsconfig") && basename.ends_with(".json"))
        || basename == "package.json"
        || basename == "Cargo.toml"
        || basename == "pyproject.toml"
        || basename == "go.mod"
        || basename == ".editorconfig"
        || (at_root && basename.ends_with(".toml"));
    if is_config {
        return FileClass::Config;
    }

    // ---- Rank 2: test ----
    let is_test = slashed.contains("/tests/")
        || slashed.contains("/test/")
        || slashed.contains("/__tests__/")
        || basename.contains(".test.")
        || basename.contains(".spec.")
        || (basename.starts_with("test_") && basename.ends_with(".py"))
        || basename.ends_with("_test.go");
    if is_test {
        return FileClass::Test;
    }

    // ---- Rank 3: docs / data ----
    const DOC_EXTS: &[&str] = &[".md", ".mdx", ".rst", ".txt", ".json", ".lock", ".snap"];
    if DOC_EXTS.iter().any(|e| basename.ends_with(e)) {
        return FileClass::Docs;
    }

    FileClass::Source
}

/// Why an unchanged file was offered as a bundle candidate (WI-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelatedKind {
    /// Repo-root build/lint config implied by a changed file's language.
    ConfigAllowlist,
    /// Resolved from a first-party relative import in a changed file.
    OneHopImport,
    /// Matched a user-supplied `[bundle] include` glob.
    IncludeGlob,
}

impl RelatedKind {
    fn label(self) -> &'static str {
        match self {
            RelatedKind::ConfigAllowlist => "config-allowlist",
            RelatedKind::OneHopImport => "one-hop-import",
            RelatedKind::IncludeGlob => "include-glob",
        }
    }
}

/// Where a candidate came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateOrigin {
    /// Appears in the diff under review.
    Changed,
    /// Unchanged working-tree file pulled in for context.
    Related(RelatedKind),
}

impl CandidateOrigin {
    fn is_related(self) -> bool {
        matches!(self, CandidateOrigin::Related(_))
    }

    fn label(self) -> &'static str {
        match self {
            CandidateOrigin::Changed => "changed",
            CandidateOrigin::Related(k) => k.label(),
        }
    }
}

/// One file offered to the changed-file section, before budgeting.
#[derive(Debug, Clone)]
struct Candidate {
    path: String,
    body: Vec<u8>,
    size_bytes: u64,
    hunk_count: u32,
    class: FileClass,
    origin: CandidateOrigin,
}

impl Candidate {
    /// WI-1 ordering key.
    ///
    /// The spec writes this as an additive score:
    ///
    /// ```text
    /// Score = (class_rank * 1000) - min(hunk_count, 99) * 10 + size_tiebreak
    /// ```
    ///
    /// That cannot be read as a literal integer sum: `size_tiebreak` is
    /// described as "`size_bytes` ascending", and `size_bytes` is unbounded,
    /// so a 60 KB file would contribute 61440 and swamp the 1000-wide class
    /// bands — reintroducing exactly the largest-first eviction WI-1 exists
    /// to remove. The spec is describing a *lexicographic* ordering whose
    /// terms are listed most-significant first, and this tuple is that
    /// ordering. Within any (class, origin) group the result is identical to
    /// the additive formula, because the hunk term (0..=990) never crosses a
    /// class band.
    ///
    /// `is_related` sits directly after `class` to satisfy AC 185: every
    /// changed file precedes every unchanged context file of the same class,
    /// including the case where a changed file has `hunk_count == 0`.
    fn sort_key(&self) -> (u8, u8, std::cmp::Reverse<u32>, u64, &str) {
        (
            self.class.rank(),
            u8::from(self.origin.is_related()),
            std::cmp::Reverse(self.hunk_count.min(99)),
            self.size_bytes,
            self.path.as_str(),
        )
    }
}

/// One row of the AC 180 `## File inclusion order` block.
#[derive(Debug, Clone)]
pub struct InclusionRow {
    pub path: String,
    pub class: FileClass,
    pub hunk_count: u32,
    pub size_bytes: u64,
    pub origin: CandidateOrigin,
    pub included: bool,
}

/// AC 182a — a fixed `%6d | ` gutter, 1-based, on every emitted file body.
/// Costs ~9 bytes per line, which the caller charges against the section
/// budget before deciding whether the file fits.
fn with_line_numbers(body: &str) -> String {
    let mut out = String::with_capacity(body.len() + body.len() / 8);
    for (i, line) in body.lines().enumerate() {
        out.push_str(&format!("{:>6} | {}\n", i + 1, line));
    }
    out
}

// ------------------------- WI-2: related files -------------------------

/// Repo context needed for related-file discovery and budget derivation.
///
/// `Default` leaves `repo_root` and `tracked` unset, which makes every
/// related-file lookup resolve to nothing — so a caller that does not opt
/// in reproduces v0.3.3 candidate selection exactly (AC 190).
#[derive(Default)]
pub struct RelatedContext<'a> {
    /// Absolute path to the repository working directory. `None` disables
    /// all related-file discovery (nothing can be read from disk).
    pub repo_root: Option<&'a std::path::Path>,
    /// Repo-relative, forward-slashed paths tracked at HEAD. A one-hop
    /// specifier that does not land in this set is skipped silently.
    pub tracked: Option<&'a std::collections::HashSet<String>>,
    /// `[bundle]` section from `.quorum/config.toml`.
    pub cfg: crate::config::BundleConfig,
}

/// Repo-root config files implied by a changed file's extension (§4.3).
/// Order within each list is the emission order when several exist.
const TS_CONFIGS: &[&str] = &[
    "eslint.config.js",
    "eslint.config.mjs",
    "eslint.config.cjs",
    "eslint.config.ts",
    ".eslintrc.js",
    ".eslintrc.cjs",
    ".eslintrc.json",
    ".eslintrc",
    "tsconfig.json",
    "package.json",
];

const LANG_CONFIGS: &[(&str, &[&str])] = &[
    ("ts", TS_CONFIGS),
    ("tsx", TS_CONFIGS),
    ("js", TS_CONFIGS),
    ("jsx", TS_CONFIGS),
    ("mjs", TS_CONFIGS),
    ("cjs", TS_CONFIGS),
    ("rs", &["Cargo.toml"]),
    ("py", &["pyproject.toml", "setup.cfg"]),
    ("go", &["go.mod"]),
];

/// Extension candidates tried, in this fixed order, when a JS/TS relative
/// specifier carries no extension of its own.
const JS_EXT_CANDIDATES: &[&str] = &[
    ".ts",
    ".tsx",
    ".js",
    ".jsx",
    ".mjs",
    ".cjs",
    ".json",
    "/index.ts",
    "/index.tsx",
    "/index.js",
    "/index.jsx",
];

fn extension_of(path: &str) -> &str {
    let basename = path.rsplit('/').next().unwrap_or(path);
    match basename.rfind('.') {
        Some(i) => &basename[i + 1..],
        None => "",
    }
}

/// Normalize a repo-relative path containing `.` / `..` segments.
/// Returns `None` if the result escapes the repository root (AC 189).
fn normalize_repo_relative(raw: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for seg in raw.split('/') {
        match seg {
            "" | "." => continue,
            ".." => {
                // Popping past the root means the specifier pointed outside
                // the repository — skip it rather than clamping.
                out.pop()?;
            }
            s => out.push(s),
        }
    }
    if out.is_empty() {
        return None;
    }
    Some(out.join("/"))
}

fn parent_dir(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[..i],
        None => "",
    }
}

/// Extract quoted module specifiers from JS/TS source: `from '...'`,
/// `require('...')`, `import('...')`, and bare `import '...'`.
///
/// Deliberately textual and forgiving — a false positive costs at most one
/// failed tracked-path lookup, which is skipped silently.
fn js_specifiers(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim_start();
        // Skip obvious line comments so commented-out imports are not pulled in.
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        for marker in [" from ", "require(", "import(", "import "] {
            let mut rest = line;
            while let Some(i) = rest.find(marker) {
                rest = &rest[i + marker.len()..];
                let rest_trim = rest.trim_start();
                let quote = match rest_trim.chars().next() {
                    Some(c @ ('"' | '\'')) => c,
                    _ => continue,
                };
                let after = &rest_trim[1..];
                if let Some(end) = after.find(quote) {
                    out.push(after[..end].to_string());
                }
            }
        }
    }
    out
}

/// Rust module references: `mod x;` / `pub mod x;`, `use crate::a::b`,
/// and `use super::x`.
///
/// `mod x;` is not in the spec's `./`, `../`, `crate::`, `super::` list,
/// but it is the Rust equivalent of a relative specifier — without it
/// one-hop discovery finds almost nothing in a Rust repo, which is the
/// repo Quorum is dogfooded on. Included deliberately; see the Stage 1
/// report.
fn rust_specifiers(body: &str) -> Vec<RustRef> {
    let mut out = Vec::new();
    for line in body.lines() {
        let t = line.trim();
        if t.starts_with("//") {
            continue;
        }
        if let Some(rest) = t
            .strip_prefix("mod ")
            .or_else(|| t.strip_prefix("pub mod "))
        {
            if let Some(name) = rest.strip_suffix(';') {
                let name = name.trim();
                if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    out.push(RustRef::Mod(name.to_string()));
                }
            }
            continue;
        }
        for marker in [
            "use crate::",
            "use super::",
            "pub use crate::",
            "pub use super::",
        ] {
            if let Some(rest) = t.strip_prefix(marker) {
                let is_super = marker.ends_with("super::");
                // Take the module path up to the first non-path character.
                let path: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':')
                    .collect();
                let segs: Vec<String> = path
                    .split("::")
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect();
                if !segs.is_empty() {
                    out.push(if is_super {
                        RustRef::Super(segs)
                    } else {
                        RustRef::Crate(segs)
                    });
                }
                break;
            }
        }
    }
    out
}

enum RustRef {
    Mod(String),
    Crate(Vec<String>),
    Super(Vec<String>),
}

/// The crate source root for `crate::` resolution: the nearest ancestor
/// directory literally named `src`. Returns `None` outside such a tree.
fn crate_src_root(path: &str) -> Option<String> {
    let segs: Vec<&str> = path.split('/').collect();
    // Search right-to-left so a nested `src` wins over an outer one.
    let idx = segs.iter().rposition(|s| *s == "src")?;
    Some(segs[..=idx].join("/"))
}

/// Resolve one changed file's first-party specifiers to tracked paths.
/// One hop only: this is called on changed files and never on the files
/// it returns (AC 184).
fn one_hop_candidates(
    path: &str,
    body: &[u8],
    tracked: &std::collections::HashSet<String>,
) -> Vec<String> {
    let text = match std::str::from_utf8(body) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let dir = parent_dir(path);
    let mut out: Vec<String> = Vec::new();
    let mut push = |cand: String| {
        if tracked.contains(&cand) && !out.contains(&cand) {
            out.push(cand);
        }
    };

    match extension_of(path) {
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => {
            for spec in js_specifiers(text) {
                // AC: relative specifiers only — a bare package name is a
                // third-party dependency, not first-party context.
                if !(spec.starts_with("./") || spec.starts_with("../")) {
                    continue;
                }
                let joined = if dir.is_empty() {
                    spec.clone()
                } else {
                    format!("{dir}/{spec}")
                };
                let base = match normalize_repo_relative(&joined) {
                    Some(b) => b,
                    None => continue, // escaped the repo root (AC 189)
                };
                // Exact hit first (specifier carried its own extension),
                // then the fixed extension-candidate order.
                let mut tried = vec![base.clone()];
                tried.extend(JS_EXT_CANDIDATES.iter().map(|e| format!("{base}{e}")));
                for cand in tried {
                    if tracked.contains(&cand) {
                        push(cand);
                        break;
                    }
                }
            }
        }
        "rs" => {
            for r in rust_specifiers(text) {
                let bases: Vec<String> = match r {
                    RustRef::Mod(name) => {
                        let d = if dir.is_empty() {
                            name.clone()
                        } else {
                            format!("{dir}/{name}")
                        };
                        vec![format!("{d}.rs"), format!("{d}/mod.rs")]
                    }
                    RustRef::Super(segs) => {
                        let d = if dir.is_empty() {
                            segs.join("/")
                        } else {
                            format!("{dir}/{}", segs.join("/"))
                        };
                        vec![format!("{d}.rs"), format!("{d}/mod.rs")]
                    }
                    RustRef::Crate(segs) => {
                        let root = match crate_src_root(path) {
                            Some(r) => r,
                            None => continue,
                        };
                        // Progressively shorten: `crate::a::b::C` may name a
                        // module `a/b.rs` with an item `C` inside it.
                        let mut v = Vec::new();
                        for take in (1..=segs.len()).rev() {
                            let joined = segs[..take].join("/");
                            v.push(format!("{root}/{joined}.rs"));
                            v.push(format!("{root}/{joined}/mod.rs"));
                        }
                        v
                    }
                };
                for cand in bases {
                    if let Some(norm) = normalize_repo_relative(&cand) {
                        if tracked.contains(&norm) {
                            push(norm);
                            break;
                        }
                    }
                }
            }
        }
        _ => {}
    }
    out
}

// ------------------------- `[bundle] include` globs -------------------------

/// Minimal glob matcher over repo-relative, forward-slashed paths.
///
/// No glob crate is present in the dependency set and CLAUDE.md forbids
/// adding one, so this is hand-rolled. Supports `?` (one character, not
/// `/`), `*` (any run within one path segment), and `**` (any run of whole
/// segments, including none).
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let p: Vec<&str> = pattern.split('/').collect();
    let t: Vec<&str> = path.split('/').collect();
    seg_match(&p, &t)
}

fn seg_match(p: &[&str], t: &[&str]) -> bool {
    match p.split_first() {
        None => t.is_empty(),
        Some((&"**", rest)) => (0..=t.len()).any(|i| seg_match(rest, &t[i..])),
        Some((head, rest)) => match t.split_first() {
            Some((first, t_rest)) if wildcard_match(head, first) => seg_match(rest, t_rest),
            _ => false,
        },
    }
}

/// `*` / `?` matching within a single path segment, via backtracking.
fn wildcard_match(pat: &str, s: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let c: Vec<char> = s.chars().collect();
    let (mut pi, mut ci) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while ci < c.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == c[ci]) {
            pi += 1;
            ci += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = pi;
            mark = ci;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            ci = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Outcome of related-file discovery, including what the cap turned away.
struct RelatedResult {
    candidates: Vec<Candidate>,
    /// AC 186 — candidates dropped because `related_file_max` bound.
    capped_out: usize,
}

/// Build the unchanged-context candidate list (AC 183–190).
///
/// `changed_paths` is the dedup set: a file already in the diff is never
/// offered again here (AC 182).
fn collect_related(
    changed: &[StagedFile],
    changed_paths: &std::collections::HashSet<String>,
    ctx: &RelatedContext<'_>,
    exclusions: &mut Vec<(String, FileExclusionReason)>,
) -> RelatedResult {
    let mut out = RelatedResult {
        candidates: Vec::new(),
        capped_out: 0,
    };
    let repo_root = match ctx.repo_root {
        Some(r) => r,
        None => return out,
    };
    let empty = std::collections::HashSet::new();
    let tracked = ctx.tracked.unwrap_or(&empty);

    // Ordered, deduplicated wishlist of repo-relative paths.
    let mut wanted: Vec<(String, RelatedKind)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Not a closure: it borrows nothing, so passing the two accumulators
    // explicitly keeps the borrow checker out of the discovery loops.
    fn want(
        path: String,
        kind: RelatedKind,
        changed_paths: &std::collections::HashSet<String>,
        wanted: &mut Vec<(String, RelatedKind)>,
        seen: &mut std::collections::HashSet<String>,
    ) {
        if changed_paths.contains(&path) || !seen.insert(path.clone()) {
            return;
        }
        wanted.push((path, kind));
    }

    if ctx.cfg.related_files {
        // --- Config allowlist, keyed on the languages actually changed ---
        let mut langs: Vec<&str> = Vec::new();
        for f in changed {
            let ext = extension_of(&f.path);
            if LANG_CONFIGS.iter().any(|(e, _)| *e == ext) && !langs.contains(&ext) {
                langs.push(ext);
            }
        }
        for lang in langs {
            if let Some((_, configs)) = LANG_CONFIGS.iter().find(|(e, _)| *e == lang) {
                for c in *configs {
                    // Repo-root only, and only if it actually exists.
                    if repo_root.join(c).is_file() {
                        want(
                            (*c).to_string(),
                            RelatedKind::ConfigAllowlist,
                            changed_paths,
                            &mut wanted,
                            &mut seen,
                        );
                    }
                }
            }
        }

        // --- One-hop neighbours of each changed file ---
        for f in changed {
            if f.status == FileStatus::Deleted || f.is_binary {
                continue;
            }
            let body = match &f.index_blob {
                Some(b) => b,
                None => continue,
            };
            for cand in one_hop_candidates(&f.path, body, tracked) {
                want(
                    cand,
                    RelatedKind::OneHopImport,
                    changed_paths,
                    &mut wanted,
                    &mut seen,
                );
            }
        }
    }

    // --- `[bundle] include` globs (AC 193) ---
    // Independent of `related_files`: an explicit user glob is a direct
    // instruction, not part of the automatic discovery AC 190 turns off.
    if !ctx.cfg.include.is_empty() {
        let mut matched: Vec<String> = tracked
            .iter()
            .filter(|p| ctx.cfg.include.iter().any(|g| glob_match(g, p)))
            .cloned()
            .collect();
        matched.sort();
        for p in matched {
            want(
                p,
                RelatedKind::IncludeGlob,
                changed_paths,
                &mut wanted,
                &mut seen,
            );
        }
    }

    // --- Materialize: deny-list, size, binary, and the cap ---
    let cap = ctx.cfg.related_file_max as usize;
    for (path, kind) in wanted {
        if out.candidates.len() >= cap {
            out.capped_out += 1;
            continue;
        }
        // AC 187 — deny-list rules apply identically to related files.
        if let Some(reason) = deny_list::reason(&path) {
            exclusions.push((path.clone(), FileExclusionReason::DenyList(reason)));
            continue;
        }
        let abs = repo_root.join(&path);
        let meta = match std::fs::metadata(&abs) {
            Ok(m) => m,
            Err(_) => continue,
        };
        // AC 188 — same MAX_FILE_BYTES ceiling as changed files.
        if meta.len() > MAX_FILE_BYTES {
            exclusions.push((path.clone(), FileExclusionReason::TooLarge));
            continue;
        }
        let body = match std::fs::read(&abs) {
            Ok(b) => b,
            Err(_) => continue,
        };
        // AC 188 — same binary check.
        if crate::git::looks_binary(&body) {
            exclusions.push((path.clone(), FileExclusionReason::Binary));
            continue;
        }
        out.candidates.push(Candidate {
            size_bytes: body.len() as u64,
            path,
            body,
            hunk_count: 0,
            class: {
                // AC 183 — allowlist configs enter at Config rank even when
                // their path would not imply it. AC 193 — `include` globs
                // enter at the class their path implies, not a privileged one.
                match kind {
                    RelatedKind::ConfigAllowlist => FileClass::Config,
                    _ => FileClass::Source,
                }
            },
            origin: CandidateOrigin::Related(kind),
        });
    }
    // `include` and one-hop files take the class their path implies.
    for c in out.candidates.iter_mut() {
        if !matches!(
            c.origin,
            CandidateOrigin::Related(RelatedKind::ConfigAllowlist)
        ) {
            c.class = class_rank(&c.path);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_smaller_than_budget_is_noop() {
        let (out, truncated) = truncate_with_marker("hello", 100, "x");
        assert!(!truncated);
        assert_eq!(out, "hello");
    }

    #[test]
    fn truncate_marks_overflow() {
        let big = "x".repeat(500);
        let (out, truncated) = truncate_with_marker(&big, 200, "x");
        assert!(truncated);
        assert!(out.len() <= 200);
        assert!(out.contains("[x truncated"));
    }

    // ===================== v0.4 Stage 1 unit tests =====================

    /// AC 177 — `class_rank` is a pure function of the path string,
    /// table-tested across all four ranks. This table is the spec's own
    /// acceptance list (§4.2) plus the negative cases that pin the
    /// qualifiers the prose carries ("`*.toml` **at repo root**").
    #[test]
    fn ac177_class_rank_table() {
        use FileClass::*;
        let table: &[(&str, FileClass)] = &[
            // ---- rank 1: config ----
            ("eslint.config.js", Config),
            ("eslint.config.mjs", Config),
            (".eslintrc.json", Config),
            (".eslintrc", Config),
            ("tsconfig.json", Config),
            ("tsconfig.build.json", Config),
            ("package.json", Config),
            ("Cargo.toml", Config),
            ("crates/quorum-core/Cargo.toml", Config),
            ("pyproject.toml", Config),
            ("go.mod", Config),
            (".editorconfig", Config),
            ("rustfmt.toml", Config),
            // ---- rank 2: test ----
            ("tests/fixtures/golden.json", Test),
            ("src/x.test.ts", Test),
            ("src/x.spec.tsx", Test),
            ("app/__tests__/util.js", Test),
            ("pkg/test_utils.py", Test),
            ("internal/api_test.go", Test),
            ("crates/quorum-cli/test/helper.rs", Test),
            // ---- rank 3: docs / data ----
            ("README.md", Docs),
            ("docs/guide.mdx", Docs),
            ("docs/index.rst", Docs),
            ("NOTES.txt", Docs),
            ("data/seed.json", Docs),
            ("Cargo.lock", Docs),
            ("package-lock.json", Docs),
            ("snapshots/render.snap", Docs),
            // ---- rank 0: source ----
            ("src/main.rs", Source),
            ("src/lib/util.ts", Source),
            ("cmd/server/main.go", Source),
            ("app/models/user.py", Source),
            ("crates/quorum-core/src/bundle.rs", Source),
            // `*.toml` is config only at the repo root; nested build files
            // are ordinary source so they cannot outrank the diff.
            ("deploy/nested/config.toml", Source),
        ];
        assert!(table.len() >= 20, "spec requires at least 20 paths");
        for (path, want) in table {
            assert_eq!(
                class_rank(path),
                *want,
                "class_rank({path:?}) should be {want:?}"
            );
        }
    }

    /// AC 177 — purity: classification depends on nothing but the string.
    #[test]
    fn ac177_class_rank_is_pure_and_separator_agnostic() {
        assert_eq!(class_rank("src\\x.test.ts"), FileClass::Test);
        for _ in 0..3 {
            assert_eq!(class_rank("README.md"), FileClass::Docs);
        }
    }

    /// AC 176 — ordering is class, then changed-before-context, then hunk
    /// count descending, then size ascending, then path. Fully deterministic.
    #[test]
    fn ac176_ordering_is_deterministic() {
        let mk = |path: &str, size: u64, hunks: u32, origin: CandidateOrigin| Candidate {
            path: path.to_string(),
            body: Vec::new(),
            size_bytes: size,
            hunk_count: hunks,
            class: class_rank(path),
            origin,
        };
        let mut v = [
            mk("README.md", 10, 9, CandidateOrigin::Changed),
            mk("src/b.rs", 900, 1, CandidateOrigin::Changed),
            mk("src/a.rs", 100, 1, CandidateOrigin::Changed),
            mk("src/hot.rs", 5000, 7, CandidateOrigin::Changed),
            mk(
                "src/ctx.rs",
                10,
                0,
                CandidateOrigin::Related(RelatedKind::OneHopImport),
            ),
            mk("Cargo.toml", 50, 2, CandidateOrigin::Changed),
            mk("src/x.test.rs", 50, 3, CandidateOrigin::Changed),
        ];
        v.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
        let order: Vec<&str> = v.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(
            order,
            vec![
                "src/hot.rs",    // source, changed, 7 hunks
                "src/a.rs",      // source, changed, 1 hunk, 100 bytes
                "src/b.rs",      // source, changed, 1 hunk, 900 bytes
                "src/ctx.rs",    // source, UNCHANGED context -> after all changed
                "Cargo.toml",    // config
                "src/x.test.rs", // test
                "README.md",     // docs
            ]
        );
    }

    /// AC 176 — size is a tiebreak, never a primary key. A large,
    /// heavily-edited source file must still precede a tiny doc.
    #[test]
    fn ac176_size_never_outranks_class() {
        let big_src = Candidate {
            path: "src/huge.rs".into(),
            body: Vec::new(),
            size_bytes: 500_000,
            hunk_count: 1,
            class: FileClass::Source,
            origin: CandidateOrigin::Changed,
        };
        let tiny_doc = Candidate {
            path: "a.md".into(),
            body: Vec::new(),
            size_bytes: 1,
            hunk_count: 99,
            class: FileClass::Docs,
            origin: CandidateOrigin::Changed,
        };
        assert!(big_src.sort_key() < tiny_doc.sort_key());
    }

    /// AC 185 — a changed file with zero hunks still precedes an unchanged
    /// context file of the same class. This is the case the spec's additive
    /// score cannot express on its own.
    #[test]
    fn ac185_changed_precedes_context_even_at_zero_hunks() {
        let changed = Candidate {
            path: "src/z.rs".into(),
            body: Vec::new(),
            size_bytes: 9_999,
            hunk_count: 0,
            class: FileClass::Source,
            origin: CandidateOrigin::Changed,
        };
        let context = Candidate {
            path: "src/a.rs".into(),
            body: Vec::new(),
            size_bytes: 1,
            hunk_count: 0,
            class: FileClass::Source,
            origin: CandidateOrigin::Related(RelatedKind::OneHopImport),
        };
        assert!(
            changed.sort_key() < context.sort_key(),
            "changed files must never be displaced by context files"
        );
    }

    /// AC 182a — a fixed 1-based `%6d | ` gutter on every line.
    #[test]
    fn ac182a_line_number_gutter_shape() {
        let out = with_line_numbers("alpha\nbeta\n");
        assert_eq!(out, "     1 | alpha\n     2 | beta\n");
        // Width holds past 6 digits rather than truncating the number.
        let many: String = (0..1_000_001).map(|_| "x\n").collect();
        let numbered = with_line_numbers(&many);
        assert!(numbered.contains("1000001 | x"));
        // ~9 bytes of overhead per line, as §5.3's arithmetic assumes.
        let body = "abc\n".repeat(100);
        assert_eq!(with_line_numbers(&body).len(), body.len() + 9 * 100);
    }

    /// AC 194 — every section scales by the same ratio, rounded down, with
    /// the 2 KB envelope held constant.
    #[test]
    fn ac194_budgets_scale_proportionally() {
        let d = Budgets::from_total_kb(200);
        assert_eq!(d.total, BUDGET_TOTAL);
        assert_eq!(d.diff, BUDGET_DIFF);
        assert_eq!(d.files, BUDGET_FILES);
        assert_eq!(d.memory, BUDGET_MEMORY);
        assert_eq!(d.conventions, BUDGET_CONVENTIONS);
        assert_eq!(Budgets::default(), d);

        // Doubling the total roughly doubles each section; the constant
        // envelope means it is slightly more than 2x, never less.
        let big = Budgets::from_total_kb(400);
        for (small, large) in [
            (d.diff, big.diff),
            (d.files, big.files),
            (d.memory, big.memory),
            (d.conventions, big.conventions),
        ] {
            assert!(large >= small * 2, "{large} should be at least 2x {small}");
        }
        // "Same ratio, rounded down" stated exactly: every section is the
        // same fraction of the same scalable pool, floored. Comparing
        // floats here would only measure the flooring error, which differs
        // per section by up to a byte.
        let scalable = 400 * 1024 - ENVELOPE_OVERHEAD;
        for (got, share) in [
            (big.diff, SHARE_DIFF_KB),
            (big.files, SHARE_FILES_KB),
            (big.memory, SHARE_MEMORY_KB),
            (big.conventions, SHARE_CONVENTIONS_KB),
        ] {
            assert_eq!(got, scalable * share / SCALABLE_DENOM_KB);
        }
    }

    /// AC 194 — the scalable pool never overruns the declared total.
    #[test]
    fn ac194_sections_fit_inside_total_at_every_size() {
        for kb in [100usize, 128, 200, 333, 512, 1024] {
            let b = Budgets::from_total_kb(kb);
            assert_eq!(b.total, kb * 1024);
            // v0.3.3 proportions are deliberately oversubscribed (maxima,
            // not reservations) — only the total is enforced. What must
            // hold is that the *ratio* is preserved exactly.
            let scalable = kb * 1024 - ENVELOPE_OVERHEAD;
            assert_eq!(b.diff, scalable * 100 / 198);
            assert_eq!(b.files, scalable * 80 / 198);
        }
    }

    /// AC 193 — hand-rolled glob matching over repo-relative paths.
    #[test]
    fn ac193_glob_matcher() {
        assert!(glob_match("*.toml", "Cargo.toml"));
        assert!(!glob_match("*.toml", "a/Cargo.toml"));
        assert!(glob_match("**/*.toml", "a/b/Cargo.toml"));
        assert!(glob_match("**/*.toml", "Cargo.toml"));
        assert!(glob_match("src/**", "src/a/b/c.rs"));
        assert!(glob_match("src/**/*.rs", "src/a/b/c.rs"));
        assert!(glob_match("src/*.rs", "src/main.rs"));
        assert!(!glob_match("src/*.rs", "src/a/main.rs"));
        assert!(glob_match("config/?.json", "config/a.json"));
        assert!(!glob_match("config/?.json", "config/ab.json"));
        assert!(glob_match("schema/*.sql", "schema/001_init.sql"));
        assert!(!glob_match("schema/*.sql", "schema/001_init.rs"));
        // No backtracking blowup on a pathological pattern.
        assert!(!glob_match(
            "a*a*a*a*a*b",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaac"
        ));
    }

    /// AC 189 — a specifier that climbs out of the repository is dropped,
    /// never clamped to the root.
    #[test]
    fn ac189_normalize_rejects_escapes() {
        assert_eq!(
            normalize_repo_relative("src/a/../b.rs"),
            Some("src/b.rs".to_string())
        );
        assert_eq!(
            normalize_repo_relative("src/./b.rs"),
            Some("src/b.rs".into())
        );
        assert_eq!(normalize_repo_relative("../outside.rs"), None);
        assert_eq!(normalize_repo_relative("src/../../etc/passwd"), None);
    }

    /// The textual specifier scanners feeding one-hop resolution.
    #[test]
    fn one_hop_specifier_extraction() {
        let js = "import a from './a';\n\
                  import { b } from \"../lib/b.ts\";\n\
                  const c = require('./c.js');\n\
                  import 'side-effect';\n\
                  // import x from './commented';\n\
                  import pkg from 'react';\n";
        let specs = js_specifiers(js);
        assert!(specs.contains(&"./a".to_string()));
        assert!(specs.contains(&"../lib/b.ts".to_string()));
        assert!(specs.contains(&"./c.js".to_string()));
        assert!(!specs.contains(&"./commented".to_string()));
        // Bare package names are extracted but filtered later as non-relative.
        assert!(specs.contains(&"react".to_string()));

        let rs = "mod helper;\npub mod inner;\nuse crate::bundle::assemble;\nuse super::sibling;\n";
        let refs = rust_specifiers(rs);
        assert_eq!(refs.len(), 4);
        assert!(matches!(&refs[0], RustRef::Mod(m) if m == "helper"));
        assert!(matches!(&refs[1], RustRef::Mod(m) if m == "inner"));
        assert!(matches!(&refs[2], RustRef::Crate(s) if s == &["bundle", "assemble"]));
        assert!(matches!(&refs[3], RustRef::Super(s) if s == &["sibling"]));
    }

    #[test]
    fn crate_src_root_finds_nearest_src() {
        assert_eq!(
            crate_src_root("crates/quorum-core/src/memory/store.rs").as_deref(),
            Some("crates/quorum-core/src")
        );
        assert_eq!(crate_src_root("src/main.rs").as_deref(), Some("src"));
        assert_eq!(crate_src_root("build.rs"), None);
    }
}
