//! Pre-push coverage for a new branch (v0.6 spec AC 277a step 2).
//!
//! A pushed ref the remote does not have yet (`remote_sha` all zero) is
//! reviewed from the merge-base with a tracking ref of the destination
//! remote, but only one the remote still advertises right now. With no
//! such ref, the branch's full tree is reviewed against the empty tree.
//! Never the tip commit only.
//!
//! - Tracking refs are the destinations of the remote's fetch refspecs
//!   (`remote.<name>.fetch`), not a `refs/remotes/<name>/*` pattern: a
//!   remote named `a` must not borrow remote `a/b`'s refs.
//! - A tracking ref is **verified** only if its local value equals the
//!   tip `git ls-remote <remote>` reports for the remote ref it maps
//!   from. That runs the `git` CLI, not libgit2, so the user's
//!   credential helpers apply; it never prompts, is killed after
//!   [`LS_REMOTE_TIMEOUT`], and any failure means nothing is verified.
//! - **Eligible** refs are verified refs with exactly one merge-base with
//!   the pushed commit. `<remote>/HEAD`'s target wins if eligible;
//!   otherwise the fewest commits from merge-base to the pushed commit,
//!   ties by the lowest ref name.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use quorum_core::git::{looks_binary, FileStatus, StagedDiff, StagedFile};

use super::hook_mode::PushDestination;

/// How long `git ls-remote` may take before it is killed and the remote
/// counts as unverifiable (driver ruling for 0.5.1).
pub const LS_REMOTE_TIMEOUT: Duration = Duration::from_secs(15);

/// What a new-branch push reviews.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewBranchPlan {
    /// Review the tree delta from `base` (the merge-base with the
    /// verified tracking ref `tracking_ref`) to the pushed commit.
    MergeBase { base: String, tracking_ref: String },
    /// The merge-base's tree equals the pushed commit's tree: the remote
    /// already has all of it. Pass, with no Lippa session.
    AlreadyOnRemote { tracking_ref: String },
    /// No eligible tracking ref: review the full tree. `reason` says why,
    /// for the stderr notice.
    FullTree { reason: String },
}

/// Decide what a new-branch push of `local_sha` to `dest` reviews.
///
/// `Err` only when the repository or the pushed commit itself cannot be
/// read; every reason a base is unavailable is a [`NewBranchPlan::FullTree`].
pub fn plan_new_branch(
    repo_start: &Path,
    local_sha: &str,
    dest: &PushDestination,
) -> Result<NewBranchPlan, String> {
    let repo = git2::Repository::discover(repo_start).map_err(|e| format!("repo discover: {e}"))?;
    let local_oid = git2::Oid::from_str(local_sha).map_err(|e| format!("oid parse: {e}"))?;
    let local = repo
        .find_commit(local_oid)
        .map_err(|e| format!("local commit {local_sha}: {e}"))?;
    let full = |reason: String| Ok(NewBranchPlan::FullTree { reason });

    let name = match dest {
        PushDestination::NotPassed => {
            return full(
                "the hook did not pass the remote name; reinstall the hook \
                 (`quorum install --hook=pre-push`) for exact coverage"
                    .into(),
            )
        }
        PushDestination::Url => {
            return full("the push names a URL, not a configured remote".into())
        }
        PushDestination::Named(name) => name,
    };
    if repo.is_shallow() {
        return full("this is a shallow clone, so merge-bases are not reliable".into());
    }
    let tracking = match repo.find_remote(name) {
        Ok(remote) => tracking_refs(&repo, &remote),
        Err(_) => Vec::new(),
    };
    if tracking.is_empty() {
        return full(format!("remote `{name}` has no tracking refs here"));
    }
    let workdir = repo.workdir().unwrap_or_else(|| repo.path());
    let Some(advertised) = ls_remote(workdir, name) else {
        return full(format!(
            "`git ls-remote {name}` failed, so no tracking ref could be verified"
        ));
    };
    let verified: Vec<&TrackingRef> = tracking
        .iter()
        .filter(|t| advertised.get(&t.remote_ref) == Some(&t.target))
        .collect();
    if verified.is_empty() {
        return full(format!(
            "no tracking ref of `{name}` matches what the remote has now \
             (run `git fetch {name}`)"
        ));
    }

    let mut eligible: Vec<(&TrackingRef, git2::Oid, usize)> = Vec::new();
    let mut criss_cross = false;
    for t in verified {
        let bases = match repo.merge_bases(local_oid, t.commit) {
            Ok(b) => b,
            // NotFound: no common history.
            Err(_) => continue,
        };
        match bases.len() {
            1 => {
                let base = bases[0];
                eligible.push((t, base, distance(&repo, local_oid, base)));
            }
            0 => {}
            _ => criss_cross = true,
        }
    }
    if eligible.is_empty() {
        return full(if criss_cross {
            format!("no verified tracking ref of `{name}` has a single merge-base with the branch (criss-cross history)")
        } else {
            format!("the branch shares no history with any verified tracking ref of `{name}` (orphan branch)")
        });
    }

    let head_target = repo
        .find_reference(&format!("refs/remotes/{name}/HEAD"))
        .ok()
        .and_then(|r| r.symbolic_target().map(str::to_string));
    let chosen = eligible
        .iter()
        .find(|(t, _, _)| head_target.as_deref() == Some(t.local_ref.as_str()))
        .or_else(|| {
            eligible.iter().min_by(|a, b| {
                a.2.cmp(&b.2)
                    .then_with(|| a.0.local_ref.cmp(&b.0.local_ref))
            })
        })
        .expect("eligible is not empty");
    let (t, base, _) = chosen;
    let base_tree = repo
        .find_commit(*base)
        .map_err(|e| format!("merge-base {base}: {e}"))?
        .tree_id();
    if base_tree == local.tree_id() {
        return Ok(NewBranchPlan::AlreadyOnRemote {
            tracking_ref: t.local_ref.clone(),
        });
    }
    Ok(NewBranchPlan::MergeBase {
        base: base.to_string(),
        tracking_ref: t.local_ref.clone(),
    })
}

