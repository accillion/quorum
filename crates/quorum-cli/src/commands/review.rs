//! `quorum review` — the main pipeline.

use crate::exit::{CliError, Exit};
use crate::render::{render_review_markdown_with_dismissed, warn_if_large};
use quorum_core::archive::{
    archive_filename, build as build_archive, ArchiveInputs, SuppressionSummary,
};
use quorum_core::bundle::{assemble, BundleInputs, MemoryInput};
use quorum_core::conventions::{load as load_conventions, ConventionsState};
use quorum_core::discovery::discover;
use quorum_core::git::{diff_for_source, repo_metadata, DiffSource};
use quorum_core::memory::{
    finding_identity_hash, FindingIdentityHash, LocalSqliteMemoryStore, MemoryStore,
};
use quorum_core::review::{review_from_json, Finding, RepoMetadata};
use quorum_lippa_client::keyring::Storage;
use quorum_lippa_client::{
    keyring, AuthMethod, ClientError, LippaClient, SessionCreateRequest, SessionStatus,
};
use std::path::Path;
use std::time::Instant;

/// What kind of invocation the review pipeline is serving. Drives
/// output framing (markdown vs stderr finding lines), the no-auth
/// fail-open branch, and skips the markdown stdout dump under hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookMode {
    /// Direct user invocation: `quorum review` (with or without
    /// `--tui` / `--range`). Full markdown output, errors propagate.
    None,
    /// `quorum review --hook-mode=pre-commit`. No markdown stdout;
    /// one-line stderr per high-severity finding; no-auth skips with
    /// exit 0 + stderr note so the shell template's fail-open
    /// semantics apply.
    PreCommit,
    /// `quorum review --hook-mode=pre-push`. Same hook framing as
    /// pre-commit; one pre-push invocation may call into the review
    /// pipeline N times (once per stdin tuple) — the outer
    /// `commands::hook_mode::run_pre_push` handles the loop. Each
    /// tuple writes its own archive at the
    /// `<push-start-ISO>.tuple-<N>.json` filename supplied via
    /// `archive_filename_override`.
    PrePush,
}

#[derive(Debug, Clone)]
pub struct ReviewOptions {
    pub json_to_stdout: bool,
    pub no_keyring_storage: Option<Storage>,
    /// Phase 1B: `StagedIndex` (default) or `CommitRange { base, head }`.
    pub diff_source: DiffSource,
    /// Phase 1B: when true, dismissals from this session are permanent
    /// (`expires_at = NULL`). Default expiry is 365 days; this flag is
    /// surfaced via `quorum review --no-expire`.
    pub no_expire: bool,
    /// Phase 1B Stage 2: enter the interactive TUI after the review
    /// converges and the filter site runs. TTY check happens upstream
    /// of bundle assembly (`main.rs`) so this flag is only ever true
    /// when stdout is a real terminal.
    pub tui: bool,
    /// Phase 1B Stage 3: pre-commit / pre-push framing.
    pub hook_mode: HookMode,
    /// Phase 1B Stage 3: override the archive filename for this run
    /// (used by `pre-push` to write
    /// `<push-start-ISO>.tuple-<N>.json`). When `None`, the Phase 1A
    /// `<started_at>.json` naming is used.
    pub archive_filename_override: Option<String>,
}

