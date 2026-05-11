//! `quorum` — the Phase 1A CLI.

mod commands;
mod exit;
mod render;

use clap::{Parser, Subcommand};
use exit::{CliError, Exit};
use quorum_lippa_client::keyring::Storage;
use std::path::PathBuf;
use std::process::ExitCode;

const DEFAULT_BASE_URL: &str = "https://app.lippa.ai";

#[derive(Parser, Debug)]
#[command(
    name = "quorum",
    version,
    about = "Quorum: multi-model code reviewer (Phase 1A)"
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
            AuthCmd::Login { url, no_keyring } => {
                let url = url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
                let storage = storage_for(no_keyring)?;
                let tty = is_tty();
                commands::auth::login(&url, &storage, tty).await?;
                Ok(Exit::Ok)
            }
            AuthCmd::Logout { url, no_keyring } => {
                let url = url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
                let storage = storage_for(no_keyring)?;
                commands::auth::logout(&url, &storage).await?;
                Ok(Exit::Ok)
            }
            AuthCmd::Status { url, no_keyring } => {
                let url = url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
                let storage = storage_for(no_keyring)?;
                commands::auth::status(&url, &storage).await?;
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
                },
            )
            .await
        }
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
