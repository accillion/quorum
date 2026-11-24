//! Gate B ruling 2: remove the fix's explanatory comments from a bug
//! case's base, so neither `diff.patch` nor the materialized base shows
//! them.
//!
//! In a bug case the diff is the fix reversed, so a comment-only line on
//! its removed side is a comment the fix added, and the reviewer would see
//! it deleted. This edit deletes those lines from the case base file
//! (stored in the case directory as `base/<path>`, listed in
//! `base_overrides`), recomputes `case_base_tree`, and regenerates
//! `diff.patch` from the edited base to the unchanged case commit tree.
//! `case_commit_tree` does not change. An edit that would leave a `/* */`
//! block unbalanced is refused, so the base still parses.

use std::collections::BTreeMap;
use std::path::Path;

use git2::{DiffFormat, DiffOptions, Oid, Repository};

use crate::corpus::{comment_only_removed_lines, Case};
use crate::materialize::{MaterializeError, Materialized};

#[derive(Debug, Clone, PartialEq)]
pub struct CommentEdit {
    pub files: Vec<String>,
    pub lines_removed: usize,
    pub case_base_tree: String,
    pub diff: String,
}

fn err(case: &Case, msg: impl Into<String>) -> MaterializeError {
    MaterializeError::Case {
        case: case.id().into(),
        msg: msg.into(),
    }
}

fn block_balance(lines: &[&str]) -> i64 {
    lines
        .iter()
        .map(|l| l.matches("/*").count() as i64 - l.matches("*/").count() as i64)
        .sum()
}

/// Compute the edit for a materialized case. `None` if the diff removes no
/// comment-only line.
pub fn comment_edit(
    case: &Case,
    m: &Materialized,
) -> Result<Option<CommentEdit>, MaterializeError> {
    let hits = comment_only_removed_lines(&String::from_utf8_lossy(&case.diff));
    if hits.is_empty() {
        return Ok(None);
    }
    let mut by_file: BTreeMap<String, Vec<(usize, String)>> = BTreeMap::new();
    for (p, n, t) in hits {
        by_file.entry(p).or_default().push((n, t));
    }
    let repo = Repository::open(&m.dir)?;
    let base_tree = repo.find_commit(Oid::from_str(&m.base_commit)?)?.tree()?;
    let case_tree = repo.find_commit(Oid::from_str(&m.case_commit)?)?.tree()?;
    let mut idx = git2::Index::new()?;
    idx.read_tree(&base_tree)?;
    let mut lines_removed = 0;
    let mut edited_files = Vec::new();
    for (path, hits) in &by_file {
        let entry = base_tree
            .get_path(Path::new(path))
            .map_err(|_| err(case, format!("{path} is not in the case base")))?;
        let blob = repo.find_blob(entry.id())?;
        let text = String::from_utf8(blob.content().to_vec())
            .map_err(|_| err(case, format!("{path} is not UTF-8")))?;
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        let mut drop = vec![false; lines.len()];
        let mut dropped: Vec<&str> = Vec::new();
        for (n, t) in hits {
            let i = n
                .checked_sub(1)
                .filter(|&i| i < lines.len())
                .ok_or_else(|| err(case, format!("{path}:{n} is past the end of the base file")))?;
            let have = lines[i].trim_end_matches(['\n', '\r']);
            if have != t.as_str() {
                return Err(err(
                    case,
                    format!("{path}:{n} in the base is not the removed line"),
                ));
            }
            drop[i] = true;
            dropped.push(lines[i]);
        }
        if block_balance(&dropped) != 0 {
            return Err(err(
                case,
                format!("{path}: removing these lines would unbalance a /* */ block; edit by hand"),
            ));
        }
        let new: String = lines
            .iter()
            .zip(&drop)
            .filter(|(_, d)| !**d)
            .map(|(l, _)| *l)
            .collect();
        let oid = repo.blob(new.as_bytes())?;
        let mut e = idx
            .get_path(Path::new(path), 0)
            .ok_or_else(|| err(case, format!("{path} not in the index")))?;
        e.id = oid;
        e.file_size = new.len() as u32;
        idx.add(&e)?;
        let out = case.dir.join("base").join(path);
        std::fs::create_dir_all(out.parent().expect("parent"))?;
        std::fs::write(&out, new.as_bytes())?;
        lines_removed += hits.len();
        edited_files.push(path.clone());
    }
    let new_base = repo.find_tree(idx.write_tree_to(&repo)?)?;
    let mut opts = DiffOptions::new();
    opts.context_lines(3);
    let d = repo.diff_tree_to_tree(Some(&new_base), Some(&case_tree), Some(&mut opts))?;
    let mut patch = Vec::new();
    d.print(DiffFormat::Patch, |_, _, l| {
        if matches!(l.origin(), '+' | '-' | ' ') {
            patch.push(l.origin() as u8);
        }
        patch.extend_from_slice(l.content());
        true
    })?;
    Ok(Some(CommentEdit {
        files: edited_files,
        lines_removed,
        case_base_tree: new_base.id().to_string(),
        diff: String::from_utf8_lossy(&patch).into_owned(),
    }))
}

