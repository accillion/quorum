//! Staged-diff extraction via `git2::Repository::diff_tree_to_index`.
//!
//! Phase 1A reviews HEAD vs index — what's *staged*. File contents come
//! from index blobs, not the working tree, so unstaged edits in modified
//! files do not leak into the review (spec §4.3.2).

use std::path::Path;

#[derive(thiserror::Error, Debug)]
pub enum GitError {
    #[error("git error: {0}")]
    Inner(#[from] git2::Error),
    #[error("not in a git repository: {0}")]
    NotARepo(std::path::PathBuf),
    #[error("repository has no HEAD commit yet")]
    NoHead,
}

#[derive(Debug, Clone)]
pub struct StagedFile {
    pub path: String,
    pub status: FileStatus,
    /// File contents from the *index* blob (Some) — None for deletes and binaries.
    pub index_blob: Option<Vec<u8>>,
    /// `true` if git2 detected this file as binary.
    pub is_binary: bool,
    /// Size of the index blob, even if `index_blob` is None.
    pub size_bytes: u64,
    /// v0.4 AC 179 — number of diff hunks touching this file, from
    /// libgit2's own hunk callback (`Diff::foreach`). Deleted and binary
    /// files carry 0. Feeds the WI-1 priority scorer: more hunks means
    /// more of the change under review lives in this file.
    pub hunk_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    TypeChange,
    Other,
}

/// Result of inspecting the staged diff (or a commit-range diff —
/// Phase 1B extends the same shape to cover `DiffSource::CommitRange`).
pub struct StagedDiff {
    /// Unified diff text (3 context lines, full repo scope). Under
    /// `DiffSource::StagedIndex` this is `HEAD..index`; under
    /// `DiffSource::CommitRange { base, head }` this is `base..head`.
    pub unified: String,
    /// Per-file entries with blob contents from the **right side** of
    /// the diff (the index for staged; the `head` tree for commit-range).
    /// Filtered for binary / deleted via the same rules either way.
    pub files: Vec<StagedFile>,
    /// `true` if no diff entries exist.
    pub is_empty: bool,
}

/// Phase 1B P07: bundle source is a parameter, not a hardcoded git op.
/// The CLI picks the variant at invocation; `quorum-core::bundle` is
/// agnostic — it receives a `StagedDiff` either way and budgets/deny-lists
/// identically.
#[derive(Debug, Clone)]
pub enum DiffSource {
    /// HEAD vs index — Phase 1A behavior, pre-commit hook semantics.
    StagedIndex,
    /// `base..head` revision range — pre-push hook + `quorum review --range`.
    CommitRange { base: String, head: String },
}

const BINARY_DETECTION_NULLBYTE_WINDOW: usize = 8192;

/// Discover the git repo at or above `start`, then extract the staged diff.
pub fn staged_diff(start: &Path) -> Result<(git2::Repository, StagedDiff), GitError> {
    let repo =
        git2::Repository::discover(start).map_err(|_| GitError::NotARepo(start.to_path_buf()))?;
    let diff = compute(&repo)?;
    Ok((repo, diff))
}

fn compute(repo: &git2::Repository) -> Result<StagedDiff, GitError> {
    // HEAD tree.
    let head_tree = match repo.head() {
        Ok(h) => Some(h.peel_to_tree()?),
        Err(_) => None,
    };
    // Index.
    let index = repo.index()?;

    let mut opts = git2::DiffOptions::new();
    opts.context_lines(3)
        .include_untracked(false)
        .ignore_submodules(true);
    // `find_similar` for rename detection (spec §6.1 ac 30).
    let diff = repo.diff_tree_to_index(head_tree.as_ref(), Some(&index), Some(&mut opts))?;
    {
        let mut find_opts = git2::DiffFindOptions::new();
        find_opts.renames(true).copies(false);
        // diff is consumed mutably via `find_similar`; safe because we don't
        // hold any other borrow.
        // SAFETY note: git2 requires `&mut self`; we own the value here.
    }
    // git2's `find_similar` requires &mut Diff, take it by value via `drop`/rebuild.
    let mut diff = diff;
    let mut find_opts = git2::DiffFindOptions::new();
    find_opts.renames(true).copies(false);
    diff.find_similar(Some(&mut find_opts))?;

    let mut unified = String::new();
    diff.print(git2::DiffFormat::Patch, |_d, _h, line| {
        match line.origin() {
            'F' | 'H' => unified.push_str(std::str::from_utf8(line.content()).unwrap_or("")),
            c => {
                unified.push(c);
                unified.push_str(std::str::from_utf8(line.content()).unwrap_or(""));
            }
        }
        true
    })?;

    // Walk file deltas.
    let hunks = hunk_counts(&diff);
    let mut files: Vec<StagedFile> = Vec::new();
    let stats_count = diff.deltas().len();
    for i in 0..stats_count {
        let delta = match diff.get_delta(i) {
            Some(d) => d,
            None => continue,
        };
        let new_path = delta
            .new_file()
            .path()
            .and_then(|p| p.to_str())
            .map(|s| s.replace('\\', "/"));
        let path = match new_path {
            Some(p) => p,
            None => continue,
        };
        let status = match delta.status() {
            git2::Delta::Added => FileStatus::Added,
            git2::Delta::Modified => FileStatus::Modified,
            git2::Delta::Deleted => FileStatus::Deleted,
            git2::Delta::Renamed => FileStatus::Renamed,
            git2::Delta::Typechange => FileStatus::TypeChange,
            _ => FileStatus::Other,
        };
        let new_file = delta.new_file();
        let oid = new_file.id();

        let (index_blob, is_binary, size_bytes) = if status == FileStatus::Deleted {
            (None, false, 0)
        } else if let Ok(blob) = repo.find_blob(oid) {
            let content = blob.content().to_vec();
            let detected_binary = blob.is_binary() || detect_binary(&content);
            let size = content.len() as u64;
            if detected_binary {
                (None, true, size)
            } else {
                (Some(content), false, size)
            }
        } else {
            (None, false, 0)
        };

        // AC 179: deleted and binary files carry hunk_count = 0.
        let hunk_count = if status == FileStatus::Deleted || is_binary {
            0
        } else {
            hunks.get(&path).copied().unwrap_or(0)
        };

        files.push(StagedFile {
            path,
            status,
            index_blob,
            is_binary,
            size_bytes,
            hunk_count,
        });
    }

    let is_empty = files.is_empty();
    Ok(StagedDiff {
        unified,
        files,
        is_empty,
    })
}

/// v0.4 AC 179 / CC-Recon-v04-2 — per-file hunk counts straight from
/// libgit2. `Diff::foreach`'s hunk callback fires once per hunk with the
/// owning `DiffDelta`, so no `@@`-header counting of the patch text is
/// needed. (`DiffStats` exposes only files_changed / insertions /
/// deletions, never per-file hunks — hence `foreach`.)
///
/// Keyed on the delta's *new* path, forward-slashed, to match the key
/// used when building `StagedFile::path`.
fn hunk_counts(diff: &git2::Diff<'_>) -> std::collections::HashMap<String, u32> {
    let mut counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut file_cb = |_d: git2::DiffDelta<'_>, _p: f32| true;
    let mut hunk_cb = |d: git2::DiffDelta<'_>, _h: git2::DiffHunk<'_>| {
        if let Some(p) = d
            .new_file()
            .path()
            .and_then(|p| p.to_str())
            .map(|s| s.replace('\\', "/"))
        {
            *counts.entry(p).or_insert(0) += 1;
        }
        true
    };
    // A failure here is non-fatal: hunk counts are a ranking signal, not
    // correctness. Fall back to whatever was collected before the error.
    let _ = diff.foreach(&mut file_cb, None, Some(&mut hunk_cb), None);
    counts
}

/// v0.4 WI-2 — the same null-byte heuristic used for index blobs, made
/// public so related-file discovery applies an identical binary check to
/// working-tree reads (AC 188).
pub fn looks_binary(bytes: &[u8]) -> bool {
    detect_binary(bytes)
}

fn detect_binary(bytes: &[u8]) -> bool {
    let window_end = bytes.len().min(BINARY_DETECTION_NULLBYTE_WINDOW);
    bytes[..window_end].contains(&0)
}

/// Discover the repo and dispatch on a [`DiffSource`]. Returns the same
/// `StagedDiff` shape so downstream bundle assembly is source-agnostic.
pub fn diff_for_source(
    start: &Path,
    source: &DiffSource,
) -> Result<(git2::Repository, StagedDiff), GitError> {
    let repo =
        git2::Repository::discover(start).map_err(|_| GitError::NotARepo(start.to_path_buf()))?;
    let diff = match source {
        DiffSource::StagedIndex => compute(&repo)?,
        DiffSource::CommitRange { base, head } => compute_range(&repo, base, head)?,
    };
    Ok((repo, diff))
}

/// `base..head` revision range. Both refs are resolved via
/// `revparse_single` so callers can pass SHAs, branches, tags, or
/// arbitrary rev expressions (`HEAD~3`, `origin/main`, etc.).
fn compute_range(repo: &git2::Repository, base: &str, head: &str) -> Result<StagedDiff, GitError> {
    let base_commit = repo.revparse_single(base)?.peel_to_commit()?;
    let head_commit = repo.revparse_single(head)?.peel_to_commit()?;
    let base_tree = base_commit.tree()?;
    let head_tree = head_commit.tree()?;

    let mut opts = git2::DiffOptions::new();
    opts.context_lines(3)
        .include_untracked(false)
        .ignore_submodules(true);
    let mut diff = repo.diff_tree_to_tree(Some(&base_tree), Some(&head_tree), Some(&mut opts))?;
    let mut find_opts = git2::DiffFindOptions::new();
    find_opts.renames(true).copies(false);
    diff.find_similar(Some(&mut find_opts))?;

    let mut unified = String::new();
    diff.print(git2::DiffFormat::Patch, |_d, _h, line| {
        match line.origin() {
            'F' | 'H' => unified.push_str(std::str::from_utf8(line.content()).unwrap_or("")),
            c => {
                unified.push(c);
                unified.push_str(std::str::from_utf8(line.content()).unwrap_or(""));
            }
        }
        true
    })?;

    let hunks = hunk_counts(&diff);
    let mut files: Vec<StagedFile> = Vec::new();
    let n = diff.deltas().len();
    for i in 0..n {
        let delta = match diff.get_delta(i) {
            Some(d) => d,
            None => continue,
        };
        let path = match delta
            .new_file()
            .path()
            .and_then(|p| p.to_str())
            .map(|s| s.replace('\\', "/"))
        {
            Some(p) => p,
            None => continue,
        };
        let status = match delta.status() {
            git2::Delta::Added => FileStatus::Added,
            git2::Delta::Modified => FileStatus::Modified,
            git2::Delta::Deleted => FileStatus::Deleted,
            git2::Delta::Renamed => FileStatus::Renamed,
            git2::Delta::Typechange => FileStatus::TypeChange,
            _ => FileStatus::Other,
        };
        let new_file = delta.new_file();
        let oid = new_file.id();

        let (blob_bytes, is_binary, size_bytes) = if status == FileStatus::Deleted {
            (None, false, 0)
        } else if let Ok(blob) = repo.find_blob(oid) {
            let content = blob.content().to_vec();
            let detected_binary = blob.is_binary() || detect_binary(&content);
            let size = content.len() as u64;
            if detected_binary {
                (None, true, size)
            } else {
                (Some(content), false, size)
            }
        } else {
            (None, false, 0)
        };

        // AC 179: deleted and binary files carry hunk_count = 0.
        let hunk_count = if status == FileStatus::Deleted || is_binary {
            0
        } else {
            hunks.get(&path).copied().unwrap_or(0)
        };

        files.push(StagedFile {
            path,
            status,
            index_blob: blob_bytes,
            is_binary,
            size_bytes,
            hunk_count,
        });
    }

    let is_empty = files.is_empty();
    Ok(StagedDiff {
        unified,
        files,
        is_empty,
    })
}

/// v0.4 WI-2 — the set of repo-relative, forward-slashed paths tracked at
/// HEAD. One-hop related-file resolution consults this so an untracked
/// scratch file next to a changed source file is never pulled into the
/// bundle (spec §4.3: "a specifier that does not resolve to a tracked
/// file is skipped silently").
///
/// A repo with no HEAD commit yields an empty set, which makes every
/// one-hop candidate resolve to nothing — fail-closed, as intended.
pub fn tracked_paths(repo: &git2::Repository) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let tree = match repo.head().and_then(|h| h.peel_to_tree()) {
        Ok(t) => t,
        Err(_) => return out,
    };
    let _ = tree.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
        if entry.kind() == Some(git2::ObjectType::Blob) {
            if let Some(name) = entry.name() {
                let full = format!("{dir}{name}").replace('\\', "/");
                out.insert(full);
            }
        }
        git2::TreeWalkResult::Ok
    });
    out
}

