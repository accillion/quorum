//! `quorum review` — the main pipeline.

use crate::exit::{CliError, Exit};
use crate::render::{render_review_markdown, warn_if_large};
use quorum_core::archive::{archive_filename, build as build_archive, ArchiveInputs};
use quorum_core::bundle::{assemble, BundleInputs, MemoryInput};
use quorum_core::conventions::{load as load_conventions, ConventionsState};
use quorum_core::discovery::discover;
use quorum_core::git::{repo_metadata, staged_diff};
use quorum_core::review::{review_from_json, RepoMetadata};
use quorum_lippa_client::keyring::Storage;
use quorum_lippa_client::{
    keyring, AuthMethod, ClientError, LippaClient, SessionCreateRequest, SessionStatus,
};
use std::path::Path;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct ReviewOptions {
    pub json_to_stdout: bool,
    pub no_keyring_storage: Option<Storage>,
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
    let storage = opts
        .no_keyring_storage
        .clone()
        .unwrap_or(Storage::OsKeyring);
    let cookie = match keyring::load_cookie(&storage, &cfg.base_url) {
        Ok(Some(c)) => c,
        Ok(None) => return Err(CliError::NotAuthenticated),
        Err(e) => return Err(CliError::Keyring(e.to_string())),
    };

    // ===== Git, diff, repo metadata =====
    let (repo, staged) = staged_diff(repo_start).map_err(|e| CliError::Git(e.to_string()))?;
    if staged.is_empty {
        println!("No staged changes — nothing to review");
        return Ok(Exit::Ok);
    }
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

    // ===== Render markdown =====
    let md = render_review_markdown(&review);
    print!("{md}");
    if let Some(note) = warn_if_large(&review) {
        eprintln!("{note}");
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
    };
    let buf = build_archive(&review, &archive_inputs);
    let reviews_dir = workdir.join(".quorum").join("reviews");
    std::fs::create_dir_all(&reviews_dir).map_err(|e| CliError::Io(e.to_string()))?;
    let filename = archive_filename(started_at);
    let archive_path = reviews_dir.join(&filename);
    std::fs::write(&archive_path, &buf).map_err(|e| CliError::Io(e.to_string()))?;
    println!("\n## Archive\nJSON archive written to .quorum/reviews/{filename}");

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
