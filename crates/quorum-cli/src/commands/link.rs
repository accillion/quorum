//! `quorum link` — bind the current repo to a Lippa project.

use crate::exit::CliError;
use quorum_core::config::{read, write, QuorumConfig};
use std::path::Path;

pub fn link_show(repo_root: &Path) -> Result<(), CliError> {
    match read(repo_root) {
        Ok(cfg) => {
            println!("project_id = {}", cfg.project_id);
            println!("base_url   = {}", cfg.base_url);
            println!("remote_url = {}", cfg.remote_url);
            Ok(())
        }
        Err(quorum_core::config::ConfigError::NotFound(_)) => Err(CliError::NotLinked),
        Err(e) => Err(CliError::Config(e.to_string())),
    }
}

pub fn link_write(
    repo_root: &Path,
    project_id: String,
    base_url: String,
    remote_url: bool,
) -> Result<(), CliError> {
    let cfg = QuorumConfig {
        project_id,
        base_url,
        remote_url,
    };
    let p = write(repo_root, &cfg).map_err(|e| CliError::Config(e.to_string()))?;
    println!("Wrote {}", p.display());
    Ok(())
}
