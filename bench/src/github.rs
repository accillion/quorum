//! Fetch a source commit's tree from GitHub into the bench object cache,
//! byte-exact, over HTTPS with rustls (the workspace's git2 is built
//! without a TLS transport, and Quorum's tech stack rules out shelling
//! out to git).
//!
//! 1. Ask the API for the commit's tree id (one request per commit).
//! 2. Download the commit's source tarball and write every file as a blob,
//!    with its mode (executable, symlink), then build the tree.
//! 3. If that tree is not the commit's tree (a submodule gitlink, an
//!    `export-ignore` or `export-subst` file the tarball leaves out or
//!    changes), rebuild it from the API's recursive tree listing, taking a
//!    blob from the tarball only when its hash matches and fetching any
//!    other blob raw from the API.
//!
//! The result must hash to the commit's tree id, so no content is trusted
//! that git would not produce. `GITHUB_TOKEN`, if the user has set it, is
//! sent as a bearer token to raise the API rate limit; it is never printed.

use std::collections::BTreeMap;
use std::io::Read;

use git2::{ObjectType, Oid, Repository};
use serde_json::Value;

#[derive(thiserror::Error, Debug)]
pub enum GhError {
    #[error("`{0}` is not a github.com repository URL")]
    NotGitHub(String),
    #[error("GitHub {what}: HTTP {status}")]
    Http { what: String, status: u16 },
    #[error("GitHub: {0}")]
    Transport(String),
    #[error("git: {0}")]
    Git(#[from] git2::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Bad(String),
}

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("quorum-bench")
        .timeout(std::time::Duration::from_secs(900))
        .build()
        .expect("reqwest client")
}

pub fn owner_repo(url: &str) -> Result<(String, String), GhError> {
    let rest = url
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .strip_prefix("https://github.com/")
        .ok_or_else(|| GhError::NotGitHub(url.into()))?;
    match rest.split('/').collect::<Vec<_>>().as_slice() {
        [o, r] if !o.is_empty() && !r.is_empty() => Ok((o.to_string(), r.to_string())),
        _ => Err(GhError::NotGitHub(url.into())),
    }
}

async fn get(
    http: &reqwest::Client,
    url: &str,
    accept: &str,
    what: &str,
) -> Result<Vec<u8>, GhError> {
    let mut req = http.get(url).header("accept", accept);
    if url.starts_with("https://api.github.com/") {
        if let Ok(t) = std::env::var("GITHUB_TOKEN") {
            if !t.is_empty() {
                req = req.bearer_auth(t);
            }
        }
    }
    let r = req
        .send()
        .await
        .map_err(|e| GhError::Transport(e.without_url().to_string()))?;
    let status = r.status().as_u16();
    if status != 200 {
        return Err(GhError::Http {
            what: what.into(),
            status,
        });
    }
    Ok(r.bytes()
        .await
        .map_err(|e| GhError::Transport(e.without_url().to_string()))?
        .to_vec())
}

async fn api_json(http: &reqwest::Client, url: &str, what: &str) -> Result<Value, GhError> {
    let b = get(http, url, "application/vnd.github+json", what).await?;
    serde_json::from_slice(&b).map_err(|e| GhError::Bad(format!("{what}: {e}")))
}

