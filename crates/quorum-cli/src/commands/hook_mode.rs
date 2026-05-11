//! `quorum review --hook-mode=<type>` dispatcher (Phase 1B Stage 3).
//!
//! - `pre-commit` is a one-shot review of the staged diff. We mostly
//!   delegate to [`super::review::run`] with [`HookMode::PreCommit`]; the
//!   only behavioral difference is markdown stdout is suppressed and
//!   high-severity findings are summarized on stderr (the hook script
//!   uses HOOK_EXIT to decide whether to block).
//!
//! - `pre-push` reads Git's stdin ref tuples and loops:
//!   one Lippa session per non-skipped tuple, each with
//!   `DiffSource::CommitRange { base, head }`. The push's overall
//!   exit code is the maximum severity across all tuples.
//!
//! Stdin parsing per D4/D5 preflight adjudication:
//!   * branch deletion detected via `local_ref == "(delete)"` (literal),
//!     NOT via zero-sha. Skipped with stderr note.
//!   * tag push detected via `local_ref` prefix `refs/tags/`. Skipped
//!     with stderr note (no commit-range semantics for tags).
//!   * new branch detected via `remote_sha` all-zeros. Base resolved
//!     via merge-base against `origin/HEAD`; falls back to tip-only
//!     review with stderr note.
//!   * standard / force-push: `DiffSource::CommitRange { base:
//!     remote_sha, head: local_sha }`. Git's range semantics handle
//!     force-push correctly.

use crate::commands::review::{run as run_review, HookMode, ReviewOptions};
use crate::exit::{CliError, Exit};
use quorum_core::git::DiffSource;
use quorum_lippa_client::keyring::Storage;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

pub const ZERO_SHA: &str = "0000000000000000000000000000000000000000";

/// One Git stdin tuple from a pre-push hook.
///
/// Parsed via [`parse_tuples`] from the line shape
/// `<local-ref> <local-sha> <remote-ref> <remote-sha>`. We do NOT
/// assume `local_ref` is a ref name — Git for Windows 2.53 emits the
/// literal string `(delete)` for branch-deletion tuples (D4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushTuple {
    pub local_ref: String,
    pub local_sha: String,
    pub remote_ref: String,
    pub remote_sha: String,
}

impl PushTuple {
    /// Classify the tuple per the D4/D5 adjudication rules.
    pub fn classify(&self) -> TupleAction {
        if self.local_ref == "(delete)" {
            return TupleAction::SkipDeletion;
        }
        if self.local_ref.starts_with("refs/tags/") {
            return TupleAction::SkipTag;
        }
        if self.remote_sha == ZERO_SHA {
            return TupleAction::NewBranch;
        }
        TupleAction::Range {
            base: self.remote_sha.clone(),
            head: self.local_sha.clone(),
        }
    }
}

/// What [`run_pre_push`] should do with each tuple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TupleAction {
    /// `local_ref == "(delete)"` — branch deletion. Spec §4.5.5 D4.
    SkipDeletion,
    /// `local_ref` starts with `refs/tags/` — D5 addendum.
    SkipTag,
    /// Remote-sha all zeros — new branch push. Base resolved at run
    /// time via merge-base; if unresolvable, the loop reviews the
    /// tip commit only with a stderr note.
    NewBranch,
    /// Standard / force-push: `base..head` revision range.
    Range { base: String, head: String },
}

/// Parse stdin (newline-delimited, four space-separated fields per
/// line) into a vector of [`PushTuple`]s. Empty / blank lines are
/// ignored. Malformed lines (fewer than 4 fields) emit a stderr warning
/// and are skipped — Git's contract is rigid so this should never
/// happen in practice; the defensive skip avoids panicking the hook.
pub fn parse_tuples<R: Read>(reader: R) -> Vec<PushTuple> {
    let mut out = Vec::new();
    let buf = BufReader::new(reader);
    for line in buf.lines().map_while(Result::ok) {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        let fields: Vec<&str> = trimmed.split_whitespace().collect();
        if fields.len() < 4 {
            eprintln!(
                "quorum: skipping malformed pre-push stdin line ({} fields): {trimmed}",
                fields.len()
            );
            continue;
        }
        out.push(PushTuple {
            local_ref: fields[0].to_string(),
            local_sha: fields[1].to_string(),
            remote_ref: fields[2].to_string(),
            remote_sha: fields[3].to_string(),
        });
    }
    out
}

