//! `.quorum/conventions.md` — trusted only if committed to git AND clean.
//!
//! Three states (spec §4.3.2):
//!   - Absent → returns `Ok(None)`, silent.
//!   - Present in worktree but not tracked OR with uncommitted changes
//!     → returns `Ok(None)`, caller may emit the "untracked/uncommitted"
//!     note.
//!   - Present, tracked, and byte-identical to `HEAD:.quorum/conventions.md`
//!     → returns `Ok(Some(content))`.

use std::path::Path;

pub enum ConventionsState {
    /// File absent in the worktree. Caller must NOT emit a note.
    Absent,
    /// File present in the worktree but ignored by Quorum. Caller emits
    /// `note: .quorum/conventions.md present but not committed ...`.
    PresentButIgnored,
    /// File trusted and ready to bundle.
    Trusted(String),
}

impl ConventionsState {
    /// Per-call helper for the bundle path (§6.2 bridge): `true` iff
    /// `.quorum/conventions.md` is tracked AND HEAD blob is byte-identical
    /// to the worktree. The bundle assembler consults this once per
    /// invocation to decide whether `promoted_convention` rows render in
    /// the conventions section (trusted) or fall back to the memory
    /// section (dirty/uncommitted/missing). No caching — `load()` is the
    /// source of truth and re-runs on every bundle assembly.
    pub fn is_trusted(&self) -> bool {
        matches!(self, ConventionsState::Trusted(_))
    }
}

pub fn load(repo_root: &Path) -> Result<ConventionsState, git2::Error> {
    let rel = ".quorum/conventions.md";
    let path = repo_root.join(".quorum").join("conventions.md");
    if !path.exists() {
        return Ok(ConventionsState::Absent);
    }
    // Open repo at root.
    let repo = git2::Repository::open(repo_root)?;
    let head = repo.head()?.peel_to_commit()?;
    let tree = head.tree()?;
    let entry = match tree.get_path(std::path::Path::new(rel)) {
        Ok(e) => e,
        Err(_) => return Ok(ConventionsState::PresentButIgnored),
    };
    let object = entry.to_object(&repo)?;
    let blob = match object.as_blob() {
        Some(b) => b,
        None => return Ok(ConventionsState::PresentButIgnored),
    };
    let head_bytes = blob.content().to_vec();

    let disk_bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => return Ok(ConventionsState::PresentButIgnored),
    };
    if head_bytes == disk_bytes {
        let content = String::from_utf8(disk_bytes).unwrap_or_default();
        Ok(ConventionsState::Trusted(content))
    } else {
        Ok(ConventionsState::PresentButIgnored)
    }
}