pub async fn run(repo_start: &Path, opts: ReviewOptions) -> Result<Exit, CliError> {
    let started_at = time::OffsetDateTime::now_utc();
    let start_clock = Instant::now();

    // ===== Config =====
    let cfg = match quorum_core::config::read(repo_start) {
        Ok(c) => c,
        Err(quorum_core::config::ConfigError::NotFound(_)) => return Err(CliError::NotLinked),
        Err(e) => return Err(CliError::Config(e.to_string())),
    };

    // ===== Storage / cookie =====
    //
    // Resolution order per spec §4.4 / §4.6.1 (P11):
    //   1. QUORUM_LIPPA_SESSION env var — direct cookie passthrough,
    //      no keyring read. Cookie wrapped in Secret at env read.
    //   2. Keyring entry for host.
    //   3. --no-keyring file fallback (still under `storage`).
    //
    // When BOTH the env var AND a keyring entry exist, the env var
    // wins with a stderr info note — emitted ONLY in interactive
    // `quorum review` invocations. Suppressed under `--hook-mode=*`
    // (and any future `--non-interactive`) so CI output stays clean.
    // Spec P31.
    //
    // Under hook modes (pre-commit / pre-push) the absence of auth is
    // NOT a hard error — we exit 0 with a single stderr "not
    // authenticated" line so the shell template's fail-open path
    // proceeds. Spec §4.4 / AC 87.
    let storage = opts
        .no_keyring_storage
        .clone()
        .unwrap_or(Storage::OsKeyring);
    let env_session = std::env::var("QUORUM_LIPPA_SESSION")
        .ok()
        .filter(|s| !s.is_empty())
        .map(quorum_lippa_client::Secret::new);
    let cookie = if let Some(env_secret) = env_session {
        // Probe keyring strictly to decide whether to emit the
        // precedence note; the env value wins regardless.
        if opts.hook_mode == HookMode::None {
            if let Ok(Some(_keyring_cookie)) = keyring::load_cookie(&storage, &cfg.base_url) {
                eprintln!(
                    "quorum: using QUORUM_LIPPA_SESSION env var (a keyring entry for {} also exists; env var takes precedence)",
                    cfg.base_url
                );
            }
        }
        env_secret
    } else {
        match keyring::load_cookie(&storage, &cfg.base_url) {
            Ok(Some(c)) => c,
            Ok(None) if opts.hook_mode != HookMode::None => {
                eprintln!(
                    "quorum: not authenticated — hook reviews are being skipped; run quorum auth login"
                );
                return Ok(Exit::Ok);
            }
            Ok(None) => return Err(CliError::NotAuthenticated),
            Err(e) => return Err(CliError::Keyring(e.to_string())),
        }
    };

    // ===== Git, diff, repo metadata =====
    let (repo, staged) =
        diff_for_source(repo_start, &opts.diff_source).map_err(|e| CliError::Git(e.to_string()))?;
    if staged.is_empty {
        let label = match &opts.diff_source {
            DiffSource::StagedIndex => "staged",
            DiffSource::CommitRange { .. } => "commit-range",
        };
        println!("No {label} changes — nothing to review");
        return Ok(Exit::Ok);
    }
    // `no_expire` flag is consumed by the TUI/dismiss path (Stage 2); the
    // CLI surface accepts it now (AC 60) so existing call sites work.
    let _no_expire = opts.no_expire;
    let facts = repo_metadata(&repo).map_err(|e| CliError::Git(e.to_string()))?;
    let workdir = repo
        .workdir()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| repo_start.to_path_buf());

    // ===== Memory + conventions =====
    let disc = discover(&workdir);
    if let Some(chosen) = &disc.chosen {
        eprintln!("using repo memory: {chosen}");
        if !disc.ignored.is_empty() {
            eprintln!("ignored: {}", disc.ignored.join(", "));
        }
    }
    let memory = match &disc.chosen_path {
        Some(p) => match std::fs::read_to_string(p) {
            Ok(content) => Some(MemoryInput {
                source_basename: disc
                    .chosen
                    .clone()
                    .unwrap_or_else(|| p.display().to_string()),
                content,
            }),
            Err(_) => None,
        },
        None => None,
    };

    let conventions = load_conventions(&workdir).map_err(|e| CliError::Git(e.to_string()))?;
    if matches!(conventions, ConventionsState::PresentButIgnored) {
        eprintln!("note: .quorum/conventions.md present but not committed (or has uncommitted changes); ignored.");
    }

    // ===== Bundle assembly =====
    let bundle_inputs = BundleInputs {
        staged: &staged,
        memory,
        conventions: &conventions,
        discovery: &disc,
        branch: &facts.branch,
        head_sha: &facts.head_sha,
        remote_url: if cfg.remote_url {
            facts.remote_url.as_deref()
        } else {
            None
        },
    };
    let bundle = match assemble(&bundle_inputs) {
        Ok(b) => b,
        Err(quorum_core::bundle::BundleError::BundleTooLarge(n)) => {
            return Err(CliError::BundleTooLarge(n / 1024));
        }
    };
    for (path, _reason) in &bundle.exclusions {
        eprintln!("excluded: {path}");
    }
    if bundle.diff_truncated {
        eprintln!("note: diff truncated to fit budget");
    }
    if !bundle.files_omitted.is_empty() {
        eprintln!(
            "note: file contents omitted: {}",
            bundle.files_omitted.join(", ")
        );
    }

    // ===== Create / poll / fetch =====
    let client = LippaClient::new(cfg.base_url.clone(), AuthMethod::Cookie(cookie))
        .map_err(|e| CliError::Network(e.to_string()))?;
    let session_id = match client
        .create_session(SessionCreateRequest {
            prompt: bundle.prompt,
            project_id: Some(cfg.project_id.clone()),
            debate_mode: "standard".into(),
            idempotency_key: None,
        })
        .await
    {
        Ok(s) => s,
        Err(ClientError::Auth(quorum_lippa_client::AuthError::LoginRequired(_))) => {
            return Err(CliError::CreateUnauthorized);
        }
        Err(ClientError::Transport(m)) => return Err(CliError::Network(m)),
        Err(ClientError::HttpStatus(s, b)) => {
            return Err(CliError::Network(format!("{s}: {}", truncate(&b, 240))));
        }
        Err(e) => return Err(CliError::Network(e.to_string())),
    };

    // Polling loop.
    let poll_opts = quorum_lippa_client::client::PollOptions::default();
    let mut base = poll_opts.initial_delay;
    let mut last_status_label = "pending".to_string();
    let deadline = Instant::now() + poll_opts.overall_timeout;
    let session_status = loop {
        let elapsed_left = deadline.saturating_duration_since(Instant::now());
        if elapsed_left.is_zero() {
            return Err(CliError::PollTimeout(last_status_label));
        }
        let (status, hints) = match client.poll_status(&session_id).await {
            Ok(t) => t,
            Err(ClientError::Auth(_)) => return Err(CliError::PollUnauthorized),
            Err(ClientError::Transport(m)) => return Err(CliError::Network(m)),
            Err(e) => return Err(CliError::Network(e.to_string())),
        };
        match &status {
            SessionStatus::InProgress => {}
            SessionStatus::Converged => break status,
            SessionStatus::Failed(s) => {
                return Err(CliError::ConsensusTerminal {
                    state: "failed".into(),
                    detail: s.clone(),
                });
            }
            SessionStatus::Cancelled => {
                return Err(CliError::ConsensusTerminal {
                    state: "cancelled".into(),
                    detail: String::new(),
                });
            }
            SessionStatus::Error(s) => {
                return Err(CliError::ConsensusTerminal {
                    state: "error".into(),
                    detail: s.clone(),
                });
            }
            SessionStatus::Unauthorized => return Err(CliError::PollUnauthorized),
        }
        last_status_label = match &status {
            SessionStatus::InProgress => "running".into(),
            other => format!("{other:?}"),
        };
        let next = quorum_lippa_client::client::next_poll_delay(
            base,
            poll_opts.max_delay,
            hints.retry_after,
            rand::random::<f32>(),
        );
        tokio::time::sleep(next).await;
        base = (base * 2).min(poll_opts.max_delay);
    };

    let _ = session_status;
    let detail = match client.fetch_detail(&session_id).await {
        Ok(v) => v,
        Err(ClientError::Auth(_)) => return Err(CliError::PollUnauthorized),
        Err(ClientError::Transport(m)) => return Err(CliError::Network(m)),
        Err(e) => return Err(CliError::Network(e.to_string())),
    };

    let repo_meta = RepoMetadata {
        remote_url: if cfg.remote_url {
            facts.remote_url.clone()
        } else {
            None
        },
        branch: facts.branch.clone(),
        head_sha: facts.head_sha.clone(),
        project_id: Some(cfg.project_id.clone()),
        base_url: cfg.base_url.clone(),
    };
    let mut review =
        review_from_json(&detail, &repo_meta).map_err(|e| CliError::Malformed(e.to_string()))?;
    review.elapsed = start_clock.elapsed();

    // ===== Dismissals filter site (Phase 1B Stage 1) =====
    //
    // Open the local SQLite store, fetch active (non-expired) dismissals
    // keyed by identity hash, drop matching findings from the rendered
    // surface, and bump `record_seen` for the matched hashes in one
    // transaction. A failure to open the store warns and proceeds
    // without filtering so we never block reviews on a local DB issue.
    let store = match LocalSqliteMemoryStore::new(&workdir) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("warning: dismissals store unavailable; proceeding without filter: {e}");
            None
        }
    };
    let review_session_id = review.session_id.clone();
    let (mut suppressed_summaries, mut dismissals_applied) = if let Some(store) = &store {
        match store.load_active_dismissals() {
            Ok(active) => apply_dismissals_filter(
                &mut review,
                &active,
                store,
                &review_session_id,
                cfg.memory.candidate_threshold,
                opts.hook_mode,
            ),
            Err(e) => {
                eprintln!("warning: load_active_dismissals failed: {e}");
                (Vec::new(), 0u32)
            }
        }
    } else {
        (Vec::new(), 0u32)
    };

    // ===== TUI (Phase 1B Stage 2) =====
    //
    // Empty-filtered shortcut: if no findings remain after the filter
    // site, the TUI is *not* entered. Print a one-line summary, write
    // archive, exit 0 — same path the normal exit takes
    // (spec §4.3.1 empty-state).
    if opts.tui {
        if review.findings.is_empty() {
            println!(
                "No visible findings ({} dismissed).",
                suppressed_summaries.len()
            );
        } else if let Some(store) = &store {
            match crate::tui::run(
                &review,
                store,
                &facts.head_sha,
                &facts.branch,
                opts.no_expire,
            ) {
                Ok(outcome) => {
                    review.findings = outcome.kept_findings;
                    let new_count = outcome.newly_suppressed.len() as u32;
                    suppressed_summaries.extend(outcome.newly_suppressed);
                    dismissals_applied += new_count;
                }
                Err(e) => {
                    eprintln!("warning: TUI error: {e}");
                }
            }
        } else {
            eprintln!(
                "warning: TUI requested but dismissals store unavailable; falling back to markdown."
            );
        }
    }

    // ===== Render markdown =====
    //
    // Under --tui OR hook mode we suppress the markdown stdout dump
    // (the TUI is the user-facing surface; the hook script consumes
    // exit codes, not stdout). The archive is written either way.
    if !opts.tui && opts.hook_mode == HookMode::None {
        let md = render_review_markdown_with_dismissed(&review, dismissals_applied);
        print!("{md}");
        if let Some(note) = warn_if_large(&review) {
            eprintln!("{note}");
        }
    } else if opts.hook_mode != HookMode::None {
        // Spec §4.5.2: one-line stderr per remaining high-severity
        // finding (post-filter) plus a footer pointing the user to
        // the interactive surface. Pre-push uses the same framing.
        let highs: Vec<&quorum_core::Finding> = review
            .findings
            .iter()
            .filter(|f| f.severity == quorum_core::Severity::High)
            .collect();
        if !highs.is_empty() {
            let head = match opts.hook_mode {
                HookMode::PreCommit => "quorum review (pre-commit)",
                HookMode::PrePush => "quorum review (pre-push)",
                HookMode::None => unreachable!(),
            };
            eprintln!("{head} — {} high-severity finding(s):", highs.len());
            for f in &highs {
                eprintln!("  [H] {}", f.title);
            }
            eprintln!("       run `quorum review --tui` to dismiss interactively.");
        }
    }

    // ===== Archive =====
    let archive_inputs = ArchiveInputs {
        started_at,
        elapsed: review.elapsed,
        session_id: review.session_id.clone(),
        project_id: review.project_id.clone(),
        base_url: review.base_url.clone(),
        remote_url: if cfg.remote_url {
            facts.remote_url.clone()
        } else {
            None
        },
        branch: facts.branch.clone(),
        head_sha: facts.head_sha.clone(),
        dismissals_applied,
        suppressed_findings: suppressed_summaries,
    };
    let buf = build_archive(&review, &archive_inputs);
    let reviews_dir = workdir.join(".quorum").join("reviews");
    std::fs::create_dir_all(&reviews_dir).map_err(|e| CliError::Io(e.to_string()))?;
    let filename = opts
        .archive_filename_override
        .clone()
        .unwrap_or_else(|| archive_filename(started_at));
    let archive_path = reviews_dir.join(&filename);
    std::fs::write(&archive_path, &buf).map_err(|e| CliError::Io(e.to_string()))?;
    if opts.hook_mode == HookMode::None && !opts.tui {
        println!("\n## Archive\nJSON archive written to .quorum/reviews/{filename}");
    }

    if opts.json_to_stdout {
        // Same buffer, byte-identical (AC 26).
        std::io::Write::write_all(&mut std::io::stdout(), &buf).ok();
    }

    // Persist renewed cookie if changed.
    if let Some(new) = client.renewed_cookie() {
        if let Err(e) = keyring::store_cookie(&storage, &cfg.base_url, &new) {
            eprintln!("note: cookie renewal save failed: {e}");
        }
    }

    if review.has_high_severity() {
        Ok(Exit::HighSeverity)
    } else {
        Ok(Exit::Ok)
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

/// Drop findings whose identity hash matches an active dismissal; emit a
/// [`SuppressionSummary`] for each matched row; call `record_seen` once
/// for all matches in one transaction (idempotent per session id).
///
/// Returns `(suppressed_summaries, dismissals_applied_count)`. The count
/// matches `suppressed_summaries.len()` cast to `u32`.
fn apply_dismissals_filter(
    review: &mut quorum_core::Review,
    active: &std::collections::HashMap<FindingIdentityHash, quorum_core::Dismissal>,
    store: &LocalSqliteMemoryStore,
    review_session_id: &str,
    candidate_threshold: u32,
    hook_mode: HookMode,
) -> (Vec<SuppressionSummary>, u32) {
    let mut summaries: Vec<SuppressionSummary> = Vec::new();
    let mut matched_hashes: Vec<FindingIdentityHash> = Vec::new();
    let original = std::mem::take(&mut review.findings);
    let mut kept: Vec<Finding> = Vec::with_capacity(original.len());
    for f in original {
        let h = finding_identity_hash(&f);
        if let Some(d) = active.get(&h) {
            matched_hashes.push(h);
            summaries.push(SuppressionSummary {
                finding_identity_hash: d.finding_identity_hash.to_hex(),
                title_snapshot: d.title_snapshot.clone(),
                source_type_snapshot: d.source_type_snapshot.clone(),
                reason: d.reason.as_db_str().to_string(),
                dismissed_at: d
                    .dismissed_at
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
            });
        } else {
            kept.push(f);
        }
    }
    review.findings = kept;

    if !matched_hashes.is_empty() {
        let now = time::OffsetDateTime::now_utc();
        match store.record_seen(&matched_hashes, review_session_id, now, candidate_threshold) {
            Ok(transitions) => {
                // Phase 1C §3.2 T1 / §5.3 / Q14 lean: emit one stderr
                // line per fired auto-promote transition. Suppress under
                // any hook mode (parity with the Phase 1B
                // QUORUM_LIPPA_SESSION precedence note); the transition
                // itself still fires — only the user-visible note is
                // gated. Stage 2 picks up the local_only entry on the
                // next bundle assembly.
                if hook_mode == HookMode::None {
                    for ev in &transitions {
                        eprintln!(
                            "quorum: dismissal {} auto-promoted to local convention (recurrence={})",
                            ev.short_hash, ev.recurrence_at_transition,
                        );
                    }
                }
            }
            Err(e) => eprintln!("warning: record_seen failed: {e}"),
        }
    }

    let n = summaries.len() as u32;
    (summaries, n)
}
