//! Repo-memory file discovery: first-match-wins across CLAUDE.md / AGENTS.md / .cursorrules.

use std::path::{Path, PathBuf};

pub const CANDIDATES: &[&str] = &["CLAUDE.md", "AGENTS.md", ".cursorrules"];

pub struct Discovery {
    /// The chosen file (basename), or `None` if none exist.
    pub chosen: Option<String>,
    /// Other candidates that exist but were not chosen, in stable order.
    pub ignored: Vec<String>,
    /// Absolute path to the chosen file, or `None`.
    pub chosen_path: Option<PathBuf>,
}

pub fn discover(repo_root: &Path) -> Discovery {
    let mut chosen = None;
    let mut chosen_path = None;
    let mut ignored = Vec::new();
    for c in CANDIDATES {
        let p = repo_root.join(c);
        if p.is_file() {
            if chosen.is_none() {
                chosen = Some((*c).to_string());
                chosen_path = Some(p);
            } else {
                ignored.push((*c).to_string());
            }
        }
    }
    Discovery {
        chosen,
        ignored,
        chosen_path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn empty_repo_returns_no_chosen() {
        let d = tempdir().unwrap();
        let r = discover(d.path());
        assert!(r.chosen.is_none());
        assert!(r.ignored.is_empty());
    }

    #[test]
    fn claude_md_wins_when_all_present() {
        let d = tempdir().unwrap();
        std::fs::write(d.path().join("CLAUDE.md"), b"x").unwrap();
        std::fs::write(d.path().join("AGENTS.md"), b"y").unwrap();
        std::fs::write(d.path().join(".cursorrules"), b"z").unwrap();
        let r = discover(d.path());
        assert_eq!(r.chosen.as_deref(), Some("CLAUDE.md"));
        assert_eq!(
            r.ignored,
            vec!["AGENTS.md".to_string(), ".cursorrules".to_string()]
        );
    }

    #[test]
    fn agents_md_wins_when_claude_absent() {
        let d = tempdir().unwrap();
        std::fs::write(d.path().join("AGENTS.md"), b"y").unwrap();
        std::fs::write(d.path().join(".cursorrules"), b"z").unwrap();
        let r = discover(d.path());
        assert_eq!(r.chosen.as_deref(), Some("AGENTS.md"));
        assert_eq!(r.ignored, vec![".cursorrules".to_string()]);
    }
}
