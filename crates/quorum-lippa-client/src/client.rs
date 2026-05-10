//! `LippaClient`: create / poll / fetch Consensus sessions.

use crate::auth::{AuthError, AuthMethod};
use crate::consensus::{SessionCreateRequest, SessionStatus};
use crate::secret::Secret;
use crate::wire;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ClientError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("server returned {0}: {1}")]
    HttpStatus(reqwest::StatusCode, String),
    #[error("malformed server response: {0}")]
    Malformed(String),
    #[error("authentication error: {0}")]
    Auth(#[from] AuthError),
    #[error("polling timed out after {0:?} (last status: {1})")]
    PollTimeout(Duration, String),
    #[error("session terminated by server: {0}")]
    SessionTerminal(String),
}

#[derive(Debug, Clone)]
pub struct PollOptions {
    pub initial_delay: Duration,
    pub max_delay: Duration,
    pub overall_timeout: Duration,
}

impl Default for PollOptions {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(8),
            overall_timeout: Duration::from_secs(300),
        }
    }
}

pub struct LippaClient {
    inner: reqwest::Client,
    base_url: String,
    auth: AuthMethod,
    /// In-memory cookie mirror, updated on `Set-Cookie` reissue.
    session_cookie: Mutex<Secret>,
    /// The initial cookie value, captured at construction so `renewed_cookie`
    /// can compare and only return `Some` when the value actually changed.
    initial_cookie: Secret,
}

impl LippaClient {
    pub fn new(base_url: String, auth: AuthMethod) -> Result<Self, ClientError> {
        let inner = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let initial = match &auth {
            AuthMethod::Cookie(s) => s.clone(),
            AuthMethod::Bearer(s) => s.clone(),
        };
        Ok(Self {
            inner,
            base_url,
            auth,
            session_cookie: Mutex::new(initial.clone()),
            initial_cookie: initial,
        })
    }

