//! Auth: cookie path (Phase 1A) + typed Bearer error stub (Phase 1A+).

use crate::secret::Secret;

#[derive(Clone)]
pub enum AuthMethod {
    Cookie(Secret),
    /// Constructible but `apply` returns `BearerNotYetSupported`. The
    /// variant is here so the seam exists for the future M-ExternalAuth swap.
    Bearer(Secret),
}

#[derive(thiserror::Error, Debug)]
pub enum AuthError {
    #[error("login rejected by server: {0}")]
    LoginRejected(reqwest::StatusCode),
    #[error("server response did not contain a session cookie")]
    NoSessionCookie,
    #[error("session cookie parse failed: {0}")]
    CookieParseError(String),
    #[error("Bearer auth requires Lippa M-ExternalAuth Phase 1; not yet supported")]
    BearerNotYetSupported,
    #[error("login required: {0}")]
    LoginRequired(&'static str),
    #[error("session ended mid-review: {0}")]
    SessionEnded(&'static str),
    #[error("transport error: {0}")]
    Transport(String),
}

impl AuthMethod {
    /// Inject the auth header onto an outgoing request. Returns
    /// `BearerNotYetSupported` for the Bearer arm in Phase 1A.
    pub fn apply(
        &self,
        builder: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, AuthError> {
        match self {
            AuthMethod::Cookie(secret) => Ok(builder.header(
                reqwest::header::COOKIE,
                format!("session={}", secret.expose()),
            )),
            AuthMethod::Bearer(_) => Err(AuthError::BearerNotYetSupported),
        }
    }
}

pub struct LoginRequest<'a> {
    pub base_url: &'a str,
    pub email: &'a str,
    pub password: &'a str,
}

/// POST /api/v1/auth/login (JSON, confirmed by preflight Blocker 1).
pub async fn login_with_cookie(req: LoginRequest<'_>) -> Result<Secret, AuthError> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| AuthError::Transport(e.to_string()))?;
    let resp = client
        .post(format!("{}/api/v1/auth/login", req.base_url))
        .json(&serde_json::json!({ "email": req.email, "password": req.password }))
        .send()
        .await
        .map_err(|e| AuthError::Transport(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(AuthError::LoginRejected(resp.status()));
    }
    extract_session_cookie(resp.headers())
}

/// Parse `Set-Cookie: session=<value>` out of response headers.
/// Returns `Secret` at the boundary — no intermediate plaintext String
/// lives outside this function.
pub fn extract_session_cookie(headers: &reqwest::header::HeaderMap) -> Result<Secret, AuthError> {
    let mut found_any = false;
    for v in headers.get_all(reqwest::header::SET_COOKIE).iter() {
        found_any = true;
        let s = match v.to_str() {
            Ok(s) => s,
            Err(_) => continue,
        };
        // Find `session=...` substring. Cookies in Set-Cookie come one per
        // header value, separated by `;` for attributes.
        for cookie in s.split(',') {
            let cookie = cookie.trim();
            if let Some(rest) = cookie.strip_prefix("session=") {
                let value: String = rest.split(';').next().unwrap_or("").to_string();
                if value.is_empty() {
                    return Err(AuthError::CookieParseError(
                        "session= value is empty".into(),
                    ));
                }
                return Ok(Secret::new(value));
            }
        }
    }
    // Same error whether headers were absent or none matched `session=`;
    // distinguishing wouldn't help the caller and would mask
    // browsing-tier upgrades to multiple cookies.
    let _ = found_any;
    Err(AuthError::NoSessionCookie)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue, SET_COOKIE};

    #[test]
    fn extracts_session_cookie() {
        let mut h = HeaderMap::new();
        h.insert(
            SET_COOKIE,
            HeaderValue::from_static("session=abc; path=/; httponly"),
        );
        let s = extract_session_cookie(&h).unwrap();
        assert_eq!(s.expose(), "abc");
    }

    #[test]
    fn no_session_cookie_present() {
        let mut h = HeaderMap::new();
        h.insert(SET_COOKIE, HeaderValue::from_static("other=val"));
        match extract_session_cookie(&h) {
            Err(AuthError::NoSessionCookie) => {}
            o => panic!("got {o:?}"),
        }
    }

    #[test]
    fn empty_session_value_is_parse_error() {
        let mut h = HeaderMap::new();
        h.insert(SET_COOKIE, HeaderValue::from_static("session=; path=/"));
        match extract_session_cookie(&h) {
            Err(AuthError::CookieParseError(_)) => {}
            o => panic!("got {o:?}"),
        }
    }

    #[test]
    fn bearer_apply_returns_typed_error() {
        let am = AuthMethod::Bearer(Secret::new("tok".into()));
        let client = reqwest::Client::new();
        let builder = client.get("https://example.invalid/");
        match am.apply(builder) {
            Err(AuthError::BearerNotYetSupported) => {}
            o => panic!("got {o:?}"),
        }
    }
}