/// A local ref that is the destination of one of a remote's fetch
/// refspecs.
#[derive(Debug)]
struct TrackingRef {
    /// e.g. `refs/remotes/origin/main`.
    local_ref: String,
    /// The remote ref it maps from, e.g. `refs/heads/main`.
    remote_ref: String,
    /// The ref's own value, compared with what the remote advertises.
    target: git2::Oid,
    /// The commit it peels to, for merge-bases.
    commit: git2::Oid,
}

/// Every direct local ref matching a fetch refspec destination of
/// `remote`, with the remote ref it maps from. Symbolic refs (such as
/// `<remote>/HEAD`) and negative refspecs are skipped.
fn tracking_refs(repo: &git2::Repository, remote: &git2::Remote<'_>) -> Vec<TrackingRef> {
    let specs: Vec<git2::Refspec<'_>> = remote
        .refspecs()
        .filter(|s| s.direction() == git2::Direction::Fetch)
        .filter(|s| !s.str().is_some_and(|s| s.starts_with('^')))
        .collect();
    let mut out: Vec<TrackingRef> = Vec::new();
    let Ok(refs) = repo.references() else {
        return out;
    };
    for r in refs.flatten() {
        if r.kind() != Some(git2::ReferenceType::Direct) {
            continue;
        }
        let (Some(name), Some(target)) = (r.name(), r.target()) else {
            continue;
        };
        if out.iter().any(|t| t.local_ref == name) {
            continue;
        }
        let Some(remote_ref) = specs
            .iter()
            .filter(|s| s.dst_matches(name))
            .find_map(|s| s.rtransform(name).ok())
            .and_then(|b| b.as_str().map(str::to_string))
        else {
            continue;
        };
        let Ok(commit) = r.peel_to_commit() else {
            continue;
        };
        out.push(TrackingRef {
            local_ref: name.to_string(),
            remote_ref,
            target,
            commit: commit.id(),
        });
    }
    out
}

/// Commits reachable from `head` but not from `base`.
fn distance(repo: &git2::Repository, head: git2::Oid, base: git2::Oid) -> usize {
    let Ok(mut walk) = repo.revwalk() else {
        return usize::MAX;
    };
    if walk.push(head).is_err() || walk.hide(base).is_err() {
        return usize::MAX;
    }
    walk.count()
}

/// `git ls-remote <name>`: the remote's refs and tips right now, or
/// `None` on any failure (spawn error, non-zero exit, timeout,
/// unparseable output). Git's own stderr is discarded, because it may
/// quote the remote's URL with credentials in it.
fn ls_remote(dir: &Path, name: &str) -> Option<HashMap<String, git2::Oid>> {
    if name.starts_with('-') {
        return None;
    }
    let mut cmd = Command::new("git");
    cmd.arg("ls-remote")
        .arg(name)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0");
    let out = run_with_timeout(cmd, LS_REMOTE_TIMEOUT)?;
    let mut refs = HashMap::new();
    for line in String::from_utf8_lossy(&out).lines() {
        let (sha, refname) = line.split_once('\t')?;
        if refname.ends_with("^{}") {
            continue;
        }
        refs.insert(refname.to_string(), git2::Oid::from_str(sha).ok()?);
    }
    Some(refs)
}

