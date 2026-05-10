//! Bundle assembly: turn a staged diff + memory + conventions into a
//! single envelope to submit to Lippa's Consensus prompt field.
//!
//! Per-section budgets (spec §4.3.2):
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
}

#[derive(thiserror::Error, Debug)]
pub enum BundleError {
    #[error("bundle assembled to {0} bytes; exceeds 200KB cap")]
    BundleTooLarge(usize),
}

pub struct BundleInputs<'a> {
    pub staged: &'a StagedDiff,
    pub memory: Option<MemoryInput>,
    pub conventions: &'a ConventionsState,
    pub discovery: &'a Discovery,
    pub branch: &'a str,
    pub head_sha: &'a str,
    pub remote_url: Option<&'a str>,
}

pub struct MemoryInput {
    pub source_basename: String,
    pub content: String,
}

pub fn assemble(inp: &BundleInputs<'_>) -> Result<BundleResult, BundleError> {
    let mut exclusions: Vec<(String, FileExclusionReason)> = Vec::new();
    let mut files_omitted: Vec<String> = Vec::new();
    let mut sections: Vec<String> = Vec::new();

    sections.push(envelope_header(inp));

    let (diff_section, diff_truncated) =
        diff_section(&inp.staged.unified, &inp.staged.files, &mut exclusions);
    sections.push(diff_section);

    let files_section = files_section(&inp.staged.files, &mut exclusions, &mut files_omitted);
    sections.push(files_section);

    if let Some(mem) = &inp.memory {
        sections.push(memory_section(mem));
    } else if let Some(chosen) = &inp.discovery.chosen {
        sections.push(format!("\n## Memory context\n[not loaded: {chosen}]\n"));
    }

    sections.push(conventions_section(inp.conventions));
    sections.push(format!("\n{END}\n"));

    let prompt = sections.join("");
    let bytes_used = prompt.len();
    if bytes_used > BUDGET_TOTAL {
        return Err(BundleError::BundleTooLarge(bytes_used));
    }

    Ok(BundleResult {
        prompt,
        exclusions,
        bytes_used,
        diff_truncated,
        files_omitted,
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
    let (body, truncated) = truncate_with_marker(&diff_body, BUDGET_DIFF, "diff");
    (format!("{header}{body}"), truncated)
}

fn files_section(
    files: &[StagedFile],
    exclusions: &mut Vec<(String, FileExclusionReason)>,
    files_omitted: &mut Vec<String>,
) -> String {
    let mut s = String::new();
    s.push_str("\n## Changed-file contents (from index blobs)\n");

    // Eligible files: not deleted, not deny-listed (already excluded), not
    // binary, not >2MB. Sort by size descending for largest-first inclusion.
    let mut eligible: Vec<&StagedFile> = files
        .iter()
        .filter(|f| {
            if f.status == FileStatus::Deleted {
                exclusions.push((f.path.clone(), FileExclusionReason::Deleted));
                return false;
            }
            if deny_list::is_denied(&f.path) {
                return false; // already marked in diff section
            }
            if f.is_binary {
                exclusions.push((f.path.clone(), FileExclusionReason::Binary));
                return false;
            }
            if f.size_bytes > MAX_FILE_BYTES {
                exclusions.push((f.path.clone(), FileExclusionReason::TooLarge));
                return false;
            }
            f.index_blob.is_some()
        })
        .collect();
    eligible.sort_by_key(|f| std::cmp::Reverse(f.size_bytes));

    let mut used = 0usize;
    for f in eligible {
        let header = format!("\n### {}\n```\n", f.path);
        let footer = "\n```\n";
        let blob = f
            .index_blob
            .as_ref()
            .map(|b| String::from_utf8_lossy(b).to_string())
            .unwrap_or_default();
        let entry_size = header.len() + blob.len() + footer.len();
        if used + entry_size > BUDGET_FILES {
            files_omitted.push(f.path.clone());
            s.push_str(&format!(
                "[file omitted: {}, {} bytes; budget exhausted]\n",
                f.path, f.size_bytes
            ));
            continue;
        }
        s.push_str(&header);
        s.push_str(&blob);
        s.push_str(footer);
        used += entry_size;
    }
    s
}

fn memory_section(mem: &MemoryInput) -> String {
    let (body, _) = truncate_with_marker(&mem.content, BUDGET_MEMORY, "memory");
    format!("\n## Repo memory ({})\n{body}\n", mem.source_basename)
}

fn conventions_section(state: &ConventionsState) -> String {
    match state {
        ConventionsState::Trusted(text) => {
            let (body, _) = truncate_with_marker(text, BUDGET_CONVENTIONS, "conventions");
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
}