/// Extract the repo metadata `quorum review` needs for the JSON archive.
pub fn repo_metadata(repo: &git2::Repository) -> Result<RepoFacts, GitError> {
    let head = repo.head().map_err(|_| GitError::NoHead)?;
    let commit = head.peel_to_commit()?;
    let head_sha = commit.id().to_string();
    let branch = match head.shorthand() {
        Some(s) => s.to_string(),
        None => "HEAD".to_string(),
    };
    let remote_url = repo
        .find_remote("origin")
        .ok()
        .and_then(|r| r.url().map(|s| s.to_string()))
        .map(strip_https_creds);
    Ok(RepoFacts {
        head_sha,
        branch,
        remote_url,
    })
}

#[derive(Debug, Clone)]
pub struct RepoFacts {
    pub head_sha: String,
    pub branch: String,
    pub remote_url: Option<String>,
}

/// `https://user:pw@host/...` → `https://host/...`. Other URL forms pass through.
pub fn strip_https_creds(url: String) -> String {
    if let Ok(mut parsed) = url::Url::parse(&url) {
        let has_creds = !parsed.username().is_empty() || parsed.password().is_some();
        let http_like = parsed.scheme() == "https" || parsed.scheme() == "http";
        if http_like && has_creds {
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            return parsed.to_string();
        }
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_creds_removes_https_userinfo() {
        assert_eq!(
            strip_https_creds("https://u:p@github.com/o/r".to_string()),
            "https://github.com/o/r"
        );
        assert_eq!(
            strip_https_creds("git@github.com:o/r.git".to_string()),
            "git@github.com:o/r.git"
        );
        assert_eq!(
            strip_https_creds("https://github.com/o/r".to_string()),
            "https://github.com/o/r"
        );
    }

    /// AC 179 — a modification with two separated hunks reports 2.
    #[test]
    fn hunk_count_reports_two_for_two_hunk_modification() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();

        // 40 numbered lines committed as the baseline.
        let base: String = (1..=40).map(|i| format!("line {i}\n")).collect();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, &base).unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_path(std::path::Path::new("a.txt")).unwrap();
        idx.write().unwrap();
        let tree_id = idx.write_tree().unwrap();
        {
            let tree = repo.find_tree(tree_id).unwrap();
            let sig = git2::Signature::now("t", "t@e").unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "base", &tree, &[])
                .unwrap();
        }

        // Edit line 2 and line 38 — far enough apart (more than twice the
        // 3-line context window) that libgit2 emits two distinct hunks
        // rather than coalescing them into one.
        let edited: String = (1..=40)
            .map(|i| match i {
                2 => "line 2 CHANGED\n".to_string(),
                38 => "line 38 CHANGED\n".to_string(),
                _ => format!("line {i}\n"),
            })
            .collect();
        std::fs::write(&f, &edited).unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_path(std::path::Path::new("a.txt")).unwrap();
        idx.write().unwrap();

        let diff = compute(&repo).unwrap();
        let file = diff.files.iter().find(|f| f.path == "a.txt").unwrap();
        assert_eq!(
            file.hunk_count, 2,
            "two separated edits must report 2 hunks; unified diff was:\n{}",
            diff.unified
        );
    }

    #[test]
    fn detect_binary_finds_null_byte() {
        assert!(detect_binary(&[0xff, 0x00, b'h', b'i']));
        assert!(!detect_binary(b"hello world\n"));
    }
}
