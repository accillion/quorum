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
//!   `DiffSource::CommitRange { base, head }`. A tuple that errors is
//!   warned about and the loop goes on. The push's overall exit code
//!   comes from [`aggregate_push_exits`]: a blocking verdict (exit 1)
//!   always wins, which is not the numeric maximum.
//!
//! Stdin parsing per D4/D5 preflight adjudication:
//!   * branch deletion detected via `local_ref == "(delete)"` (literal),
//!     NOT via zero-sha. Skipped with stderr note.
//!   * tag push detected via `local_ref` prefix `refs/tags/`. Skipped
//!     with stderr note (no commit-range semantics for tags).
//!   * new branch detected via `remote_sha` all-zeros. Reviewed from the
//!     merge-base with a verified tracking ref of the destination remote,
//!     else as its full tree, never tip-only (v0.6 spec AC 277a step 2;
//!     see [`super::push_coverage`]).
//!   * standard / force-push: `DiffSource::CommitRange { base:
//!     remote_sha, head: local_sha }`. Git's range semantics handle
//!     force-push correctly.

use crate::commands::push_coverage::{plan_new_branch, NewBranchPlan};
use crate::commands::review::{run as run_review, HookMode, ReviewDiff, ReviewOptions};
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
    /// Remote-sha all zeros — new branch push. Planned at run time by
    /// [`super::push_coverage::plan_new_branch`]: a verified merge-base,
    /// else the full tree; never the tip commit only.
    NewBranch,
    /// Standard / force-push: `base..head` revision range.
    Range { base: String, head: String },
}

/// Where a push is going, from the `$1` / `$2` Git passes the pre-push
/// hook (v0.6 spec AC 277a). The v4 template forwards them as `--remote`
/// and `--url`. The URL itself is never kept: it may carry credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushDestination {
    /// A configured remote, by name.
    Named(String),
    /// `$1` equals `$2`: the push named a URL, not a remote. A URL push
    /// has no tracking refs.
    Url,
    /// No remote argument: a v1–v3 hook, which never passed one. Quorum
    /// never guesses `origin` for it.
    NotPassed,
}

impl PushDestination {
    pub fn from_args(remote: Option<String>, url: Option<String>) -> Self {
        match remote {
            None => PushDestination::NotPassed,
            Some(r) if r.is_empty() => PushDestination::NotPassed,
            Some(r) if url.as_deref() == Some(r.as_str()) => PushDestination::Url,
            Some(r) => PushDestination::Named(r),
        }
    }
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
            diff_source: ReviewDiff::Source(DiffSource::StagedIndex),
            no_expire: false,
            tui: false,
            hook_mode: HookMode::PreCommit,
            archive_filename_override: None,
            tier_override: None,
        },
    )
    .await
}

/// AC 277a step 1: what to do with a ref the remote already has
/// (`remote_sha` not all-zero), before any Lippa session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExistingRef {
    /// Review the exact tree delta, `remote_sha` → `local_sha`.
    Review,
    /// Both commits have the same tree: the remote gains nothing.
    TreeUnchanged,
    /// `remote_sha` is not in the local object store, so the delta
    /// cannot be computed.
    RemoteMissing,
}

fn check_existing_ref(repo_start: &Path, remote_sha: &str, local_sha: &str) -> ExistingRef {
    let Ok(repo) = git2::Repository::discover(repo_start) else {
        // The review pipeline reports the missing repository itself.
        return ExistingRef::Review;
    };
    let tree = |sha: &str| {
        git2::Oid::from_str(sha)
            .ok()
            .and_then(|oid| repo.find_commit(oid).ok())
            .map(|c| c.tree_id())
    };
    match (tree(remote_sha), tree(local_sha)) {
        (None, _) => ExistingRef::RemoteMissing,
        (Some(remote), Some(local)) if remote == local => ExistingRef::TreeUnchanged,
        _ => ExistingRef::Review,
    }
}

