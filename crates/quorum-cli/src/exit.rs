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
    #[error("{}", render_http_status(*status, *body_is_html, body_preview))]
    HttpStatus {
        status: u16,
        body_is_html: bool,
        body_preview: String,
    },
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

/// Classify a non-2xx Lippa response body and build a status-appropriate
/// CLI message. HTML bodies (typical from an edge proxy that intercepts
/// before Lippa sees the request) are suppressed: a dump of `<!doctype
/// html><html>...` into stderr is what BUG 2 actually surfaced.
pub(crate) fn classify_http_status(status: u16, body: &str) -> CliError {
    let trimmed = body.trim_start();
    let body_is_html = trimmed.len() >= 5 && {
        let head = &trimmed[..trimmed.len().min(16)].to_ascii_lowercase();
        head.starts_with("<!doctype") || head.starts_with("<html")
    };
    let body_preview = if body_is_html {
        body.chars().take(100).collect()
    } else {
        body.to_string()
    };
    CliError::HttpStatus {
        status,
        body_is_html,
        body_preview,
    }
}

fn render_http_status(status: u16, body_is_html: bool, body_preview: &str) -> String {
    let head = match status {
        400 => "request rejected by Lippa (HTTP 400); the bundle may be malformed".to_string(),
        401 => "authentication required (HTTP 401); run `quorum auth login`".to_string(),
        403 => "authorization failed (HTTP 403); check your Quorum account, project binding, and Lippa plan tier".to_string(),
        404 => "Lippa endpoint not found (HTTP 404); check your project id and base URL".to_string(),
        409 => "Lippa rejected as conflict (HTTP 409); retry may succeed".to_string(),
        429 => "Lippa rate-limited the request (HTTP 429); retry in a minute".to_string(),
        s if (500..=599).contains(&s) => format!(
            "Lippa returned a server error (HTTP {s}); this is usually transient — retry in a minute"
        ),
        s => format!("Lippa returned HTTP {s}"),
    };
    if body_is_html {
        format!("{head} (server returned an HTML page; full body suppressed — re-run with RUST_LOG=debug to see)")
    } else if body_preview.is_empty() {
        head
    } else {
        format!("{head}: {body_preview}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_403_html_suppresses_body() {
        let err = classify_http_status(403, "<!DOCTYPE html><html><body>nope</body></html>");
        let msg = err.to_string();
        assert!(msg.contains("authorization failed"));
        assert!(msg.contains("HTTP 403"));
        assert!(msg.contains("HTML page"));
        assert!(!msg.contains("<body>"), "raw HTML must not leak: {msg}");
    }

    #[test]
    fn classify_500_json_includes_body_verbatim() {
        let err = classify_http_status(500, r#"{"detail":"db down"}"#);
        let msg = err.to_string();
        assert!(msg.contains("server error"));
        assert!(msg.contains("HTTP 500"));
        assert!(msg.contains(r#"{"detail":"db down"}"#));
    }

    #[test]
    fn classify_400_short_body_renders() {
        let err = classify_http_status(400, "missing project_id");
        let msg = err.to_string();
        assert!(msg.contains("HTTP 400"));
        assert!(msg.contains("missing project_id"));
    }

    #[test]
    fn network_variant_still_says_retry() {
        let err = CliError::Network("connection refused".into());
        let msg = err.to_string();
        assert!(msg.contains("network error"));
        assert!(msg.contains("retry may succeed"));
    }

    #[test]
    fn body_is_html_detection_is_case_insensitive() {
        let err = classify_http_status(502, "<HTML><body>bad gateway</body></HTML>");
        let msg = err.to_string();
        assert!(msg.contains("HTML page"));
        assert!(!msg.contains("bad gateway"));
    }
}