/// The commit's tree id and parents, from the API.
pub async fn commit_info(
    http: &reqwest::Client,
    source_repo: &str,
    sha: &str,
) -> Result<(String, Vec<String>), GhError> {
    let (o, r) = owner_repo(source_repo)?;
    let v = api_json(
        http,
        &format!("https://api.github.com/repos/{o}/{r}/git/commits/{sha}"),
        "commit",
    )
    .await?;
    let tree = v["tree"]["sha"]
        .as_str()
        .ok_or_else(|| GhError::Bad("commit without tree".into()))?
        .to_string();
    let parents = v["parents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["sha"].as_str().map(str::to_string))
        .collect();
    Ok((tree, parents))
}

pub async fn parent_of(
    http: &reqwest::Client,
    source_repo: &str,
    sha: &str,
) -> Result<String, GhError> {
    commit_info(http, source_repo, sha)
        .await?
        .1
        .into_iter()
        .next()
        .ok_or_else(|| GhError::Bad(format!("{sha} has no parent")))
}

/// libgit2's in-memory pack (`materialize::packed`) dumps only objects
/// reachable from a commit written into it, so a tree built there is
/// anchored by an unreferenced commit object. Returns the tree id.
fn anchored(repo: &Repository, tree: Oid) -> Result<Oid, GhError> {
    let sig = git2::Signature::new("quorum-bench", "bench@invalid", &git2::Time::new(0, 0))?;
    repo.commit(
        None,
        &sig,
        &sig,
        "bench cache anchor\n",
        &repo.find_tree(tree)?,
        &[],
    )?;
    Ok(tree)
}

/// Entries of a tree under construction: path to (oid, filemode).
pub type Entries = BTreeMap<String, (Oid, i32)>;

/// Build nested trees from flat paths.
pub fn build_tree(repo: &Repository, entries: &Entries) -> Result<Oid, git2::Error> {
    /// A path relative to the current level, and its entry.
    type Flat<'a> = (&'a str, (Oid, i32));
    fn level(repo: &Repository, entries: &[Flat<'_>]) -> Result<Oid, git2::Error> {
        let mut tb = repo.treebuilder(None)?;
        let mut dirs: BTreeMap<&str, Vec<Flat>> = BTreeMap::new();
        for (p, e) in entries {
            match p.split_once('/') {
                Some((d, rest)) => dirs.entry(d).or_default().push((rest, *e)),
                None => {
                    tb.insert(p, e.0, e.1)?;
                }
            }
        }
        for (d, sub) in dirs {
            let oid = level(repo, &sub)?;
            tb.insert(d, oid, 0o040000)?;
        }
        tb.write()
    }
    let flat: Vec<Flat> = entries.iter().map(|(p, e)| (p.as_str(), *e)).collect();
    level(repo, &flat)
}

/// Write every file of a GitHub source tarball as a blob; return the
/// entries by repo path.
pub fn tarball_entries(repo: &Repository, gz: &[u8]) -> Result<Entries, GhError> {
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(gz));
    let mut out = Entries::new();
    for e in ar.entries()? {
        let mut e = e?;
        let kind = e.header().entry_type();
        let path = e.path()?.to_string_lossy().replace('\\', "/");
        // Strip the `<repo>-<sha>/` top directory.
        let Some((_, rel)) = path.split_once('/') else {
            continue;
        };
        if rel.is_empty() {
            continue;
        }
        if kind.is_symlink() {
            let target = e
                .link_name()?
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            out.insert(rel.to_string(), (repo.blob(target.as_bytes())?, 0o120000));
        } else if kind.is_file() {
            let mode = if e.header().mode()? & 0o100 != 0 {
                0o100755
            } else {
                0o100644
            };
            let mut buf = Vec::new();
            e.read_to_end(&mut buf)?;
            out.insert(rel.to_string(), (repo.blob(&buf)?, mode));
        }
    }
    Ok(out)
}

fn mode_of(s: &str) -> Option<i32> {
    i32::from_str_radix(s, 8).ok()
}

fn map_path(repo: &Repository) -> std::path::PathBuf {
    repo.path().join("bench-trees.json")
}

