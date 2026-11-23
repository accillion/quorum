//! Materialize a case into `bench/.cache/repos/<case-id>/` (AC 242) and
//! prove it does not leak the fix's tests (AC 244).
//!
//! The case repo is a fresh repository with no remote and no upstream
//! history: two commits on `main`, "case base" and "case", with a fixed
//! author and time, so every run sees the same commit ids. Only the blobs
//! of the case base tree are copied in, so no removed test file exists
//! anywhere in its object database. Both trees are checked against the
//! hashes pinned in `case.toml`.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use git2::{ApplyLocation, Diff, FetchOptions, ObjectType, Oid, Repository, Signature, Time, Tree};

use crate::corpus::Case;

/// 2026-01-01T00:00:00Z: the fixed time of both case commits.
const FIXED_TIME: i64 = 1_767_225_600;

#[derive(thiserror::Error, Debug)]
pub enum MaterializeError {
    #[error("git: {0}")]
    Git(#[from] git2::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{case}: {what} tree is {got}, case.toml pins {want}")]
    TreeMismatch {
        case: String,
        what: &'static str,
        got: String,
        want: String,
    },
    #[error("{case}: {msg}")]
    Case { case: String, msg: String },
}

#[derive(Debug, Clone)]
pub struct Materialized {
    pub dir: PathBuf,
    pub base_commit: String,
    pub case_commit: String,
}

impl Materialized {
    /// The `--range` argument Quorum is run with (AC 242a).
    pub fn range(&self) -> String {
        format!("{}..{}", self.base_commit, self.case_commit)
    }
}

/// The bare upstream cache for a case's source repository.
pub fn upstream_path(cache: &Path, case: &Case) -> PathBuf {
    cache
        .join("upstream")
        .join(format!("{}.git", case.source_repo_key().replace('/', "__")))
}

/// Open (or create) the bare upstream cache and make sure `sha` and, when
/// `with_parent`, its first parent are present, fetching shallowly.
pub fn ensure_upstream(
    cache: &Path,
    case: &Case,
    with_parent: bool,
) -> Result<Repository, MaterializeError> {
    let path = upstream_path(cache, case);
    let repo = match Repository::open_bare(&path) {
        Ok(r) => r,
        Err(_) => {
            std::fs::create_dir_all(&path)?;
            Repository::init_bare(&path)?
        }
    };
    let sha = &case.meta.base_commit;
    let have = |r: &Repository| -> bool {
        let Ok(c) = Oid::from_str(sha).and_then(|o| r.find_commit(o)) else {
            return false;
        };
        !with_parent || c.parent(0).is_ok()
    };
    if !have(&repo) {
        let mut fo = FetchOptions::new();
        fo.depth(if with_parent { 2 } else { 1 });
        let mut remote = repo.remote_anonymous(&case.meta.source_repo)?;
        remote.fetch(&[&format!("+{sha}:refs/bench/{sha}")], Some(&mut fo), None)?;
        if !have(&repo) {
            return Err(MaterializeError::Case {
                case: case.id().into(),
                msg: format!("commit {sha} (with parent: {with_parent}) not available after fetch"),
            });
        }
    }
    Ok(repo)
}

type Reverts = BTreeMap<String, Option<(Oid, i32)>>;

fn copy_tree(
    src: &Repository,
    dst: &Repository,
    tree: &Tree<'_>,
    prefix: &str,
    removed: &BTreeSet<String>,
    reverts: &Reverts,
) -> Result<Option<Oid>, git2::Error> {
    let mut tb = dst.treebuilder(None)?;
    for entry in tree.iter() {
        let name = entry.name().unwrap_or_default().to_string();
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        match entry.kind() {
            Some(ObjectType::Tree) => {
                let sub = src.find_tree(entry.id())?;
                if let Some(oid) = copy_tree(src, dst, &sub, &path, removed, reverts)? {
                    tb.insert(&name, oid, 0o040000)?;
                }
            }
            Some(ObjectType::Blob) => {
                if removed.contains(&path) {
                    continue;
                }
                let (oid, mode) = match reverts.get(&path) {
                    Some(Some(prev)) => *prev,
                    Some(None) => continue,
                    None => (entry.id(), entry.filemode()),
                };
                let blob = src.find_blob(oid)?;
                let new = dst.blob(blob.content())?;
                tb.insert(&name, new, mode)?;
            }
            // A submodule: keep the gitlink, never its content.
            Some(ObjectType::Commit) => {
                tb.insert(&name, entry.id(), 0o160000)?;
            }
            _ => {}
        }
    }
    if tb.is_empty() {
        return Ok(None);
    }
    tb.write().map(Some)
}

/// Build the case repo at `out` from an upstream that already holds the
/// case's commits.
pub fn materialize_from(
    upstream: &Repository,
    case: &Case,
    out: &Path,
) -> Result<Materialized, MaterializeError> {
    let id = case.id().to_string();
    if out.exists() {
        std::fs::remove_dir_all(out)?;
    }
    std::fs::create_dir_all(out)?;
    let dst = Repository::init(out)?;
    {
        let mut cfg = dst.config()?;
        cfg.set_bool("core.autocrlf", false)?;
        cfg.set_bool("core.longpaths", true)?;
    }
    let f = upstream.find_commit(Oid::from_str(&case.meta.base_commit)?)?;
    let mut reverts = Reverts::new();
    if !case.meta.base_reverts.is_empty() {
        let parent_tree = f.parent(0)?.tree()?;
        for p in &case.meta.base_reverts {
            let prev = match parent_tree.get_path(Path::new(p)) {
                Ok(e) => Some((e.id(), e.filemode())),
                Err(_) => None,
            };
            if f.tree()?.get_path(Path::new(p)).is_err() {
                return Err(MaterializeError::Case {
                    case: id,
                    msg: format!("base_reverts path `{p}` is not in the base commit"),
                });
            }
            reverts.insert(p.clone(), prev);
        }
    }
    let removed: BTreeSet<String> = case.meta.removed_paths.iter().cloned().collect();
    let base_tree_oid =
        copy_tree(upstream, &dst, &f.tree()?, "", &removed, &reverts)?.ok_or_else(|| {
            MaterializeError::Case {
                case: id.clone(),
                msg: "empty base tree".into(),
            }
        })?;
    if base_tree_oid.to_string() != case.meta.case_base_tree {
        return Err(MaterializeError::TreeMismatch {
            case: id,
            what: "case base",
            got: base_tree_oid.to_string(),
            want: case.meta.case_base_tree.clone(),
        });
    }
    let sig = Signature::new("quorum-bench", "bench@invalid", &Time::new(FIXED_TIME, 0))?;
    let base_tree = dst.find_tree(base_tree_oid)?;
    let base = dst.commit(
        Some("refs/heads/main"),
        &sig,
        &sig,
        "case base\n",
        &base_tree,
        &[],
    )?;
    dst.set_head("refs/heads/main")?;
    let mut co = git2::build::CheckoutBuilder::new();
    co.force();
    dst.checkout_head(Some(&mut co))?;
    let diff = Diff::from_buffer(&case.diff)?;
    dst.apply(&diff, ApplyLocation::Both, None)?;
    let mut idx = dst.index()?;
    let case_tree_oid = idx.write_tree()?;
    if case_tree_oid.to_string() != case.meta.case_commit_tree {
        return Err(MaterializeError::TreeMismatch {
            case: id,
            what: "case commit",
            got: case_tree_oid.to_string(),
            want: case.meta.case_commit_tree.clone(),
        });
    }
    let case_tree = dst.find_tree(case_tree_oid)?;
    let base_commit = dst.find_commit(base)?;
    let head = dst.commit(
        Some("HEAD"),
        &sig,
        &sig,
        "case\n",
        &case_tree,
        &[&base_commit],
    )?;
    Ok(Materialized {
        dir: out.to_path_buf(),
        base_commit: base.to_string(),
        case_commit: head.to_string(),
    })
}

/// Fetch (if needed) and materialize into `<cache>/repos/<case-id>`.
pub fn materialize(case: &Case, cache: &Path) -> Result<Materialized, MaterializeError> {
    let upstream = ensure_upstream(cache, case, !case.meta.base_reverts.is_empty())?;
    materialize_from(&upstream, case, &cache.join("repos").join(case.id()))
}

/// The repro test files: (repo path, bytes).
pub fn repro_files(case: &Case) -> std::io::Result<Vec<(String, Vec<u8>)>> {
    let Some(r) = &case.meta.repro else {
        return Ok(Vec::new());
    };
    r.test_files
        .iter()
        .map(|t| {
            Ok((
                t.clone(),
                std::fs::read(case.dir.join("repro").join("files").join(t))?,
            ))
        })
        .collect()
}

/// AC 244, repository half: no removed path in either tree or the working
/// directory, and no repro test file's blob anywhere in the object
/// database. Each string is a leak.
pub fn leakage_problems(case: &Case, m: &Materialized) -> Result<Vec<String>, MaterializeError> {
    let repo = Repository::open(&m.dir)?;
    let mut out = Vec::new();
    let base_tree = repo.find_commit(Oid::from_str(&m.base_commit)?)?.tree()?;
    let case_tree = repo.find_commit(Oid::from_str(&m.case_commit)?)?.tree()?;
    for p in &case.meta.removed_paths {
        if base_tree.get_path(Path::new(p)).is_ok() {
            out.push(format!("removed path `{p}` is in the case base tree"));
        }
        if case_tree.get_path(Path::new(p)).is_ok() {
            out.push(format!("removed path `{p}` is in the case commit tree"));
        }
        if m.dir.join(p).exists() {
            out.push(format!("removed path `{p}` is in the working directory"));
        }
    }
    let odb = repo.odb()?;
    for (path, bytes) in repro_files(case)? {
        let oid = Oid::hash_object(ObjectType::Blob, &bytes)?;
        if odb.exists(oid) {
            out.push(format!(
                "repro test file `{path}` is in the object database"
            ));
        }
    }
    Ok(out)
}

fn line_key(l: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    l.hash(&mut h);
    h.finish()
}

/// Minimum length of a trimmed line that counts as distinctive.
pub const DISTINCTIVE_LINE: usize = 30;

/// Hashes of every trimmed line of 30+ characters in the case commit's
/// text files (blobs under 512 KB).
pub fn case_tree_lines(m: &Materialized) -> Result<HashSet<u64>, MaterializeError> {
    let repo = Repository::open(&m.dir)?;
    let tree = repo.find_commit(Oid::from_str(&m.case_commit)?)?.tree()?;
    let mut set = HashSet::new();
    let mut err = None;
    tree.walk(git2::TreeWalkMode::PreOrder, |_, e| {
        if e.kind() == Some(ObjectType::Blob) {
            match repo.find_blob(e.id()) {
                Ok(b) if !b.is_binary() && b.size() < 512 * 1024 => {
                    for l in String::from_utf8_lossy(b.content()).lines() {
                        let t = l.trim();
                        if t.len() >= DISTINCTIVE_LINE {
                            set.insert(line_key(t));
                        }
                    }
                }
                Ok(_) => {}
                Err(x) => {
                    err = Some(x);
                    return git2::TreeWalkResult::Abort;
                }
            }
        }
        git2::TreeWalkResult::Ok
    })?;
    if let Some(e) = err {
        return Err(e.into());
    }
    Ok(set)
}

/// AC 244, bundle half: a distinctive line of a repro test file (one that
/// is not also in the case commit's own files) must not appear in the
/// bundle. Every arm is fed these same bundle bytes.
pub fn bundle_leaks(
    bundle: &str,
    repro: &[(String, Vec<u8>)],
    tree_lines: &HashSet<u64>,
) -> Vec<String> {
    let mut out = Vec::new();
    for (path, bytes) in repro {
        for l in String::from_utf8_lossy(bytes).lines() {
            let t = l.trim();
            if t.len() >= DISTINCTIVE_LINE
                && !tree_lines.contains(&line_key(t))
                && bundle.contains(t)
            {
                out.push(format!("bundle contains a line of repro test `{path}`"));
                break;
            }
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod testrepo {
    //! A local "upstream" with a parent commit (bug) and a fix commit that
    //! adds a test file, matching `corpus::testcase`.
    use super::*;
    use crate::corpus::testcase::{BASE_SRC, BUGGY_SRC, TEST_SRC};

    pub struct Upstream {
        pub _td: tempfile::TempDir,
        pub repo: Repository,
        pub fix: Oid,
    }

    pub fn upstream() -> Upstream {
        let td = tempfile::TempDir::new().unwrap();
        let repo = Repository::init_bare(td.path()).unwrap();
        let sig = Signature::new("u", "u@invalid", &Time::new(1_700_000_000, 0)).unwrap();
        let tree_of = |files: &[(&str, &str)]| -> Oid {
            let mut idx = git2::Index::new().unwrap();
            for (p, body) in files {
                let oid = repo.blob(body.as_bytes()).unwrap();
                let entry = git2::IndexEntry {
                    ctime: git2::IndexTime::new(0, 0),
                    mtime: git2::IndexTime::new(0, 0),
                    dev: 0,
                    ino: 0,
                    mode: 0o100644,
                    uid: 0,
                    gid: 0,
                    file_size: body.len() as u32,
                    id: oid,
                    flags: 0,
                    flags_extended: 0,
                    path: p.as_bytes().to_vec(),
                };
                idx.add(&entry).unwrap();
            }
            idx.write_tree_to(&repo).unwrap()
        };
        let p_tree = repo
            .find_tree(tree_of(&[
                ("src/lib.rs", BUGGY_SRC),
                ("AGENTS.md", "rules\n"),
            ]))
            .unwrap();
        let parent = repo.commit(None, &sig, &sig, "p", &p_tree, &[]).unwrap();
        let f_tree = repo
            .find_tree(tree_of(&[
                ("src/lib.rs", BASE_SRC),
                ("AGENTS.md", "rules\n"),
                ("tests/add.rs", TEST_SRC),
            ]))
            .unwrap();
        let pc = repo.find_commit(parent).unwrap();
        let fix = repo
            .commit(None, &sig, &sig, "fix", &f_tree, &[&pc])
            .unwrap();
        drop((p_tree, f_tree, pc, tree_of));
        Upstream { _td: td, repo, fix }
    }

    /// The case's pinned hashes, computed independently of the
    /// materializer: the fix tree minus the test, then plus the patch.
    pub fn expected_trees(up: &Upstream) -> (String, String) {
        let repo = &up.repo;
        let mut idx = git2::Index::new().unwrap();
        idx.read_tree(&repo.find_commit(up.fix).unwrap().tree().unwrap())
            .unwrap();
        idx.remove_path(Path::new("tests/add.rs")).unwrap();
        let base = idx.write_tree_to(repo).unwrap();
        let buggy = repo.blob(BUGGY_SRC.as_bytes()).unwrap();
        let mut e = idx.get_path(Path::new("src/lib.rs"), 0).unwrap();
        e.id = buggy;
        e.file_size = BUGGY_SRC.len() as u32;
        idx.add(&e).unwrap();
        let case = idx.write_tree_to(repo).unwrap();
        (base.to_string(), case.to_string())
    }

    pub fn case_for(cases_dir: &Path, up: &Upstream, id: &str) -> Case {
        let d = crate::corpus::testcase::write_case(cases_dir, id, "");
        let (base, case) = expected_trees(up);
        let z = "0".repeat(40);
        let t = std::fs::read_to_string(d.join("case.toml")).unwrap();
        let t = t
            .replace(
                &format!("source_commit = \"{z}\""),
                &format!("source_commit = \"{}\"", up.fix),
            )
            .replace(
                &format!("base_commit = \"{z}\""),
                &format!("base_commit = \"{}\"", up.fix),
            )
            .replace(
                &format!("case_base_tree = \"{z}\""),
                &format!("case_base_tree = \"{base}\""),
            )
            .replace(
                &format!("case_commit_tree = \"{z}\""),
                &format!("case_commit_tree = \"{case}\""),
            );
        std::fs::write(d.join("case.toml"), t).unwrap();
        crate::corpus::load_case(&d).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::testrepo::*;
    use super::*;

    #[test]
    fn materializes_two_pinned_commits_with_no_remote() {
        let up = upstream();
        let td = tempfile::TempDir::new().unwrap();
        let case = case_for(&td.path().join("cases"), &up, "rs-x-01");
        let out = td.path().join("repos/rs-x-01");
        let m = materialize_from(&up.repo, &case, &out).unwrap();
        let repo = Repository::open(&out).unwrap();
        assert!(repo.remotes().unwrap().is_empty());
        let head = repo.head().unwrap();
        assert_eq!(head.shorthand(), Some("main"));
        assert_eq!(head.target().unwrap().to_string(), m.case_commit);
        assert_eq!(
            std::fs::read_to_string(out.join("src/lib.rs")).unwrap(),
            crate::corpus::testcase::BUGGY_SRC
        );
        assert!(!out.join("tests/add.rs").exists());
        assert!(leakage_problems(&case, &m).unwrap().is_empty());
        // Deterministic: materializing again gives the same commit ids.
        let m2 = materialize_from(&up.repo, &case, &out).unwrap();
        assert_eq!(
            (m.base_commit, m.case_commit),
            (m2.base_commit, m2.case_commit)
        );
    }

    #[test]
    fn negative_control_a_wrong_pinned_tree_is_refused() {
        let up = upstream();
        let td = tempfile::TempDir::new().unwrap();
        let mut case = case_for(&td.path().join("cases"), &up, "rs-x-01");
        case.meta.case_base_tree = "1".repeat(40);
        let e = materialize_from(&up.repo, &case, &td.path().join("r")).unwrap_err();
        assert!(matches!(
            e,
            MaterializeError::TreeMismatch {
                what: "case base",
                ..
            }
        ));
    }

    #[test]
    fn negative_control_a_test_left_in_the_base_is_a_leak() {
        let up = upstream();
        let td = tempfile::TempDir::new().unwrap();
        let mut case = case_for(&td.path().join("cases"), &up, "rs-x-01");
        // Pretend the author forgot to remove the test: pin the trees the
        // materializer would then build.
        case.meta.removed_paths.clear();
        let mut idx = git2::Index::new().unwrap();
        idx.read_tree(&up.repo.find_commit(up.fix).unwrap().tree().unwrap())
            .unwrap();
        case.meta.case_base_tree = idx.write_tree_to(&up.repo).unwrap().to_string();
        let buggy = up
            .repo
            .blob(crate::corpus::testcase::BUGGY_SRC.as_bytes())
            .unwrap();
        let mut e = idx.get_path(Path::new("src/lib.rs"), 0).unwrap();
        e.id = buggy;
        idx.add(&e).unwrap();
        case.meta.case_commit_tree = idx.write_tree_to(&up.repo).unwrap().to_string();
        let m = materialize_from(&up.repo, &case, &td.path().join("r")).unwrap();
        let p = leakage_problems(&case, &m).unwrap();
        assert!(p.iter().any(|x| x.contains("object database")), "{p:?}");
    }

    #[test]
    fn bundle_leak_detection() {
        let up = upstream();
        let td = tempfile::TempDir::new().unwrap();
        let case = case_for(&td.path().join("cases"), &up, "rs-x-01");
        let m = materialize_from(&up.repo, &case, &td.path().join("r")).unwrap();
        let lines = case_tree_lines(&m).unwrap();
        let repro = repro_files(&case).unwrap();
        assert!(bundle_leaks("diff --git a/src/lib.rs ...", &repro, &lines).is_empty());
        let leaky = format!("context\n{}\n", crate::corpus::testcase::TEST_SRC);
        assert_eq!(
            bundle_leaks(&leaky, &repro, &lines).len(),
            1,
            "negative control"
        );
    }
}
