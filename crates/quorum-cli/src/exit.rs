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

    /// Pick the numerically larger of two exits (0 < 1 < 2 < 3).
    ///
    /// Not a severity order for hook aggregation: it ranks a tooling
    /// error (2) above high-severity findings (1), and the fail-open
    /// template blocks only on 1. The pre-push loop combines tuples with
    /// `commands::hook_mode::aggregate_push_exits`, which keeps exit 1
    /// on top and uses this only to rank the non-blocking rest.
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
    #[error(
        "This account has two-factor authentication enabled. \
         Use QUORUM_LIPPA_SESSION together with QUORUM_LIPPA_BASE_URL."
    )]
    MfaRequired,
    #[error(
        "This account's email address is not verified. \
         Open the verification link Lippa emailed you, then log in again."
    )]
    EmailNotVerified,
    #[error(
        "This Lippa account is suspended. \
         Contact Lippa support if you believe this is an error."
    )]
    AccountSuspended,
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
    /// The two-factor step of an interactive login stopped; the text says
    /// why and what to do next.
    #[error("{0}")]
    SecondFactor(String),
    /// Session create answered 422 and the body names `tier`. Exit 2, the
    /// same as a config error: the fix is a config or flag value.
    #[error(
        "Lippa rejected the Consensus tier Quorum sent (tier = {sent:?}): {detail} \
         Set `tier` in .quorum/config.toml, or pass --tier, to one of: {allowed}.",
        allowed = quorum_core::config::ConsensusTier::ALLOWED
    )]
    TierRejected { sent: String, detail: String },
    /// v0.6 spec AC 277a (0.5.1): a pre-push new branch with no verified
    /// base was reviewed as its full tree, and the bundle budget cut the
    /// diff. Exit 2: the branch was not fully reviewed.
    #[error(
        "new-branch review exceeded the bundle budget; push a smaller branch or \
         review it with `quorum review --range <base>..<head>`"
    )]
    NewBranchTooLarge,
}

impl CliError {
    pub fn exit(&self) -> Exit {
        use CliError::*;
        match self {
            SecondFactor(_) => Exit::AuthRequired,
            NotAuthenticated | CreateUnauthorized | PollUnauthorized | LoginRejected(_) => {
                Exit::AuthRequired
            }
            MfaRequired | EmailNotVerified | AccountSuspended => Exit::AuthRequired,
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
        let head =
            quorum_core::memory::truncate_at_codepoint_boundary(trimmed, 16).to_ascii_lowercase();
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

/// Lippa's message when a 422 body names the `tier` field, else `None`.
///
/// Lippa raises `HTTPException(422, detail="tier must be one of: …")`,
/// so the body is `{"detail": "<text>"}`. A FastAPI validation error has
/// a list `detail` whose items carry a `loc` path, and an item whose
/// `loc` contains `"tier"` counts too. A body that is not JSON, or does
/// not name `tier`, returns `None` and keeps the generic HTTP message.
pub(crate) fn tier_rejection(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    match v.get("detail")? {
        serde_json::Value::String(s) if s.contains("tier") => Some(s.clone()),
        serde_json::Value::Array(items) => {
            let msgs: Vec<String> = items
                .iter()
                .filter(|it| {
                    it.get("loc")
                        .and_then(|l| l.as_array())
                        .is_some_and(|l| l.iter().any(|p| p.as_str() == Some("tier")))
                })
                .map(|it| {
                    it.get("msg")
                        .and_then(|m| m.as_str())
                        .unwrap_or("invalid value")
                        .to_string()
                })
                .collect();
            (!msgs.is_empty()).then(|| msgs.join("; "))
        }
        _ => None,
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

    /// The 16-byte HTML sniff cuts at a codepoint boundary (AC 211a's
    /// swap): a body whose 16th byte falls inside a character is
    /// classified, not a panic.
    #[test]
    fn classify_body_with_split_char_at_sniff_cut_does_not_panic() {
        // `–` (3 bytes) sits at bytes 15..18; a cut at 16 lands inside it.
        let body = "Accès refusé – réessayez";
        let msg = classify_http_status(403, body).to_string();
        assert!(msg.contains("HTTP 403"));
        assert!(msg.contains(body));
        // `é` sits at bytes 15..17, right after a 15-byte doctype.
        let msg = classify_http_status(502, "<!doctype html>é<body>x</body>").to_string();
        assert!(msg.contains("HTML page"));
    }

    /// Lippa's own tier 422 and a FastAPI validation item on `tier` are
    /// recognised. Everything else falls through to the generic message.
    #[test]
    fn tier_rejection_recognises_only_bodies_naming_tier() {
        assert_eq!(
            tier_rejection(r#"{"detail":"tier must be one of: low, medium, high."}"#).as_deref(),
            Some("tier must be one of: low, medium, high.")
        );
        assert_eq!(
            tier_rejection(
                r#"{"detail":[{"loc":["body","tier"],"msg":"bad tier"},{"loc":["body","x"],"msg":"no"}]}"#
            )
            .as_deref(),
            Some("bad tier")
        );
        for other in [
            "",
            "not json",
            "<html>422</html>",
            r#"{"detail":"Prompt must be at least 20 characters."}"#,
            r#"{"detail":[{"loc":["body","prompt"],"msg":"short"}]}"#,
            r#"{"error":"tier"}"#,
            r#"[1,2]"#,
        ] {
            assert_eq!(tier_rejection(other), None, "{other:?}");
        }
    }

    #[test]
    fn tier_rejected_is_a_tooling_error_naming_what_was_sent() {
        let e = CliError::TierRejected {
            sent: "medium".into(),
            detail: "tier must be one of: low, medium, high.".into(),
        };
        assert_eq!(e.exit(), Exit::ToolingError);
        let m = e.to_string();
        assert!(
            m.contains("tier = \"medium\"") && m.contains("low, medium, high"),
            "{m}"
        );
    }
}