fn load_map(repo: &Repository) -> BTreeMap<String, String> {
    std::fs::read_to_string(map_path(repo))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// The tree of `commit`'s source tarball, with no API call. It is not yet
/// proven to be the commit's tree: the caller proves it by checking the
/// pinned case trees and falls back to [`ensure_tree`] if they differ.
pub async fn tarball_tree(
    http: &reqwest::Client,
    repo: &Repository,
    source_repo: &str,
    commit: &str,
) -> Result<Oid, GhError> {
    let key = format!("tarball:{commit}");
    let mut map = load_map(repo);
    if let Some(t) = map.get(&key).or_else(|| map.get(commit)) {
        let oid = Oid::from_str(t)?;
        if repo.find_tree(oid).is_ok() {
            return Ok(oid);
        }
    }
    let (o, r) = owner_repo(source_repo)?;
    let gz = get(
        http,
        &format!("https://codeload.github.com/{o}/{r}/tar.gz/{commit}"),
        "application/octet-stream",
        "tarball",
    )
    .await?;
    let got = crate::materialize::packed(repo, || -> Result<Oid, GhError> {
        let tar_entries = tarball_entries(repo, &gz)?;
        anchored(repo, build_tree(repo, &tar_entries)?)
    })?;
    repo.odb()?.refresh()?;
    map.insert(key, got.to_string());
    std::fs::write(
        map_path(repo),
        serde_json::to_string_pretty(&map).unwrap_or_default(),
    )?;
    Ok(got)
}

/// Make sure the tree of `commit` is in `repo`, proven against the API's
/// tree id; return its id.
pub async fn ensure_tree(
    http: &reqwest::Client,
    repo: &Repository,
    source_repo: &str,
    commit: &str,
) -> Result<Oid, GhError> {
    let mut map = load_map(repo);
    let (o, r) = owner_repo(source_repo)?;
    let (want, _) = commit_info(http, source_repo, commit).await?;
    let want = Oid::from_str(&want)?;
    if repo.find_tree(want).is_ok() {
        map.insert(commit.to_string(), want.to_string());
        std::fs::write(
            map_path(repo),
            serde_json::to_string_pretty(&map).unwrap_or_default(),
        )?;
        return Ok(want);
    }
    let gz = get(
        http,
        &format!("https://codeload.github.com/{o}/{r}/tar.gz/{commit}"),
        "application/octet-stream",
        "tarball",
    )
    .await?;
    let mut got = crate::materialize::packed(repo, || -> Result<Oid, GhError> {
        let tar_entries = tarball_entries(repo, &gz)?;
        anchored(repo, build_tree(repo, &tar_entries)?)
    })?;
    if got != want {
        let v = api_json(
            http,
            &format!("https://api.github.com/repos/{o}/{r}/git/trees/{want}?recursive=1"),
            "tree listing",
        )
        .await?;
        if v["truncated"].as_bool() == Some(true) {
            return Err(GhError::Bad(format!(
                "{o}/{r}: the tree listing is truncated"
            )));
        }
        // `repo` keeps the (now empty) in-memory backend `packed` added,
        // and a write through it would be lost: the fallback writes
        // through a second handle, in its own pack.
        let repo2 = Repository::open_bare(repo.path())?;
        let odb = repo2.odb()?;
        let mut entries = Entries::new();
        let mut raws: Vec<(Oid, Vec<u8>)> = Vec::new();
        for e in v["tree"].as_array().into_iter().flatten() {
            let (Some(path), Some(mode), Some(kind), Some(sha)) = (
                e["path"].as_str(),
                e["mode"].as_str(),
                e["type"].as_str(),
                e["sha"].as_str(),
            ) else {
                continue;
            };
            let oid = Oid::from_str(sha)?;
            let mode = mode_of(mode).ok_or_else(|| GhError::Bad(format!("mode {mode}")))?;
            match kind {
                "tree" => continue,
                "commit" => {}
                _ if odb.exists(oid) => {}
                _ => {
                    let raw = get(
                        http,
                        &format!("https://api.github.com/repos/{o}/{r}/git/blobs/{sha}"),
                        "application/vnd.github.raw+json",
                        "blob",
                    )
                    .await?;
                    let w = Oid::hash_object(ObjectType::Blob, &raw)?;
                    if w != oid {
                        return Err(GhError::Bad(format!("blob {sha} hashed to {w}")));
                    }
                    raws.push((oid, raw));
                }
            }
            entries.insert(path.to_string(), (oid, mode));
        }
        got = crate::materialize::packed(&repo2, || -> Result<Oid, GhError> {
            for (_, raw) in &raws {
                repo2.blob(raw)?;
            }
            anchored(&repo2, build_tree(&repo2, &entries)?)
        })?;
        repo.odb()?.refresh()?;
        if got != want {
            return Err(GhError::Bad(format!(
                "{o}/{r}@{commit}: rebuilt tree {got} is not {want}"
            )));
        }
    }
    if repo.find_tree(got).is_err() {
        return Err(GhError::Bad(format!(
            "tree {got} was built but is not readable from the cache"
        )));
    }
    map.insert(commit.to_string(), got.to_string());
    std::fs::write(
        map_path(repo),
        serde_json::to_string_pretty(&map).unwrap_or_default(),
    )?;
    Ok(got)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_urls() {
        assert_eq!(
            owner_repo("https://github.com/oxc-project/oxc").unwrap(),
            ("oxc-project".into(), "oxc".into())
        );
        assert_eq!(
            owner_repo("https://github.com/a/b.git/").unwrap(),
            ("a".into(), "b".into())
        );
        assert!(owner_repo("https://gitlab.com/a/b").is_err());
        assert!(owner_repo("https://github.com/a").is_err());
    }

    /// A tarball in GitHub's layout builds the same tree git builds.
    #[test]
    fn tarball_tree_matches_git() {
        let td = tempfile::TempDir::new().unwrap();
        let repo = Repository::init_bare(td.path()).unwrap();
        let mut b = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        ));
        let mut add = |path: &str, body: &[u8], mode: u32| {
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(mode);
            h.set_cksum();
            b.append_data(&mut h, path, body).unwrap();
        };
        add("r-abc/src/lib.rs", b"fn a() {}\n", 0o644);
        add("r-abc/run.sh", b"#!/bin/sh\n", 0o755);
        add("r-abc/AGENTS.md", b"rules\n", 0o644);
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_size(0);
        b.append_link(&mut h, "r-abc/CLAUDE.md", "AGENTS.md")
            .unwrap();
        let gz = b.into_inner().unwrap().finish().unwrap();
        let e = tarball_entries(&repo, &gz).unwrap();
        assert_eq!(e["run.sh"].1, 0o100755);
        assert_eq!(e["CLAUDE.md"].1, 0o120000);
        let got = build_tree(&repo, &e).unwrap();
        // The same tree through git's own index.
        let mut idx = git2::Index::new().unwrap();
        for (p, (oid, mode)) in &e {
            let entry = git2::IndexEntry {
                ctime: git2::IndexTime::new(0, 0),
                mtime: git2::IndexTime::new(0, 0),
                dev: 0,
                ino: 0,
                mode: *mode as u32,
                uid: 0,
                gid: 0,
                file_size: 0,
                id: *oid,
                flags: 0,
                flags_extended: 0,
                path: p.as_bytes().to_vec(),
            };
            idx.add(&entry).unwrap();
        }
        assert_eq!(got, idx.write_tree_to(&repo).unwrap());
    }
}
