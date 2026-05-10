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
            commands::review::run(
                &repo_root,
                commands::review::ReviewOptions {
                    json_to_stdout: args.json,
                    no_keyring_storage: storage,
                },
            )
            .await
        }
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
