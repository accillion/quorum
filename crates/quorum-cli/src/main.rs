//! `quorum` — the Phase 1A CLI.

mod commands;
mod exit;
mod hooks;
mod render;
mod tui;

use clap::{Parser, Subcommand};
use exit::{CliError, Exit};
use quorum_lippa_client::keyring::Storage;
use std::path::PathBuf;
use std::process::ExitCode;

const DEFAULT_BASE_URL: &str = "https://app.lippa.ai";

#[derive(Parser, Debug)]
#[command(
    name = "quorum",
    version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("GIT_SHORT_SHA"), ")"),
    about = "Quorum: multi-model code reviewer"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    Auth(AuthArgs),
    Link(LinkArgs),
    Review(ReviewArgs),
    Install(HookArgs),
    Uninstall(HookArgs),
    /// Phase 1C — inspect dismissal-promotion state. Stage 3 ships the
    /// read surface (`list`, `show`, `history`); `promote` / `demote` /
    /// `prune` land in Stage 4.
    Convention(ConventionArgs),
}

#[derive(clap::Args, Debug)]
struct ConventionArgs {
    /// Operate against `<path>/.quorum/` instead of `$CWD/.quorum/`.
    /// Used by tests for hermetic isolation; production callers omit.
    #[arg(long, value_name = "PATH", global = true)]
    quorum_dir: Option<PathBuf>,
    #[command(subcommand)]
    cmd: ConventionCmd,
}

#[derive(Subcommand, Debug)]
enum ConventionCmd {
    /// List dismissals by promotion state.
    List {
        /// Filter by state: `candidate`, `local_only`, or
        /// `promoted_convention`. Omit for all states.
        #[arg(long, value_name = "STATE")]
        state: Option<String>,
        /// Print orphan reports (managed blocks in conventions.md with no
        /// SQLite row; SQLite rows whose managed block is missing from
        /// conventions.md). Stage 3 read-only diagnostic.
        #[arg(long)]
        orphans: bool,
        /// Emit a JSON array suitable for piping into `jq`.
        #[arg(long)]
        json: bool,
    },
    /// Show one dismissal's title, body, state, and transition log.
    Show {
        /// Hex prefix (≥ 8 chars) or full 64-hex finding_identity_hash.
        hash: String,
    },
    /// Print the `state_transitions` audit log for one dismissal.
    History {
        /// Hex prefix (≥ 8 chars) or full 64-hex finding_identity_hash.
        hash: String,
    },
}

#[derive(clap::Args, Debug)]
struct HookArgs {
    /// Hook kind: `pre-commit` or `pre-push`.
    #[arg(long, value_name = "KIND")]
    hook: String,
}

#[derive(clap::Args, Debug)]
struct AuthArgs {
    #[command(subcommand)]
    cmd: AuthCmd,
}

#[derive(Subcommand, Debug)]
enum AuthCmd {
    Login {
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        no_keyring: bool,
        /// Consume `QUORUM_LIPPA_EMAIL` + `QUORUM_LIPPA_PASSWORD` from
        /// the environment instead of prompting on the TTY. Suitable
        /// for CI bootstrap; cookie persists to keyring (or to the
        /// `--no-keyring` file fallback) per Phase 1A policy.
        #[arg(long)]
        non_interactive: bool,
    },
    Logout {
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        no_keyring: bool,
    },
    Status {
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        no_keyring: bool,
        /// Print the raw session cookie value to stdout. Asks for
        /// confirmation on stderr first; pass `-y` to skip the
        /// prompt. Non-TTY without `-y` exits 2.
        #[arg(long)]
        show_session: bool,
        /// Skip the `--show-session` confirm prompt. No-op without
        /// `--show-session`.
        #[arg(short = 'y', long = "yes")]
        yes: bool,
    },
}

#[derive(clap::Args, Debug)]
struct LinkArgs {
    #[arg(long, conflicts_with_all = ["show"])]
    project: Option<String>,
    #[arg(long)]
    url: Option<String>,
    #[arg(long, default_value_t = false)]
    no_remote_url: bool,
    #[arg(long)]
    show: bool,
}