/// Write the edit into the case: `diff.patch`, and `case.toml`'s
/// `case_base_tree`, `edited`, `edit_reason` and `base_overrides`.
pub fn apply_to_case(case: &Case, e: &CommentEdit) -> std::io::Result<()> {
    std::fs::write(case.dir.join("diff.patch"), e.diff.as_bytes())?;
    let path = case.dir.join("case.toml");
    let text = std::fs::read_to_string(&path)?;
    let mut out = String::new();
    let mut inserted = false;
    let files = e
        .files
        .iter()
        .map(|f| format!("\"{f}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let block = format!(
        "edited = true\nedit_reason = \"Gate B ruling 2 (2026-11-24): {} comment-only line(s) the fix added are removed from the case base, so the reverted diff does not show the fix's explanation.\"\nbase_overrides = [{files}]\n",
        e.lines_removed
    );
    for line in text.split_inclusive('\n') {
        if line.starts_with("case_base_tree") {
            out.push_str(&format!("case_base_tree = \"{}\"\n", e.case_base_tree));
            continue;
        }
        if !inserted && line.starts_with('[') {
            out.push_str(&block);
            out.push('\n');
            inserted = true;
        }
        out.push_str(line);
    }
    if !inserted {
        out.push_str(&block);
    }
    std::fs::write(path, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::materialize::testrepo::*;

    #[test]
    fn a_fix_comment_is_removed_from_the_base_and_the_diff() {
        let up = upstream();
        let td = tempfile::TempDir::new().unwrap();
        let case = case_for(&td.path().join("cases"), &up, "rs-x-01");
        let m =
            crate::materialize::materialize_from(&up.repo, &case, &td.path().join("repos/rs-x-01"))
                .unwrap();
        // No comment in the test case: nothing to do.
        assert!(comment_edit(&case, &m).unwrap().is_none());

        // A case whose diff removes a fix comment: rebuild its base file
        // with the comment, as an upstream fix would have it.
        let commented = "pub fn add(a: i32, b: i32) -> i32 {\n    // Add, do not subtract: the fix.\n    a + b\n}\n";
        let repo = Repository::open(&m.dir).unwrap();
        let mut idx = git2::Index::new().unwrap();
        idx.read_tree(
            &repo
                .find_commit(Oid::from_str(&m.base_commit).unwrap())
                .unwrap()
                .tree()
                .unwrap(),
        )
        .unwrap();
        let mut e = idx.get_path(Path::new("src/lib.rs"), 0).unwrap();
        e.id = repo.blob(commented.as_bytes()).unwrap();
        idx.add(&e).unwrap();
        let tree = repo.find_tree(idx.write_tree_to(&repo).unwrap()).unwrap();
        let sig = git2::Signature::new("t", "t@invalid", &git2::Time::new(0, 0)).unwrap();
        let base2 = repo.commit(None, &sig, &sig, "b", &tree, &[]).unwrap();
        let m2 = Materialized {
            dir: m.dir.clone(),
            base_commit: base2.to_string(),
            case_commit: m.case_commit.clone(),
        };
        let mut c2 = case.clone();
        c2.diff = b"diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,4 +1,3 @@\n pub fn add(a: i32, b: i32) -> i32 {\n-    // Add, do not subtract: the fix.\n-    a + b\n+    a - b\n }\n".to_vec();
        let ed = comment_edit(&c2, &m2).unwrap().unwrap();
        assert_eq!(ed.lines_removed, 1);
        assert_eq!(
            ed.case_base_tree, case.meta.case_base_tree,
            "the edited base is the uncommented base"
        );
        assert!(!ed.diff.contains("the fix."));
        assert!(ed.diff.contains("-    a + b") && ed.diff.contains("+    a - b"));
        assert_eq!(
            std::fs::read_to_string(case.dir.join("base/src/lib.rs")).unwrap(),
            crate::corpus::testcase::BASE_SRC
        );
        // The regenerated patch applies and reproduces the pinned case tree.
        let d = git2::Diff::from_buffer(ed.diff.as_bytes()).unwrap();
        let base_tree = repo
            .find_tree(Oid::from_str(&ed.case_base_tree).unwrap())
            .unwrap();
        let mut applied = repo.apply_to_tree(&base_tree, &d, None).unwrap();
        assert_eq!(
            applied.write_tree_to(&repo).unwrap().to_string(),
            case.meta.case_commit_tree
        );
    }

    #[test]
    fn an_unbalanced_block_comment_edit_is_refused() {
        assert_eq!(block_balance(&["/**\n", " * x\n"]), 1);
        assert_eq!(block_balance(&["/**\n", " * x\n", " */\n"]), 0);
    }
}