/// Format the timestamp used as the shared filename prefix for all
/// per-tuple archives of one push. Replaces `:` with `-` for Windows
/// compatibility (matches Phase 1A's `archive_filename` rule).
pub fn push_start_iso(now: time::OffsetDateTime) -> String {
    let iso = now
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "unknown".into());
    iso.replace(':', "-")
}

/// `quorum review --hook-mode=pre-commit` entry. Delegates to
/// [`run_review`] with `hook_mode = PreCommit`. The hook script reads
/// the exit code and applies QUORUM_HOOK_POLICY semantics.
pub async fn run_pre_commit(
    repo_start: &Path,
    json: bool,
    no_keyring_storage: Option<Storage>,
) -> Result<Exit, CliError> {
    run_review(
        repo_start,
        ReviewOptions {
            json_to_stdout: json,
            no_keyring_storage,
            diff_source: DiffSource::StagedIndex,
            no_expire: false,
            tui: false,
            hook_mode: HookMode::PreCommit,
            archive_filename_override: None,
        },
    )
    .await
}

/// `quorum review --hook-mode=pre-push` entry. Reads stdin tuples,
/// loops one review per non-skipped tuple, aggregates exit codes.
///
/// The shared push-start ISO is computed once before the loop. Per
/// spec §4.5.5 / §4.10.2 P28 each tuple lands at
/// `.quorum/reviews/<push-start-ISO>.tuple-<N>.json` with `N` 1-indexed.
pub async fn run_pre_push<R: Read>(
    repo_start: &Path,
    stdin: R,
    no_keyring_storage: Option<Storage>,
) -> Result<Exit, CliError> {
    let tuples = parse_tuples(stdin);
    if tuples.is_empty() {
        eprintln!("quorum: pre-push received no ref tuples on stdin; nothing to review");
        return Ok(Exit::Ok);
    }
    let push_iso = push_start_iso(time::OffsetDateTime::now_utc());
    let mut max_exit = Exit::Ok;

    for (i, t) in tuples.iter().enumerate() {
        let tuple_n = i + 1;
        let archive_name = format!("{push_iso}.tuple-{tuple_n}.json");
        let diff_source = match t.classify() {
            TupleAction::SkipDeletion => {
                eprintln!(
                    "quorum: branch deletion detected for {}; skipping review",
                    t.remote_ref
                );
                continue;
            }
            TupleAction::SkipTag => {
                eprintln!(
                    "quorum: tag push detected; skipping review (code review not applicable to refs/tags/*)"
                );
                continue;
            }
            TupleAction::NewBranch => {
                // Spec §4.5.5: resolve base via merge-base against
                // origin/HEAD. If unresolvable, review the tip commit
                // only with a stderr note. We compute the base lazily
                // inside the review pipeline: pass the head as both
                // base and head and let the pipeline handle the
                // empty-range case OR resolve here and pass the
                // computed base. The simpler path is to compute now.
                match resolve_new_branch_base(repo_start, &t.local_sha) {
                    Ok(Some(base)) => DiffSource::CommitRange {
                        base,
                        head: t.local_sha.clone(),
                    },
                    Ok(None) => {
                        eprintln!(
                            "quorum: new-branch push for {}: no upstream HEAD; reviewing the tip commit only",
                            t.remote_ref
                        );
                        DiffSource::CommitRange {
                            base: format!("{}^", t.local_sha),
                            head: t.local_sha.clone(),
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "quorum: new-branch base resolution failed for {}: {e}; skipping",
                            t.remote_ref
                        );
                        continue;
                    }
                }
            }
            TupleAction::Range { base, head } => DiffSource::CommitRange { base, head },
        };

        let exit = run_review(
            repo_start,
            ReviewOptions {
                json_to_stdout: false,
                no_keyring_storage: no_keyring_storage.clone(),
                diff_source,
                no_expire: false,
                tui: false,
                hook_mode: HookMode::PrePush,
                archive_filename_override: Some(archive_name),
            },
        )
        .await?;
        max_exit = max_exit.max(exit);
    }
    Ok(max_exit)
}