/// The remote to name in a `git fetch <remote>` hint. A URL is never
/// printed: it may carry credentials.
fn fetch_hint(dest: &PushDestination) -> &str {
    match dest {
        PushDestination::Named(name) => name,
        PushDestination::Url | PushDestination::NotPassed => "<remote>",
    }
}

/// The first 12 hex digits of a sha, for messages.
fn short_sha(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

/// `quorum review --hook-mode=pre-push` entry. Reads stdin tuples,
/// loops one review per non-skipped tuple, aggregates exit codes with
/// [`aggregate_push_exits`].
///
/// A tuple whose review errors prints a one-line stderr warning naming
/// its ref and counts as that error's exit (`AuthRequired` for an auth
/// error, `ToolingError` otherwise). The loop always reviews the rest.
///
/// The shared push-start ISO is computed once before the loop. Per
/// spec §4.5.5 / §4.10.2 P28 each tuple lands at
/// `.quorum/reviews/<push-start-ISO>.tuple-<N>.json` with `N` 1-indexed.
pub async fn run_pre_push<R: Read>(
    repo_start: &Path,
    stdin: R,
    dest: &PushDestination,
    no_keyring_storage: Option<Storage>,
) -> Result<Exit, CliError> {
    let tuples = parse_tuples(stdin);
    if tuples.is_empty() {
        eprintln!("quorum: pre-push received no ref tuples on stdin; nothing to review");
        return Ok(Exit::Ok);
    }
    let push_iso = push_start_iso(time::OffsetDateTime::now_utc());
    let mut exits = Vec::with_capacity(tuples.len());

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
                // AC 277a step 2: review what the destination remote
                // lacks, never the tip commit only.
                match plan_new_branch(repo_start, &t.local_sha, dest) {
                    Ok(NewBranchPlan::MergeBase { base, tracking_ref }) => {
                        eprintln!(
                            "quorum: new branch {}: reviewing from its merge-base with {tracking_ref}",
                            t.remote_ref
                        );
                        DiffSource::CommitRange {
                            base,
                            head: t.local_sha.clone(),
                        }
                    }
                    Ok(NewBranchPlan::AlreadyOnRemote { tracking_ref }) => {
                        eprintln!(
                            "quorum: new branch {}: {tracking_ref} already has all of it; nothing to review",
                            t.remote_ref
                        );
                        exits.push(Exit::Ok);
                        continue;
                    }
                    Ok(NewBranchPlan::FullTree { reason }) => {
                        eprintln!(
                            "quorum: new branch {}: reviewing its full tree, because {reason}",
                            t.remote_ref
                        );
                        let full_tree = ReviewDiff::NewBranchFullTree {
                            head: t.local_sha.clone(),
                        };
                        exits.push(
                            review_tuple(
                                repo_start,
                                t,
                                full_tree,
                                archive_name,
                                &no_keyring_storage,
                            )
                            .await,
                        );
                        continue;
                    }
                    Err(e) => {
                        // Not a silent skip: the tuple was not reviewed,
                        // so it counts as a tooling error. Fail-closed
                        // blocks on it; a blocking verdict elsewhere
                        // still wins in `aggregate_push_exits`.
                        eprintln!(
                            "quorum: new-branch base resolution failed for {}: {e}; not reviewed (exit 2)",
                            t.remote_ref
                        );
                        exits.push(Exit::ToolingError);
                        continue;
                    }
                }
            }
            TupleAction::Range { base, head } => match check_existing_ref(repo_start, &base, &head)
            {
                ExistingRef::Review => DiffSource::CommitRange { base, head },
                ExistingRef::TreeUnchanged => {
                    eprintln!(
                        "quorum: {}: tree unchanged from the remote's {}; nothing to review",
                        t.remote_ref,
                        short_sha(&base)
                    );
                    exits.push(Exit::Ok);
                    continue;
                }
                ExistingRef::RemoteMissing => {
                    eprintln!(
                        "quorum: {}: the remote's commit {} is not in this repository; \
                         run `git fetch {}` and push again; not reviewed (exit 2)",
                        t.remote_ref,
                        short_sha(&base),
                        fetch_hint(dest)
                    );
                    exits.push(Exit::ToolingError);
                    continue;
                }
            },
        };

        let diff = ReviewDiff::Source(diff_source);
        exits.push(review_tuple(repo_start, t, diff, archive_name, &no_keyring_storage).await);
    }
    Ok(aggregate_push_exits(exits))
}