    fn auth_apply(
        &self,
        builder: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, ClientError> {
        // Re-apply with current cookie value (auth_method snapshots at
        // construction; reissues mutate the in-memory mirror, not the
        // AuthMethod). So we override Cookie header here when in cookie mode.
        match &self.auth {
            AuthMethod::Cookie(_) => {
                let cookie = self.session_cookie.lock().unwrap().clone();
                Ok(builder.header(
                    reqwest::header::COOKIE,
                    format!("session={}", cookie.expose()),
                ))
            }
            AuthMethod::Bearer(_) => Err(ClientError::Auth(AuthError::BearerNotYetSupported)),
        }
    }

    fn capture_renewed_cookie(&self, headers: &reqwest::header::HeaderMap) {
        if let Ok(new) = crate::auth::extract_session_cookie(headers) {
            let mut cur = self.session_cookie.lock().unwrap();
            if *cur != new {
                *cur = new;
            }
        }
    }

    pub async fn create_session(
        &self,
        req: SessionCreateRequest,
    ) -> Result<SessionId, ClientError> {
        // Multipart form per recon-v0 + preflight Blocker 2.
        let mut form = reqwest::multipart::Form::new()
            .text("prompt", req.prompt)
            .text("debate_mode", req.debate_mode);
        if let Some(p) = req.project_id {
            form = form.text("project_id", p);
        }
        if let Some(k) = req.idempotency_key {
            form = form.text("idempotency_key", k);
        }

        let url = format!("{}/api/v1/consensus/sessions", self.base_url);
        let req_builder = self.inner.post(&url).multipart(form);
        let req_builder = self.auth_apply(req_builder)?;

        let resp = req_builder
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;

        self.capture_renewed_cookie(resp.headers());
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;

        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ClientError::Auth(AuthError::LoginRequired(
                "Run `quorum auth login`",
            )));
        }
        if status == reqwest::StatusCode::CONFLICT {
            // Duplicate idempotency: server returns existing session id.
            if let Ok(dup) = serde_json::from_str::<wire::CreateDuplicateBody>(&body) {
                return Ok(SessionId(dup.existing_session_id));
            }
        }
        if !status.is_success() {
            return Err(ClientError::HttpStatus(status, body));
        }
        let parsed: wire::CreateOkBody = serde_json::from_str(&body)
            .map_err(|e| ClientError::Malformed(format!("create-session: {e}")))?;
        Ok(SessionId(parsed.id))
    }

    pub async fn poll_status(
        &self,
        id: &SessionId,
    ) -> Result<(SessionStatus, RetryHints), ClientError> {
        let url = format!(
            "{}/api/v1/consensus/sessions/{}/status",
            self.base_url, id.0
        );
        let resp = self
            .auth_apply(self.inner.get(&url))?
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;

        self.capture_renewed_cookie(resp.headers());
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .map(Duration::from_secs);
        let status_code = resp.status();
        if status_code == reqwest::StatusCode::UNAUTHORIZED {
            return Ok((SessionStatus::Unauthorized, RetryHints { retry_after }));
        }
        if status_code.is_server_error() || status_code == reqwest::StatusCode::TOO_MANY_REQUESTS {
            // Treat as in-progress with retry hints; caller decides whether
            // to keep polling.
            return Ok((SessionStatus::InProgress, RetryHints { retry_after }));
        }
        if !status_code.is_success() {
            let body = resp
                .text()
                .await
                .map_err(|e| ClientError::Transport(e.to_string()))?;
            return Err(ClientError::HttpStatus(status_code, body));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let parsed: wire::StatusBody = serde_json::from_str(&body)
            .map_err(|e| ClientError::Malformed(format!("status: {e}")))?;
        Ok((
            SessionStatus::from_status_string(&parsed.status),
            RetryHints { retry_after },
        ))
    }

    /// Returns Lippa's raw detail JSON. `quorum-core::review_from_json`
    /// is the only legal mapping site (criterion 36).
    pub async fn fetch_detail(&self, id: &SessionId) -> Result<serde_json::Value, ClientError> {
        let url = format!("{}/api/v1/consensus/sessions/{}", self.base_url, id.0);
        let resp = self
            .auth_apply(self.inner.get(&url))?
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;

        self.capture_renewed_cookie(resp.headers());
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ClientError::Auth(AuthError::SessionEnded(
                "401 on detail fetch",
            )));
        }
        if !status.is_success() {
            let body = resp
                .text()
                .await
                .map_err(|e| ClientError::Transport(e.to_string()))?;
            return Err(ClientError::HttpStatus(status, body));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let v: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| ClientError::Malformed(format!("detail: {e}")))?;
        Ok(v)
    }

    /// Whoami ping for `quorum auth status`. Returns the parsed user
    /// object on 200, or distinguishes 401 (stale session) from network /
    /// other errors via `AuthError::LoginRequired` vs `Transport`.
    pub async fn whoami(&self) -> Result<serde_json::Value, ClientError> {
        let url = format!("{}/api/v1/me", self.base_url);
        let resp = self
            .auth_apply(self.inner.get(&url))?
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        self.capture_renewed_cookie(resp.headers());
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ClientError::Auth(AuthError::LoginRequired(
                "session is stale",
            )));
        }
        if !status.is_success() {
            let body = resp
                .text()
                .await
                .map_err(|e| ClientError::Transport(e.to_string()))?;
            return Err(ClientError::HttpStatus(status, body));
        }
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ClientError::Malformed(format!("whoami: {e}")))?;
        Ok(v)
    }

    /// Best-effort POST /api/v1/auth/logout. Returns Ok regardless of
    /// outcome but logs failures.
    pub async fn logout_server_side(&self) -> Result<(), ClientError> {
        let url = format!("{}/api/v1/auth/logout", self.base_url);
        let req = self.auth_apply(self.inner.post(&url).timeout(Duration::from_secs(5)))?;
        let _ = req.send().await; // best-effort
        Ok(())
    }

    /// Returns the current cookie value if it differs from the value the
    /// client was constructed with. Caller persists to active Storage at
    /// clean exit (spec §4.7).
    pub fn renewed_cookie(&self) -> Option<Secret> {
        let cur = self.session_cookie.lock().unwrap().clone();
        if cur != self.initial_cookie {
            Some(cur)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RetryHints {
    /// Honored by the caller's polling loop, overriding backoff/jitter.
    pub retry_after: Option<Duration>,
}

/// Compute the next poll delay given the current attempt's base delay,
/// the max delay, and an optional `Retry-After` hint.
///
/// `Retry-After`, when present, *replaces* the schedule for one tick
/// (AC 29). Otherwise: `base * (0.75 + rand * 0.5)` to apply ±25% jitter.
pub fn next_poll_delay(
    base: Duration,
    max: Duration,
    retry_after: Option<Duration>,
    jitter_unit: f32,
) -> Duration {
    if let Some(ra) = retry_after {
        return ra;
    }
    let next_base = (base * 2).min(max);
    let factor = 0.75 + jitter_unit.clamp(0.0, 1.0) * 0.5;
    Duration::from_secs_f32(next_base.as_secs_f32() * factor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_delay_honors_retry_after() {
        let d = next_poll_delay(
            Duration::from_secs(1),
            Duration::from_secs(8),
            Some(Duration::from_secs(13)),
            0.5,
        );
        assert_eq!(d, Duration::from_secs(13));
    }

    #[test]
    fn next_delay_doubles_until_max_then_jitter() {
        let d = next_poll_delay(Duration::from_secs(1), Duration::from_secs(8), None, 0.5);
        // 2s with no jitter offset (factor = 1.0 at jitter_unit=0.5)
        assert!(d >= Duration::from_secs_f32(1.5));
        assert!(d <= Duration::from_secs_f32(2.5));
    }

    #[test]
    fn next_delay_caps_at_max() {
        let d = next_poll_delay(Duration::from_secs(16), Duration::from_secs(8), None, 1.0);
        // Cap is 8s; jitter factor at unit=1 → 1.25; result <= 10s.
        assert!(d <= Duration::from_secs_f32(10.5));
    }
}