#[derive(clap::Args, Debug)]
struct ReviewArgs {
    #[arg(long)]
    json: bool,
    #[arg(long)]
    no_keyring: bool,
    /// Review a commit range instead of the staged diff.
    /// Accepts any `git rev-parse`-compatible expression, e.g.
    /// `HEAD~3..HEAD`, `main..feature`, `<base-sha>..<head-sha>`.
    #[arg(long, value_name = "REF-EXPR")]
    range: Option<String>,
    /// Dismissals from this session do not expire (`expires_at = NULL`).
    /// Default expiry is 365 days.
    #[arg(long)]
    no_expire: bool,
    /// Launch the interactive findings/dismiss TUI after the review
    /// converges. Requires a TTY; non-TTY exits 2 before bundle assembly.
    #[arg(long)]
    tui: bool,
    /// Run in hook mode (`pre-commit` or `pre-push`). Suppresses
    /// markdown stdout; emits one-line stderr per high-severity
    /// finding; under `pre-push` reads ref tuples from stdin.
    /// Internally consumed by the shell templates emitted by
    /// `quorum install --hook=<type>`.
    #[arg(long, value_name = "KIND")]
    hook_mode: Option<String>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .compact()
        .init();

    let cli = Cli::parse();
    let res = dispatch(cli).await;
    match res {
        Ok(exit) => exit.code(),
        Err(e) => {
            eprintln!("{e}");
            e.exit().code()
        }
    }
}

async fn dispatch(cli: Cli) -> Result<Exit, CliError> {
    match cli.cmd {
        Cmd::Auth(args) => match args.cmd {
            AuthCmd::Login {
                url,
                no_keyring,
                non_interactive,
            } => {
                let url = url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
                let storage = storage_for(no_keyring)?;
                if non_interactive {
                    // Skips the TTY check entirely (AC 90). Env-var
                    // capture happens inside login_non_interactive,
                    // both values Secret-wrapped at read; the
                    // password is dropped immediately after
                    // login_with_cookie returns (AC 128).
                    commands::auth::login_non_interactive(&url, &storage).await?;
                } else {
                    let tty = is_tty();
                    commands::auth::login(&url, &storage, tty).await?;
                }
                Ok(Exit::Ok)
            }
            AuthCmd::Logout { url, no_keyring } => {
                let url = url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
                let storage = storage_for(no_keyring)?;
                commands::auth::logout(&url, &storage).await?;
                Ok(Exit::Ok)
            }
            AuthCmd::Status {
                url,
                no_keyring,
                show_session,
                yes,
            } => {
                let url = url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
                let storage = storage_for(no_keyring)?;
                if show_session {
                    commands::auth::status_show_session(&url, &storage, is_tty(), yes)?;
                } else {
                    commands::auth::status(&url, &storage).await?;
                }
                Ok(Exit::Ok)
            }
        },
        Cmd::Link(args) => {
            let repo_root = std::env::current_dir().map_err(|e| CliError::Io(e.to_string()))?;
            if args.show {
                commands::link::link_show(&repo_root)?;
                Ok(Exit::Ok)
            } else {
                let project = args
                    .project
                    .ok_or_else(|| CliError::Config("missing --project".into()))?;
                let url = args.url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
                commands::link::link_write(&repo_root, project, url, !args.no_remote_url)?;
                Ok(Exit::Ok)
            }
        }
        Cmd::Review(args) => {
            let repo_root = std::env::current_dir().map_err(|e| CliError::Io(e.to_string()))?;
            let storage = if args.no_keyring {
                Some(fallback_storage()?)
            } else {
                None
            };
            // TTY check happens BEFORE bundle assembly and any network
            // round-trip (v1.0 §4.4 P17, AC 65). Non-TTY `--tui` fails
            // in milliseconds with exit 2.
            if args.tui && !is_tty() {
                return Err(CliError::Config(
                    "--tui requires an interactive terminal; omit --tui for stdout markdown".into(),
                ));
            }
            // Hook-mode routes to dedicated dispatchers — pre-push
            // reads stdin and loops per tuple; pre-commit is a thin
            // shim over the standard review pipeline with output
            // suppressed and stderr findings.
            if let Some(mode) = args.hook_mode {
                return match mode.as_str() {
                    "pre-commit" => {
                        commands::hook_mode::run_pre_commit(&repo_root, args.json, storage).await
                    }
                    "pre-push" => {
                        commands::hook_mode::run_pre_push(&repo_root, std::io::stdin(), storage)
                            .await
                    }
                    other => Err(CliError::Config(format!(
                        "unknown --hook-mode value: {other:?}; expected pre-commit or pre-push"
                    ))),
                };
            }
            let diff_source = match args.range {
                Some(spec) => {
                    let (base, head) = parse_range_spec(&spec)?;
                    quorum_core::git::DiffSource::CommitRange { base, head }
                }
                None => quorum_core::git::DiffSource::StagedIndex,
            };
            commands::review::run(
                &repo_root,
                commands::review::ReviewOptions {
                    json_to_stdout: args.json,
                    no_keyring_storage: storage,
                    diff_source,
                    no_expire: args.no_expire,
                    tui: args.tui,
                    hook_mode: commands::review::HookMode::None,
                    archive_filename_override: None,
                },
            )
            .await
        }
        Cmd::Install(args) => {
            let repo_root = std::env::current_dir().map_err(|e| CliError::Io(e.to_string()))?;
            commands::hooks::install(&repo_root, &args.hook)
        }
        Cmd::Uninstall(args) => {
            let repo_root = std::env::current_dir().map_err(|e| CliError::Io(e.to_string()))?;
            commands::hooks::uninstall(&repo_root, &args.hook)
        }
        Cmd::Convention(args) => dispatch_convention(args),
    }
}

