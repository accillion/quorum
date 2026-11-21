//! Sign-in that can finish Lippa's two-factor step.
//!
//! An account with two-factor authentication on answers the login with 403
//! `mfa_required` and a *pending* session cookie: it has no `user_id`, reads
//! as signed out, and Lippa forgets it after 10 minutes.
//! `POST /api/v1/auth/mfa/verify` with a code, carrying that cookie, answers
//! 200 with the real session cookie.
//!
//! One [`SignIn`] owns one HTTP client with one cookie jar for both requests.
//! The jar is the only carrier of the pending cookie. No request here sets a
//! `Cookie` header by hand, because the Lippa edge rejected hand-attached
//! cookies (SERVICES.md §4). Nothing reads the pending cookie back out of the
//! jar, and it is dropped with the `SignIn`.

use std::sync::Arc;
use std::time::Duration;

use reqwest::cookie::Jar;
use reqwest::StatusCode;

use crate::auth::{extract_session_cookie, login_on, AuthError, LoginRequest};
use crate::secret::Secret;

/// The most of a verify response body that is read. Lippa's are tens of bytes.
const VERIFY_BODY_CAP: usize = 16 * 1024;

/// One login, from password to session, on one client and one cookie jar.
pub struct SignIn {
    client: reqwest::Client,
    base_url: String,
}

/// What the user typed at the code prompt, as the verify route takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecondFactor {
    /// Six digits from an authenticator app, whitespace removed.
    Code(String),
    /// Anything else: a recovery code, trimmed and otherwise as typed. Lippa
    /// ignores its case and separators.
    RecoveryCode(String),
}

impl SecondFactor {
    /// Six digits, ignoring whitespace, are a code; anything else is a
    /// recovery code.
    pub fn from_input(input: &str) -> Self {
        let squeezed: String = input.chars().filter(|c| !c.is_whitespace()).collect();
        if squeezed.len() == 6 && squeezed.bytes().all(|b| b.is_ascii_digit()) {
            SecondFactor::Code(squeezed)
        } else {
            SecondFactor::RecoveryCode(input.trim().to_string())
        }
    }
}

/// A completed second step.
#[derive(Debug)]
pub struct Verified {
    /// The real session cookie, from the verify response's `Set-Cookie`.
    pub session: Secret,
    /// `recovery_codes_remaining` from Lippa's answer, when it has one.
    pub recovery_codes_remaining: Option<u64>,
}

#[derive(thiserror::Error, Debug)]
pub enum VerifyError {
    /// 401 `invalid_code`: a wrong code, or a locked account. Counts toward
    /// the same lockout as a wrong password.
    #[error("the code was not accepted")]
    InvalidCode,
    /// 401 `mfa_session_expired`: the pending sign-in is gone.
    #[error("the pending sign-in expired")]
    SessionExpired,
    /// 429. `Retry-After` is not read.
    #[error("rate limited")]
    RateLimited,
    /// Any other refusal.
    #[error("verify rejected by server: {0}")]
    Rejected(StatusCode),
    /// A 200 without a usable session cookie, or a transport failure.
    #[error(transparent)]
    Auth(#[from] AuthError),
}

impl SignIn {
    pub fn new(base_url: &str) -> Result<Self, AuthError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .cookie_provider(Arc::new(Jar::default()))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| AuthError::Transport(e.to_string()))?;
        Ok(Self {
            client,
            base_url: base_url.to_string(),
        })
    }

    /// Step 1: the same request, and the same errors, as
    /// [`crate::login_with_cookie`]. After [`AuthError::MfaRequired`] the
    /// pending cookie is in this sign-in's jar, ready for [`SignIn::verify`].
    pub async fn login(&self, email: &str, password: &str) -> Result<Secret, AuthError> {
        login_on(
            &self.client,
            LoginRequest {
                base_url: &self.base_url,
                email,
                password,
            },
        )
        .await
    }

    /// Step 2: `POST /api/v1/auth/mfa/verify`, carrying the pending cookie
    /// from the jar.
    pub async fn verify(&self, factor: &SecondFactor) -> Result<Verified, VerifyError> {
        let body = match factor {
            SecondFactor::Code(c) => serde_json::json!({ "code": c }),
            SecondFactor::RecoveryCode(c) => serde_json::json!({ "recovery_code": c }),
        };
        let resp = self
            .client
            .post(format!("{}/api/v1/auth/mfa/verify", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| AuthError::Transport(e.to_string()))?;
        let status = resp.status();
        if status.is_success() {
            // The real cookie comes from this response, never from the jar,
            // which still holds the pending one if this response set none.
            let session = extract_session_cookie(resp.headers())?;
            let recovery_codes_remaining = read_json(resp)
                .await
                .and_then(|v| v.get("recovery_codes_remaining")?.as_u64());
            return Ok(Verified {
                session,
                recovery_codes_remaining,
            });
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(VerifyError::RateLimited);
        }
        if status == StatusCode::UNAUTHORIZED {
            // Lippa's verify errors are `{"detail": {"error": <code>}}`.
            let code = read_json(resp)
                .await
                .and_then(|v| v.get("detail")?.get("error")?.as_str().map(str::to_owned));
            match code.as_deref() {
                Some("invalid_code") => return Err(VerifyError::InvalidCode),
                Some("mfa_session_expired") => return Err(VerifyError::SessionExpired),
                _ => {}
            }
        }
        Err(VerifyError::Rejected(status))
    }
}

