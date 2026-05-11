//! `quorum install --hook=<type>` and `quorum uninstall --hook=<type>`
//! command handlers (Phase 1B Stage 3).
//!
//! Thin shells over [`crate::hooks`] — map the typed outcomes into
//! stderr messaging + a [`crate::exit::Exit`] value. Mistyped `--hook`
//! values, "outside a git repo", and "existing non-managed hook" are
//! all exit 2 per spec §4.4 / §4.11.

use crate::exit::{CliError, Exit};
use crate::hooks::{self, HookKind, HooksError, InstallOutcome, UninstallOutcome};
use std::path::Path;

/// `quorum install --hook=<type>`. Exit 0 on success, exit 2 on
/// invalid args / outside-repo / non-managed-existing-hook.
pub fn install(repo_start: &Path, hook: &str) -> Result<Exit, CliError> {
    let kind = parse_kind(hook)?;
    match hooks::install(repo_start, kind) {
        Ok(InstallOutcome::Written(p)) => {
            println!("wrote {}", p.display());
            Ok(Exit::Ok)
        }
        Ok(InstallOutcome::Overwritten(p)) => {
            println!("overwrote {} (Quorum-managed)", p.display());
            Ok(Exit::Ok)
        }
        Err(HooksError::NotARepo(p)) => Err(CliError::Config(format!(
            "not inside a git repository: {}",
            p.display()
        ))),
        Err(HooksError::NotManaged(p)) => Err(CliError::Config(format!(
            "existing hook at {} is not Quorum-managed; remove it manually first",
            p.display()
        ))),
        Err(e) => Err(CliError::Io(e.to_string())),
    }
}

/// `quorum uninstall --hook=<type>`. Exit 0 on success (removed or
/// already absent — idempotent); exit 2 on invalid args / outside-repo
/// / non-managed hook.
pub fn uninstall(repo_start: &Path, hook: &str) -> Result<Exit, CliError> {
    let kind = parse_kind(hook)?;
    match hooks::uninstall(repo_start, kind) {
        Ok(UninstallOutcome::Removed(p)) => {
            println!("removed {}", p.display());
            Ok(Exit::Ok)
        }
        Ok(UninstallOutcome::AlreadyAbsent(p)) => {
            println!("hook already absent: {}", p.display());
            Ok(Exit::Ok)
        }
        Err(HooksError::NotARepo(p)) => Err(CliError::Config(format!(
            "not inside a git repository: {}",
            p.display()
        ))),
        Err(HooksError::NotManaged(p)) => Err(CliError::Config(format!(
            "hook at {} is not Quorum-managed; refusing to remove",
            p.display()
        ))),
        Err(e) => Err(CliError::Io(e.to_string())),
    }
}

fn parse_kind(hook: &str) -> Result<HookKind, CliError> {
    HookKind::parse(hook).ok_or_else(|| {
        CliError::Config(format!(
            "unknown --hook value: {hook:?}; expected pre-commit or pre-push"
        ))
    })
}