fn dispatch_convention(args: ConventionArgs) -> Result<Exit, CliError> {
    match args.cmd {
        ConventionCmd::List {
            state,
            orphans,
            json,
        } => {
            let state = parse_promotion_state(state.as_deref())?;
            commands::convention::list(args.quorum_dir.as_ref(), state, orphans, json)
        }
        ConventionCmd::Show { hash } => commands::convention::show(args.quorum_dir.as_ref(), &hash),
        ConventionCmd::History { hash } => {
            commands::convention::history(args.quorum_dir.as_ref(), &hash)
        }
    }
}

fn parse_promotion_state(s: Option<&str>) -> Result<Option<quorum_core::PromotionState>, CliError> {
    match s {
        None => Ok(None),
        Some(raw) => quorum_core::PromotionState::from_db_str(raw)
            .map(Some)
            .ok_or_else(|| {
                CliError::Config(format!(
                    "unknown --state value '{raw}'; expected one of \
                     candidate, local_only, promoted_convention"
                ))
            }),
    }
}

/// Parse a `--range` argument. Accepts the canonical `base..head` form;
/// rejects single-ref or `base...head` (symmetric-difference) for now —
/// the bundle pipeline reviews `head` content against `base` tree only.
fn parse_range_spec(spec: &str) -> Result<(String, String), CliError> {
    if let Some((base, head)) = spec.split_once("..") {
        if base.is_empty() || head.is_empty() {
            return Err(CliError::Config(format!(
                "--range expects `<base>..<head>`; got `{spec}`"
            )));
        }
        if head.starts_with('.') {
            // matched `...` — symmetric-difference form
            return Err(CliError::Config(
                "--range does not support `...` (symmetric-difference); use `base..head`".into(),
            ));
        }
        Ok((base.to_string(), head.to_string()))
    } else {
        Err(CliError::Config(format!(
            "--range expects `<base>..<head>`; got `{spec}`"
        )))
    }
}

fn storage_for(no_keyring: bool) -> Result<Storage, CliError> {
    if no_keyring {
        Ok(fallback_storage()?)
    } else {
        Ok(Storage::OsKeyring)
    }
}

fn fallback_storage() -> Result<Storage, CliError> {
    let base = config_dir().ok_or_else(|| CliError::Io("could not resolve config dir".into()))?;
    let dir = base.join("quorum").join("sessions");
    Ok(Storage::File(dir))
}

fn config_dir() -> Option<PathBuf> {
    // Windows: %APPDATA%\quorum\sessions
    // Unix: $XDG_CONFIG_HOME or ~/.config
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return Some(PathBuf::from(appdata));
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg));
    }
    if let Some(home) = std::env::var_os("HOME") {
        return Some(PathBuf::from(home).join(".config"));
    }
    None
}

fn is_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}
