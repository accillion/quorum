//! `.gitignore` discipline for `.quorum/dismissals.sqlite*` (v1.0 §4.2.2).
//!
//! Two paths:
//!   - `ensure_ignored`: on first DB creation, check whether the three
//!     concrete sqlite paths (`.sqlite`, `.sqlite-wal`, `.sqlite-shm`)
//!     are already gitignored. If none of them is, append the wildcard
//!     line to `<repo_root>/.gitignore` (create if absent) with an
//!     attribution-comment leader. Returns `Ok(true)` if it wrote.
//!   - `is_tracked`: returns `true` if `.quorum/dismissals.sqlite` is
//!     currently a TRACKED path in the git index (so the caller can warn
//!     loudly — the DB contains free-text notes and must not be shared).

use std::fs;
use std::io::Write;
use std::path::Path;

use git2::{Repository, Status};

const COMMENT_HEADER: &str = "# Quorum dismissals store; see https://github.com/accillion/quorum";
const IGNORE_LINE: &str = ".quorum/dismissals.sqlite*";

/// Idempotent: returns `Ok(true)` if a new line was appended to
/// `.gitignore`; `Ok(false)` if the path was already ignored or the
/// caller is not inside a git repo (in which case we silently no-op —
/// `quorum review` outside a repo errors before this is called).
pub fn ensure_ignored(repo_root: &Path) -> std::io::Result<bool> {
    let repo = match Repository::discover(repo_root) {
        Ok(r) => r,
        Err(_) => return Ok(false),
    };
    let candidates = [
        ".quorum/dismissals.sqlite",
        ".quorum/dismissals.sqlite-wal",
        ".quorum/dismissals.sqlite-shm",
    ];
    let mut already_ignored = true;
    for c in candidates {
        let ignored = repo.status_should_ignore(Path::new(c)).unwrap_or(false);
        if !ignored {
            already_ignored = false;
            break;
        }
    }
    if already_ignored {
        return Ok(false);
    }
    let gitignore_path = repo_root.join(".gitignore");
    let already = if gitignore_path.is_file() {
        fs::read_to_string(&gitignore_path).unwrap_or_default()
    } else {
        String::new()
    };
    let needs_newline_prefix = !already.is_empty() && !already.ends_with('\n');
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&gitignore_path)?;
    if needs_newline_prefix {
        writeln!(f)?;
    }
    if !already.contains(COMMENT_HEADER) {
        writeln!(f, "\n{}", COMMENT_HEADER)?;
    }
    writeln!(f, "{}", IGNORE_LINE)?;
    Ok(true)
}

/// `true` when `<repo_root>/.quorum/dismissals.sqlite` is a tracked path
/// in the git index. The caller should emit a stderr warning if so
/// (v1.0 §4.2.2 — the DB can contain free-text dismissal notes).
pub fn is_tracked(repo_root: &Path) -> bool {
    let repo = match Repository::discover(repo_root) {
        Ok(r) => r,
        Err(_) => return false,
    };
    let path = Path::new(".quorum/dismissals.sqlite");
    let status = match repo.status_file(path) {
        Ok(s) => s,
        Err(_) => return false,
    };
    // INDEX_* flags mean the file is in the index (tracked).
    status.intersects(
        Status::INDEX_NEW
            | Status::INDEX_MODIFIED
            | Status::INDEX_DELETED
            | Status::INDEX_RENAMED
            | Status::INDEX_TYPECHANGE
            | Status::CURRENT,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn make_repo(td: &TempDir) -> PathBuf {
        let root = td.path().to_path_buf();
        let _ = Repository::init(&root).unwrap();
        root
    }

    #[test]
    fn writes_to_empty_repo() {
        let td = TempDir::new().unwrap();
        let root = make_repo(&td);
        let wrote = ensure_ignored(&root).unwrap();
        assert!(wrote);
        let content = fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(content.contains(IGNORE_LINE));
        assert!(content.contains("Quorum dismissals store"));
    }

    #[test]
    fn idempotent_when_already_ignored() {
        let td = TempDir::new().unwrap();
        let root = make_repo(&td);
        fs::write(root.join(".gitignore"), ".quorum/dismissals.sqlite*\n").unwrap();
        let wrote = ensure_ignored(&root).unwrap();
        assert!(!wrote);
    }

    #[test]
    fn appends_with_newline_when_existing_gitignore_has_no_trailing_newline() {
        let td = TempDir::new().unwrap();
        let root = make_repo(&td);
        fs::write(root.join(".gitignore"), "target").unwrap(); // no trailing \n
        let wrote = ensure_ignored(&root).unwrap();
        assert!(wrote);
        let content = fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(content.starts_with("target\n"));
        assert!(content.contains(IGNORE_LINE));
    }

    #[test]
    fn is_tracked_false_for_absent_file() {
        let td = TempDir::new().unwrap();
        let root = make_repo(&td);
        assert!(!is_tracked(&root));
    }
}
