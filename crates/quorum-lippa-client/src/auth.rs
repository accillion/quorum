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
    /// 403 `mfa_required`: the password was right and the account has
    /// two-factor authentication on. Lippa also set a *pending* session
    /// cookie on that response; it is signed out by construction and is
    /// never captured.
    #[error("two-factor authentication required")]
    MfaRequired,
    /// 403 `email_not_verified`.
    #[error("email address not verified")]
    EmailNotVerified,
    /// 403 whose detail is Lippa's suspension notice (it carries no code).
    #[error("account suspended")]
    AccountSuspended,
    /// Any other 403 whose body names a Lippa error code.
    #[error("login rejected by server: {status} ({code})")]
    LoginRejectedWithCode {
        status: reqwest::StatusCode,
        code: String,
    },
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
    login_on(&client, req).await
}

/// The login request on a caller's client. [`crate::SignIn`] passes one with
/// a cookie jar, so the pending cookie of a two-factor challenge stays there.
pub(crate) async fn login_on(
    client: &reqwest::Client,
    req: LoginRequest<'_>,
) -> Result<Secret, AuthError> {
    let resp = client
        .post(format!("{}/api/v1/auth/login", req.base_url))
        .json(&serde_json::json!({ "email": req.email, "password": req.password }))
        .send()
        .await
        .map_err(|e| AuthError::Transport(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(login_refusal(resp).await);
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

/// The most of a refused login's body that is read. Lippa's refusals are
/// tens of bytes; anything past this is not one of them.
const REFUSAL_BODY_CAP: usize = 16 * 1024;

/// How long a 403 body may take to arrive. Before the body was read a 403
/// returned at once, so a stalled body must not turn it into a hang.
const REFUSAL_BODY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Lippa's suspension refusal is a bare sentence with no error code
/// (`routes/api_v1/auth.py:814-818`), so it is recognised by its opening.
const SUSPENDED_NOTICE: &str = "Your account has been suspended.";

/// Turn a non-2xx login response into an [`AuthError`]. Never captures a
/// cookie: a 403 `mfa_required` carries Lippa's pending (signed-out)
/// session cookie, and nothing here reads `Set-Cookie`.
async fn login_refusal(resp: reqwest::Response) -> AuthError {
    let status = resp.status();
    if status != reqwest::StatusCode::FORBIDDEN {
        // Only a 403 body is ever classified; skip reading anything else.
        return AuthError::LoginRejected(status);
    }
    let body = tokio::time::timeout(REFUSAL_BODY_TIMEOUT, read_capped(resp, REFUSAL_BODY_CAP))
        .await
        .ok()
        .flatten();
    classify_login_refusal(status, body.as_deref())
}

/// Read at most `cap` bytes of body; `None` if it is longer or unreadable.
async fn read_capped(mut resp: reqwest::Response, cap: usize) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await.ok()? {
        if buf.len() + chunk.len() > cap {
            return None;
        }
        buf.extend_from_slice(&chunk);
    }
    Some(buf)
}

/// Map a refused login to a distinct error. Only 403 bodies are read; any
/// other status, and any 403 body that is not one of Lippa's JSON shapes,
/// gives the plain [`AuthError::LoginRejected`] Quorum always returned.
fn classify_login_refusal(status: reqwest::StatusCode, body: Option<&[u8]>) -> AuthError {
    if status != reqwest::StatusCode::FORBIDDEN {
        return AuthError::LoginRejected(status);
    }
    let Some(json) = body.and_then(|b| serde_json::from_slice::<serde_json::Value>(b).ok()) else {
        return AuthError::LoginRejected(status);
    };
    if let Some(detail) = json.get("detail").and_then(|d| d.as_str()) {
        if detail.starts_with(SUSPENDED_NOTICE) {
            return AuthError::AccountSuspended;
        }
    }
    match lippa_error_code(&json) {
        Some("mfa_required") => AuthError::MfaRequired,
        Some("email_not_verified") => AuthError::EmailNotVerified,
        Some(code) => AuthError::LoginRejectedWithCode {
            status,
            code: code.to_string(),
        },
        None => AuthError::LoginRejected(status),
    }
}

/// The error code in one of Lippa's refusal shapes: `{"detail":{"error":c}}`
/// (api_v1 routes), `{"detail":{"code":c}}` (api_v1 `code` convention) or
/// `{"error":c}` (the CSRF gate and the legacy routes). Only a plain
/// snake_case identifier counts: the value is printed to a terminal, and
/// server text that is anything else is dropped rather than shown.
fn lippa_error_code(json: &serde_json::Value) -> Option<&str> {
    let detail = json.get("detail");
    let code = detail
        .and_then(|d| d.get("error"))
        .or_else(|| detail.and_then(|d| d.get("code")))
        .or_else(|| json.get("error"))
        .and_then(|c| c.as_str())?;
    let plain = !code.is_empty()
        && code.len() <= 64
        && code.starts_with(|c: char| c.is_ascii_lowercase())
        && code
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    plain.then_some(code)
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

    fn refuse(status: u16, body: &str) -> AuthError {
        classify_login_refusal(
            reqwest::StatusCode::from_u16(status).unwrap(),
            Some(body.as_bytes()),
        )
    }

    #[test]
    fn refusal_403_codes_map_to_distinct_variants() {
        assert!(matches!(
            refuse(403, r#"{"detail":{"error":"mfa_required"}}"#),
            AuthError::MfaRequired
        ));
        assert!(matches!(
            refuse(
                403,
                r#"{"detail":{"error":"email_not_verified","email":"x***@e.com","message":"m"}}"#
            ),
            AuthError::EmailNotVerified
        ));
        assert!(matches!(
            refuse(
                403,
                r#"{"detail":"Your account has been suspended. Contact support if you believe this is an error."}"#
            ),
            AuthError::AccountSuspended
        ));
    }

    #[test]
    fn refusal_403_other_code_is_kept_from_each_lippa_shape() {
        for body in [
            r#"{"detail":{"error":"some_new_code"}}"#,
            r#"{"detail":{"code":"some_new_code"}}"#,
            r#"{"error":"some_new_code","message":"m"}"#,
        ] {
            match refuse(403, body) {
                AuthError::LoginRejectedWithCode { status, code } => {
                    assert_eq!(status, reqwest::StatusCode::FORBIDDEN);
                    assert_eq!(code, "some_new_code");
                }
                o => panic!("{body}: got {o:?}"),
            }
        }
    }

    #[test]
    fn refusal_falls_back_to_plain_rejection() {
        // Not JSON, no code, a code that is not a plain identifier, an
        // unreadable body, and a non-403 status (even one carrying a code).
        for body in [
            "<html>403</html>",
            "",
            r#"{"detail":"Some other sentence."}"#,
            r#"{"detail":{"error":"Has Spaces"}}"#,
            r#"{"detail":{"error":"esc\u001b[31m"}}"#,
            r#"{"detail":{"error":""}}"#,
            r#"{"detail":{"error":42}}"#,
        ] {
            assert!(
                matches!(refuse(403, body), AuthError::LoginRejected(s) if s == 403),
                "{body}"
            );
        }
        assert!(matches!(
            classify_login_refusal(reqwest::StatusCode::FORBIDDEN, None),
            AuthError::LoginRejected(_)
        ));
        assert!(matches!(
            refuse(401, r#"{"detail":{"error":"mfa_required"}}"#),
            AuthError::LoginRejected(s) if s == 401
        ));
    }

    #[tokio::test]
    async fn login_403_returns_the_refusal_and_no_cookie() {
        // The pending cookie Lippa sets on the challenge never comes back
        // out of login_with_cookie: the error variant carries no value.
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/api/v1/auth/login")
            .with_status(403)
            .with_header("set-cookie", "session=pending-value; path=/; httponly")
            .with_body(r#"{"detail":{"error":"mfa_required"}}"#)
            .create_async()
            .await;
        let url = server.url();
        let err = login_with_cookie(LoginRequest {
            base_url: &url,
            email: "a@b",
            password: "x",
        })
        .await
        .expect_err("403 is a refusal");
        assert!(matches!(err, AuthError::MfaRequired), "got {err:?}");
    }

    #[tokio::test]
    async fn login_403_with_oversized_body_is_a_plain_rejection() {
        let mut server = mockito::Server::new_async().await;
        let big = format!(
            r#"{{"detail":{{"error":"mfa_required"}},"pad":"{}"}}"#,
            "x".repeat(REFUSAL_BODY_CAP)
        );
        let _m = server
            .mock("POST", "/api/v1/auth/login")
            .with_status(403)
            .with_body(big)
            .create_async()
            .await;
        let url = server.url();
        let err = login_with_cookie(LoginRequest {
            base_url: &url,
            email: "a@b",
            password: "x",
        })
        .await
        .expect_err("403 is a refusal");
        assert!(
            matches!(err, AuthError::LoginRejected(s) if s == 403),
            "got {err:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn login_403_whose_body_stalls_is_a_plain_rejection() {
        // A 403 whose headers arrive but whose body never ends. With time
        // paused, the read timeout fires as soon as nothing else can run.
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (release, held) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf);
            sock.write_all(
                b"HTTP/1.1 403 Forbidden\r\ncontent-type: application/json\r\ncontent-length: 1000\r\n\r\n{\"detail\":",
            )
            .unwrap();
            // Hold the connection open, body unfinished, until released.
            let _ = held.recv_timeout(std::time::Duration::from_secs(60));
        });
        let url = format!("http://{addr}");
        let started = std::time::Instant::now();
        let err = login_with_cookie(LoginRequest {
            base_url: &url,
            email: "a@b",
            password: "x",
        })
        .await
        .expect_err("403 is a refusal");
        // Wall clock: the server releases the socket only after 60 s, so
        // returning sooner proves the read timeout, not the close, ended it.
        assert!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "a stalled 403 body held the login for {:?}",
            started.elapsed()
        );
        assert!(
            matches!(err, AuthError::LoginRejected(s) if s == 403),
            "got {err:?}"
        );
        let _ = release.send(());
        server.join().unwrap();
    }
}
