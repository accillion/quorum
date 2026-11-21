//! `.gitignore` discipline for `.quorum/dismissals.sqlite*` (v1.0 §4.2.2).
//!
//! Two paths:
//!   - `ensure_ignored`: on first DB creation, check whether the three
//!     concrete sqlite paths (`.sqlite`, `.sqlite-wal`, `.sqlite-shm`)
//!     are already gitignored. If none of them is, append the wildcard
//!     line to `<repo_root>/.gitignore` (create if absent) with an
//!     attribution-comment leader. Returns `Ok(true)` if it wrote.
//!   - `tracked_paths` / `is_tracked`: which store files are in the git
//!     index (so the caller can warn loudly — the DB contains free-text
//!     notes and must not be shared).
//!   - `open_messages`: both of the above, as the stderr lines the store
//!     prints on open.

use std::fs;
use std::io::Write;
use std::path::Path;

use git2::Repository;

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

/// The store's files. The main file and the WAL both hold dismissal notes
/// (the WAL keeps recent writes until a checkpoint folds them in).
const STORE_PATHS: [&str; 3] = [
    ".quorum/dismissals.sqlite",
    ".quorum/dismissals.sqlite-wal",
    ".quorum/dismissals.sqlite-shm",
];

/// The store files that are in the git index, and so tracked.
///
/// This tests index membership directly. It used to test status flags,
/// which cannot work: a committed, unmodified file has status
/// `Status::CURRENT`, the empty bit set, and a committed file that has
/// since changed shows only `WT_MODIFIED`. Neither matched, so the
/// warning fired only for a file staged but not yet committed.
pub fn tracked_paths(repo_root: &Path) -> Vec<&'static str> {
    let Ok(repo) = Repository::discover(repo_root) else {
        return Vec::new();
    };
    let Ok(index) = repo.index() else {
        return Vec::new();
    };
    STORE_PATHS
        .into_iter()
        .filter(|p| index.get_path(Path::new(p), 0).is_some())
        .collect()
}

/// `true` when any store file is tracked by git (v1.0 §4.2.2: the DB can
/// contain free-text dismissal notes).
pub fn is_tracked(repo_root: &Path) -> bool {
    !tracked_paths(repo_root).is_empty()
}

/// The stderr lines the store prints about git hygiene when it opens,
/// after appending the ignore line if one is needed ([`ensure_ignored`]).
///
/// A `.gitignore` entry stops git picking up an untracked file but does
/// nothing for a file git already tracks. So when a store file is
/// tracked, the "wrote .gitignore" note must not read as the fix, and
/// the warning says what does fix it.
pub fn open_messages(repo_root: &Path) -> Vec<String> {
    let tracked = tracked_paths(repo_root);
    let wrote = ensure_ignored(repo_root).unwrap_or(false);
    let mut out = Vec::new();
    if wrote {
        if tracked.is_empty() {
            out.push(format!("wrote .gitignore: {IGNORE_LINE}"));
        } else {
            out.push(format!(
                "wrote .gitignore: {IGNORE_LINE} (this does not untrack files git already \
                 tracks; see the warning below)"
            ));
        }
    }
    if !tracked.is_empty() {
        out.push(tracked_warning(&tracked));
    }
    out
}

fn tracked_warning(tracked: &[&str]) -> String {
    let subject = match tracked {
        [one] => format!("{one} is"),
        many => format!("{} are", many.join(", ")),
    };
    format!(
        "warning: {subject} tracked by git, and the dismissals store holds your free-text \
         dismissal notes. A .gitignore entry does not untrack a file git already tracks: \
         every commit that includes it still records the notes. Untrack it with\n\
         \n    git rm --cached {}\n\n\
         and commit the removal. Copies already pushed stay in the remote's history.",
        tracked.join(" ")
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

    /// Stage and commit `paths` (relative to `root`), bypassing ignore
    /// rules the way `git add -f` does.
    fn commit_paths(root: &Path, paths: &[&str]) {
        let repo = Repository::open(root).unwrap();
        let mut index = repo.index().unwrap();
        for p in paths {
            index.add_path(Path::new(p)).unwrap();
        }
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("test", "test@example.invalid").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "commit the db", &tree, &[])
            .unwrap();
    }

    fn write_db(root: &Path, bytes: &[u8]) {
        fs::create_dir_all(root.join(".quorum")).unwrap();
        fs::write(root.join(".quorum/dismissals.sqlite"), bytes).unwrap();
    }

    /// The state that matters: the DB committed and unchanged since. Its
    /// git status is `Status::CURRENT`, which is the empty bit set, so a
    /// status mask can never match it.
    #[test]
    fn is_tracked_true_for_committed_unmodified_db() {
        let td = TempDir::new().unwrap();
        let root = make_repo(&td);
        write_db(&root, b"notes");
        commit_paths(&root, &[".quorum/dismissals.sqlite"]);
        assert!(is_tracked(&root), "a committed, unmodified DB is tracked");
    }

    /// The DB changes on every write, so after a commit it is usually
    /// modified in the working tree too. Still tracked.
    #[test]
    fn is_tracked_true_for_committed_then_modified_db() {
        let td = TempDir::new().unwrap();
        let root = make_repo(&td);
        write_db(&root, b"notes");
        commit_paths(&root, &[".quorum/dismissals.sqlite"]);
        write_db(&root, b"more notes");
        assert!(is_tracked(&root), "a committed, modified DB is tracked");
    }

    #[test]
    fn open_messages_for_a_committed_db_do_not_present_gitignore_as_the_fix() {
        let td = TempDir::new().unwrap();
        let root = make_repo(&td);
        write_db(&root, b"notes");
        commit_paths(&root, &[".quorum/dismissals.sqlite"]);

        let msgs = open_messages(&root);
        assert_eq!(msgs.len(), 2, "{msgs:#?}");
        assert!(
            msgs[0].starts_with("wrote .gitignore") && msgs[0].contains("does not untrack"),
            "the .gitignore note must be qualified: {}",
            msgs[0]
        );
        assert!(msgs[1].contains(".quorum/dismissals.sqlite is tracked by git"));
        assert!(msgs[1].contains("does not untrack"));
        assert!(msgs[1].contains("git rm --cached .quorum/dismissals.sqlite\n"));
    }

    #[test]
    fn open_messages_for_an_untracked_db_only_note_the_gitignore_write() {
        let td = TempDir::new().unwrap();
        let root = make_repo(&td);
        write_db(&root, b"notes");
        assert_eq!(
            open_messages(&root),
            vec![format!("wrote .gitignore: {IGNORE_LINE}")]
        );
        assert!(open_messages(&root).is_empty(), "second open is silent");
    }
}
