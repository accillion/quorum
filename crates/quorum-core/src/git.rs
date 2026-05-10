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

/// Result of inspecting the staged diff.
pub struct StagedDiff {
    /// Unified diff text (HEAD vs index), 3 context lines, full repo scope.
    pub unified: String,
    /// Per-file entries with index blob contents (post deny-list, post binary).
    pub files: Vec<StagedFile>,
    /// `true` if no staged changes exist.
    pub is_empty: bool,
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

        files.push(StagedFile {
            path,
            status,
            index_blob,
            is_binary,
            size_bytes,
        });
    }

    let is_empty = files.is_empty();
    Ok(StagedDiff {
        unified,
        files,
        is_empty,
    })
}

fn detect_binary(bytes: &[u8]) -> bool {
    let window_end = bytes.len().min(BINARY_DETECTION_NULLBYTE_WINDOW);
    bytes[..window_end].contains(&0)
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

    #[test]
    fn detect_binary_finds_null_byte() {
        assert!(detect_binary(&[0xff, 0x00, b'h', b'i']));
        assert!(!detect_binary(b"hello world\n"));
    }
}
