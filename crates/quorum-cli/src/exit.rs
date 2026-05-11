//! Exit-code taxonomy. Stable contract from Phase 1A onward.
//!
//! 0  ok / no high-severity findings.
//! 1  review completed, one or more high-severity findings.
//! 2  tooling error.
//! 3  authentication required or expired (distinguish create-401 vs poll-401 in stderr).

use std::process::ExitCode;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Exit {
    Ok = 0,
    HighSeverity = 1,
    ToolingError = 2,
    AuthRequired = 3,
}

impl Exit {
    pub fn code(self) -> ExitCode {
        ExitCode::from(self as u8)
    }

    /// Pick the "more severe" of two exits per the stable taxonomy.
    /// Used by the pre-push aggregator to combine per-tuple results
    /// (spec §4.5.5: overall exit is the max severity across tuples).
    pub fn max(self, other: Exit) -> Exit {
        // Severity order matches the numeric variant values: 0 < 1 < 2 < 3.
        if (other as u8) > (self as u8) {
            other
        } else {
            self
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum CliError {
    #[error("Run `quorum link --project <id>` first")]
    NotLinked,
    #[error("Run `quorum auth login`")]
    NotAuthenticated,
    #[error("Session ended mid-review (rotation or expiry). Run `quorum auth login` and retry.")]
    PollUnauthorized,
    #[error("Authentication required. Run `quorum auth login`.")]
    CreateUnauthorized,
    #[error("Login rejected: {0}")]
    LoginRejected(String),
    #[error("Consensus session {state}: {detail}")]
    ConsensusTerminal { state: String, detail: String },
    #[error("Consensus session timed out — last status: {0}")]
    PollTimeout(String),
    #[error("network error: {0}; retry may succeed.")]
    Network(String),
    #[error("Diff too large — bundle size {0}KB exceeds 200KB cap")]
    BundleTooLarge(usize),
    #[error("malformed Lippa response: {0}")]
    Malformed(String),
    #[error("local i/o error: {0}")]
    Io(String),
    #[error("quorum auth login requires an interactive terminal.")]
    NoTtyForInteractive,
    #[error("config error: {0}")]
    Config(String),
    #[error("git error: {0}")]
    Git(String),
    #[error("keyring error: {0}")]
    Keyring(String),
}

impl CliError {
    pub fn exit(&self) -> Exit {
        use CliError::*;
        match self {
            NotAuthenticated | CreateUnauthorized | PollUnauthorized | LoginRejected(_) => {
                Exit::AuthRequired
            }
            _ => Exit::ToolingError,
        }
    }
}