/// Run `cmd` with stdin closed and stderr discarded, and return its
/// stdout if it exits 0 within `timeout`. Otherwise kill it and return
/// `None`. A grandchild that keeps stdout open (an `ssh` transport)
/// cannot hold the caller past the deadline either.
fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Option<Vec<u8>> {
    let deadline = Instant::now() + timeout;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let read = stdout.read_to_end(&mut buf).map(|_| buf);
        let _ = tx.send(read);
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    let left = deadline.saturating_duration_since(Instant::now());
    rx.recv_timeout(left.max(Duration::from_millis(100)))
        .ok()?
        .ok()
}

/// The full tree of `head` as a diff against the empty tree (AC 277a step
/// 2e): every file is added. Built here because `quorum-core`'s
/// `DiffSource::CommitRange` needs a base *commit*, and 0.5.1 changes no
/// library API. Same shape and rules as core's commit-range diff:
/// 3 context lines, submodules ignored, binary files listed without
/// content, per-file hunk counts.
pub fn full_tree_diff(
    repo_start: &Path,
    head: &str,
) -> Result<(git2::Repository, StagedDiff), String> {
    let repo = git2::Repository::discover(repo_start).map_err(|e| format!("repo discover: {e}"))?;
    let staged = {
        let head_tree = repo
            .revparse_single(head)
            .and_then(|o| o.peel_to_commit())
            .and_then(|c| c.tree())
            .map_err(|e| format!("{head}: {e}"))?;
        let mut opts = git2::DiffOptions::new();
        opts.context_lines(3)
            .include_untracked(false)
            .ignore_submodules(true);
        let diff = repo
            .diff_tree_to_tree(None, Some(&head_tree), Some(&mut opts))
            .map_err(|e| e.to_string())?;
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
        })
        .map_err(|e| e.to_string())?;
        let mut hunks: HashMap<String, u32> = HashMap::new();
        let _ = diff.foreach(
            &mut |_d, _p| true,
            None,
            Some(&mut |d, _h| {
                if let Some(p) = forward_slashed(&d) {
                    *hunks.entry(p).or_insert(0) += 1;
                }
                true
            }),
            None,
        );
        let mut files = Vec::new();
        for delta in diff.deltas() {
            let Some(path) = forward_slashed(&delta) else {
                continue;
            };
            let (content, is_binary, size_bytes) = match repo.find_blob(delta.new_file().id()) {
                Ok(blob) => {
                    let bytes = blob.content().to_vec();
                    let size = bytes.len() as u64;
                    if blob.is_binary() || looks_binary(&bytes) {
                        (None, true, size)
                    } else {
                        (Some(bytes), false, size)
                    }
                }
                Err(_) => (None, false, 0),
            };
            let hunk_count = if is_binary {
                0
            } else {
                hunks.get(&path).copied().unwrap_or(0)
            };
            files.push(StagedFile {
                path,
                status: FileStatus::Added,
                index_blob: content,
                is_binary,
                size_bytes,
                hunk_count,
            });
        }
        let is_empty = files.is_empty();
        StagedDiff {
            unified,
            files,
            is_empty,
        }
    };
    Ok((repo, staged))
}

fn forward_slashed(d: &git2::DiffDelta<'_>) -> Option<String> {
    d.new_file()
        .path()
        .and_then(|p| p.to_str())
        .map(|s| s.replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Command {
        let mut c = Command::new("sh");
        c.arg("-c").arg(script);
        c
    }

    /// A command that outlives the timeout is killed and yields `None`,
    /// close to the deadline rather than when it would have finished.
    #[test]
    fn run_with_timeout_kills_a_command_that_overruns() {
        let started = Instant::now();
        let out = run_with_timeout(sh("sleep 10; echo late"), Duration::from_millis(300));
        assert_eq!(out, None);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn run_with_timeout_returns_stdout_only_on_success() {
        assert_eq!(
            run_with_timeout(sh("echo hello"), Duration::from_secs(10)).as_deref(),
            Some(&b"hello\n"[..])
        );
        assert_eq!(
            run_with_timeout(sh("echo partial; exit 3"), Duration::from_secs(10)),
            None
        );
    }

    /// A remote name that would parse as an option is never passed to
    /// `git`.
    #[test]
    fn ls_remote_refuses_an_option_like_name() {
        assert_eq!(
            ls_remote(Path::new("."), "--upload-pack=touch /tmp/x"),
            None
        );
    }
}
