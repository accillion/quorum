//! Reproduction trees (AC 244). For a bug case, `fixed/` is the case base
//! (the fixed tree minus its tests, with any Gate B comment edits) plus
//! the repro test files, and `reverted/` is the case commit plus the same
//! files. The case's repro command must pass on `fixed/` and fail on
//! `reverted/`. Used to re-check reproduction after a base edit.

use std::path::Path;

use git2::{Oid, Repository};

use crate::corpus::Case;
use crate::materialize::{repro_files, MaterializeError, Materialized};

fn write_tree(
    repo: &Repository,
    commit: &str,
    out: &Path,
    case: &Case,
) -> Result<(), MaterializeError> {
    if out.exists() {
        std::fs::remove_dir_all(out)?;
    }
    std::fs::create_dir_all(out)?;
    let tree = repo.find_commit(Oid::from_str(commit)?)?.tree()?;
    let mut co = git2::build::CheckoutBuilder::new();
    co.force().target_dir(out).recreate_missing(true);
    repo.checkout_tree(tree.as_object(), Some(&mut co))?;
    for (path, bytes) in repro_files(case)? {
        let p = out.join(&path);
        std::fs::create_dir_all(p.parent().expect("parent"))?;
        std::fs::write(p, bytes)?;
    }
    Ok(())
}

/// Write `<out>/fixed` and `<out>/reverted` for a materialized bug case.
pub fn export_trees(case: &Case, m: &Materialized, out: &Path) -> Result<(), MaterializeError> {
    let repo = Repository::open(&m.dir)?;
    write_tree(&repo, &m.base_commit, &out.join("fixed"), case)?;
    write_tree(&repo, &m.case_commit, &out.join("reverted"), case)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::materialize::testrepo::*;

    #[test]
    fn fixed_and_reverted_trees_carry_the_repro_tests() {
        let up = upstream();
        let td = tempfile::TempDir::new().unwrap();
        let case = case_for(&td.path().join("cases"), &up, "rs-x-01");
        let m =
            crate::materialize::materialize_from(&up.repo, &case, &td.path().join("repos/rs-x-01"))
                .unwrap();
        let out = td.path().join("out");
        export_trees(&case, &m, &out).unwrap();
        assert_eq!(
            std::fs::read_to_string(out.join("fixed/src/lib.rs")).unwrap(),
            crate::corpus::testcase::BASE_SRC
        );
        assert_eq!(
            std::fs::read_to_string(out.join("reverted/src/lib.rs")).unwrap(),
            crate::corpus::testcase::BUGGY_SRC
        );
        for t in ["fixed", "reverted"] {
            assert_eq!(
                std::fs::read_to_string(out.join(t).join("tests/add.rs")).unwrap(),
                crate::corpus::testcase::TEST_SRC
            );
        }
    }
}