/// The body as JSON, if it is at most [`VERIFY_BODY_CAP`] bytes of JSON. The
/// client's 30 s timeout bounds how long this can take.
async fn read_json(mut resp: reqwest::Response) -> Option<serde_json::Value> {
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await.ok()? {
        if buf.len() + chunk.len() > VERIFY_BODY_CAP {
            return None;
        }
        buf.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&buf).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn six_digits_ignoring_whitespace_are_a_code() {
        for input in ["123456", " 123 456 ", "123\t456\n", "0 0 0 0 0 0"] {
            let digits: String = input.chars().filter(|c| !c.is_whitespace()).collect();
            assert_eq!(
                SecondFactor::from_input(input),
                SecondFactor::Code(digits),
                "{input:?}"
            );
        }
    }

    #[test]
    fn anything_else_is_a_recovery_code() {
        for (input, sent) in [
            ("ABCDE-FGHIJ", "ABCDE-FGHIJ"),
            ("  abcde fghij ", "abcde fghij"),
            ("12345", "12345"),
            ("1234567", "1234567"),
            ("12a456", "12a456"),
            ("１２３４５６", "１２３４５６"), // full-width digits are not ASCII
        ] {
            assert_eq!(
                SecondFactor::from_input(input),
                SecondFactor::RecoveryCode(sent.to_string()),
                "{input:?}"
            );
        }
    }

    #[tokio::test]
    async fn the_jar_carries_the_pending_cookie_to_verify() {
        let mut server = mockito::Server::new_async().await;
        let _login = server
            .mock("POST", "/api/v1/auth/login")
            .with_status(403)
            .with_header(
                "set-cookie",
                "session=pending-value; path=/; Max-Age=1209600; httponly; samesite=lax; secure",
            )
            .with_body(r#"{"detail":{"error":"mfa_required"}}"#)
            .create_async()
            .await;
        let verify = server
            .mock("POST", "/api/v1/auth/mfa/verify")
            .match_header("cookie", "session=pending-value")
            .match_body(mockito::Matcher::Json(
                serde_json::json!({ "code": "123456" }),
            ))
            .with_status(200)
            .with_header("set-cookie", "session=full-value; path=/; httponly")
            .with_body(r#"{"ok":true,"method":"totp","recovery_codes_remaining":7}"#)
            .create_async()
            .await;
        let sign_in = SignIn::new(&server.url()).unwrap();
        let err = sign_in.login("a@b", "pw").await.expect_err("challenge");
        assert!(matches!(err, AuthError::MfaRequired), "{err:?}");
        let ok = sign_in
            .verify(&SecondFactor::Code("123456".into()))
            .await
            .expect("verified");
        verify.assert_async().await;
        assert_eq!(ok.session.expose(), "full-value");
        assert_eq!(ok.recovery_codes_remaining, Some(7));
    }

    async fn verify_answer(status: usize, body: &str) -> Result<Verified, VerifyError> {
        let mut server = mockito::Server::new_async().await;
        let _v = server
            .mock("POST", "/api/v1/auth/mfa/verify")
            .with_status(status)
            .with_body(body)
            .create_async()
            .await;
        SignIn::new(&server.url())
            .unwrap()
            .verify(&SecondFactor::Code("123456".into()))
            .await
    }

    #[tokio::test]
    async fn verify_refusals_are_classified() {
        assert!(matches!(
            verify_answer(401, r#"{"detail":{"error":"invalid_code"}}"#).await,
            Err(VerifyError::InvalidCode)
        ));
        assert!(matches!(
            verify_answer(401, r#"{"detail":{"error":"mfa_session_expired"}}"#).await,
            Err(VerifyError::SessionExpired)
        ));
        assert!(matches!(
            verify_answer(429, "").await,
            Err(VerifyError::RateLimited)
        ));
        assert!(matches!(
            verify_answer(401, "not json").await,
            Err(VerifyError::Rejected(s)) if s == 401
        ));
        assert!(matches!(
            verify_answer(403, r#"{"error":"csrf_origin_rejected"}"#).await,
            Err(VerifyError::Rejected(s)) if s == 403
        ));
        assert!(matches!(
            verify_answer(200, r#"{"ok":true}"#).await,
            Err(VerifyError::Auth(AuthError::NoSessionCookie))
        ));
    }
}