/// Review one tuple's `diff`. An error prints one stderr line naming the
/// ref and becomes that error's exit (`AuthRequired` for an auth error,
/// `ToolingError` otherwise).
async fn review_tuple(
    repo_start: &Path,
    t: &PushTuple,
    diff: ReviewDiff,
    archive_name: String,
    no_keyring_storage: &Option<Storage>,
) -> Exit {
    match run_review(
        repo_start,
        ReviewOptions {
            json_to_stdout: false,
            no_keyring_storage: no_keyring_storage.clone(),
            diff_source: diff,
            no_expire: false,
            tui: false,
            hook_mode: HookMode::PrePush,
            archive_filename_override: Some(archive_name),
            tier_override: None,
        },
    )
    .await
    {
        Ok(exit) => exit,
        Err(e) => {
            let exit = e.exit();
            let why = e.to_string().lines().collect::<Vec<_>>().join(" ");
            eprintln!(
                "quorum: pre-push review of {} failed (exit {}): {why}",
                t.local_ref, exit as u8
            );
            exit
        }
    }
}

/// Combine the per-tuple exits of one push into the hook's exit.
///
/// `HighSeverity` if any tuple returned it. Otherwise the highest of the
/// rest, in the order `AuthRequired` > `ToolingError` > `Ok`. This is
/// not the numeric maximum ([`Exit::max`]) and deliberately departs from
/// spec §4.5.5's literal "max severity": the fail-open template blocks
/// only on exit 1, so a high-severity tuple next to a transient 502 must
/// still block the push. SERVICES.md records the rule.
pub fn aggregate_push_exits(exits: impl IntoIterator<Item = Exit>) -> Exit {
    let mut rest = Exit::Ok;
    for exit in exits {
        if exit == Exit::HighSeverity {
            return Exit::HighSeverity;
        }
        rest = rest.max(exit);
    }
    rest
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

    /// A blocking verdict wins in any position; the rest rank
    /// `AuthRequired` > `ToolingError` > `Ok`.
    #[test]
    fn aggregate_push_exits_keeps_a_blocking_verdict_on_top() {
        use Exit::*;
        for (exits, want) in [
            (vec![], Ok),
            (vec![Ok, Ok], Ok),
            (vec![HighSeverity, ToolingError], HighSeverity),
            (vec![ToolingError, HighSeverity], HighSeverity),
            (vec![AuthRequired, HighSeverity, ToolingError], HighSeverity),
            (vec![ToolingError, Ok], ToolingError),
            (vec![Ok, ToolingError], ToolingError),
            (vec![AuthRequired, ToolingError], AuthRequired),
            (vec![ToolingError, AuthRequired], AuthRequired),
        ] {
            assert_eq!(aggregate_push_exits(exits.clone()), want, "{exits:?}");
        }
    }

    /// AC 277a: `$1` equal to `$2` is a URL push; a missing or empty
    /// `$1` is an old hook. Nothing defaults to `origin`.
    #[test]
    fn push_destination_comes_from_the_hook_arguments() {
        let s = |v: &str| Some(v.to_string());
        for (remote, url, want) in [
            (None, None, PushDestination::NotPassed),
            (s(""), s(""), PushDestination::NotPassed),
            (
                s("upstream"),
                s("https://h/r.git"),
                PushDestination::Named("upstream".into()),
            ),
            (s("a/b"), None, PushDestination::Named("a/b".into())),
            (
                s("https://h/r.git"),
                s("https://h/r.git"),
                PushDestination::Url,
            ),
            (s("../r.git"), s("../r.git"), PushDestination::Url),
        ] {
            assert_eq!(
                PushDestination::from_args(remote.clone(), url.clone()),
                want,
                "{remote:?} {url:?}"
            );
        }
    }
}