/// Attempt to compute the merge-base between `local_sha` and
/// `origin/HEAD`. Returns `Ok(None)` when there's no `origin/HEAD`
/// reference (fresh remote / no default branch tracked) so the caller
/// can fall back to tip-only review.
fn resolve_new_branch_base(repo_start: &Path, local_sha: &str) -> Result<Option<String>, String> {
    let repo = git2::Repository::discover(repo_start).map_err(|e| format!("repo discover: {e}"))?;
    let head_ref = match repo.find_reference("refs/remotes/origin/HEAD") {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };
    let head_oid = head_ref
        .resolve()
        .map_err(|e| format!("resolve origin/HEAD: {e}"))?
        .target()
        .ok_or_else(|| "origin/HEAD has no target oid".to_string())?;
    let local_oid = git2::Oid::from_str(local_sha).map_err(|e| format!("oid parse: {e}"))?;
    let base = repo
        .merge_base(local_oid, head_oid)
        .map_err(|e| format!("merge_base: {e}"))?;
    Ok(Some(base.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tuples_handles_standard_push() {
        let stdin = "refs/heads/main abc123 refs/heads/main 0000def\n";
        let v = parse_tuples(stdin.as_bytes());
        assert_eq!(v.len(), 1);
        let t = &v[0];
        assert_eq!(t.local_ref, "refs/heads/main");
        assert_eq!(t.local_sha, "abc123");
        assert_eq!(t.remote_ref, "refs/heads/main");
        assert_eq!(t.remote_sha, "0000def");
    }

    #[test]
    fn parse_tuples_skips_blank_lines() {
        let stdin = "\n\nrefs/heads/x sha refs/heads/x sha\n\n";
        let v = parse_tuples(stdin.as_bytes());
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn parse_tuples_handles_multi_ref_atomic_push() {
        let stdin = "refs/heads/a sha1 refs/heads/a 0000000000000000000000000000000000000000\n\
                     refs/heads/b sha2 refs/heads/b 0000000000000000000000000000000000000000\n";
        let v = parse_tuples(stdin.as_bytes());
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn parse_tuples_skips_malformed_lines() {
        let stdin = "refs/heads/main abc only-three-fields\nrefs/heads/x sha refs/heads/x sha\n";
        let v = parse_tuples(stdin.as_bytes());
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].local_ref, "refs/heads/x");
    }

    #[test]
    fn classify_deletion_uses_literal_delete_marker() {
        // D4: Git for Windows 2.53 emits literal "(delete)" not zero-sha
        // in local-ref position. Our parser must NOT rely on zero-sha
        // for deletion detection.
        let t = PushTuple {
            local_ref: "(delete)".into(),
            local_sha: ZERO_SHA.into(),
            remote_ref: "refs/heads/feature/x".into(),
            remote_sha: "abc1234".into(),
        };
        assert_eq!(t.classify(), TupleAction::SkipDeletion);
    }

    #[test]
    fn classify_tag_push_is_skipped() {
        // D5: tag pushes have refs/tags/* in local_ref. Skip with note.
        let t = PushTuple {
            local_ref: "refs/tags/v1.0".into(),
            local_sha: "abc1234".into(),
            remote_ref: "refs/tags/v1.0".into(),
            remote_sha: ZERO_SHA.into(),
        };
        assert_eq!(t.classify(), TupleAction::SkipTag);
    }

    #[test]
    fn classify_new_branch_uses_zero_remote_sha() {
        let t = PushTuple {
            local_ref: "refs/heads/feature/x".into(),
            local_sha: "abc1234".into(),
            remote_ref: "refs/heads/feature/x".into(),
            remote_sha: ZERO_SHA.into(),
        };
        assert_eq!(t.classify(), TupleAction::NewBranch);
    }

    #[test]
    fn classify_standard_push_uses_range() {
        let t = PushTuple {
            local_ref: "refs/heads/main".into(),
            local_sha: "new_sha".into(),
            remote_ref: "refs/heads/main".into(),
            remote_sha: "old_sha".into(),
        };
        assert_eq!(
            t.classify(),
            TupleAction::Range {
                base: "old_sha".into(),
                head: "new_sha".into(),
            }
        );
    }

    #[test]
    fn classify_force_push_has_no_special_marker_uses_range() {
        // P3 from preflight: force-push stdin shape is identical to a
        // standard push (no special marker). Git's range semantics
        // surface the rewind in the resulting diff.
        let t = PushTuple {
            local_ref: "refs/heads/main".into(),
            local_sha: "rewound_sha".into(),
            remote_ref: "refs/heads/main".into(),
            remote_sha: "old_tip_sha".into(),
        };
        assert!(matches!(t.classify(), TupleAction::Range { .. }));
    }

    #[test]
    fn push_start_iso_strips_colons() {
        let t = time::OffsetDateTime::from_unix_timestamp(1_715_350_981).unwrap();
        let s = push_start_iso(t);
        assert!(!s.contains(':'));
    }
}
