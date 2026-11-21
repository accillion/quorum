//! Interactive login for an account with two-factor authentication on.
//!
//! Lippa answers such an account's login with 403 `mfa_required` and a
//! pending session cookie. [`sign_in`] then asks for a code and completes
//! the login with `POST /api/v1/auth/mfa/verify`, on the same client and
//! cookie jar ([`quorum_lippa_client::SignIn`]). The pending cookie stays in
//! that jar: it is never returned, printed or stored.

use crate::exit::CliError;
use quorum_lippa_client::{AuthError, SecondFactor, Secret, SignIn, VerifyError};

/// Codes tried before the sign-in stops. Each wrong one counts toward the
/// same lockout as a wrong password, so the user keeps attempts in reserve.
pub(crate) const MAX_CODE_ATTEMPTS: usize = 3;

/// At or below this many recovery codes left, the user is told.
const FEW_RECOVERY_CODES: u64 = 3;

const CODE_PROMPT: &str = "Authentication code (or a recovery code): ";

/// Where the code prompt is asked and where progress lines go.
pub(crate) trait CodePrompt {
    /// Ask for a code with hidden input. `None` when there is no terminal to
    /// ask on, so nothing is prompted.
    fn ask(&mut self, prompt: &str) -> Option<std::io::Result<String>>;
    /// One line for the user (stderr).
    fn say(&mut self, line: &str);
}

/// The real terminal: `rpassword` for the code, stderr for the rest.
pub(crate) struct TerminalPrompt;

impl CodePrompt for TerminalPrompt {
    fn ask(&mut self, prompt: &str) -> Option<std::io::Result<String>> {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() {
            return None;
        }
        Some(rpassword::prompt_password(prompt))
    }

    fn say(&mut self, line: &str) {
        eprintln!("{line}");
    }
}

/// Log in, and for an account with two-factor authentication on, ask for
/// a code and complete the second step.
///
/// The inner `Result` is the login outcome, for the caller's usual mapping:
/// the session, or the login's own refusal. That includes
/// [`AuthError::MfaRequired`] when there is no terminal to ask on. The outer
/// error means the second step stopped, and it already says why.
pub(crate) async fn sign_in(
    base_url: &str,
    email: &str,
    password: &str,
    prompt: &mut dyn CodePrompt,
) -> Result<Result<Secret, AuthError>, CliError> {
    let flow = match SignIn::new(base_url) {
        Ok(flow) => flow,
        Err(e) => return Ok(Err(e)),
    };
    match flow.login(email, password).await {
        Err(AuthError::MfaRequired) => {}
        other => return Ok(other),
    }
    for attempt in 1..=MAX_CODE_ATTEMPTS {
        let input = match prompt.ask(CODE_PROMPT) {
            None => return Ok(Err(AuthError::MfaRequired)),
            Some(Err(e)) => return Err(CliError::Io(e.to_string())),
            Some(Ok(input)) => input,
        };
        if input.trim().is_empty() {
            return Err(CliError::SecondFactor(
                "No code entered, so nothing was sent to Lippa. \
                 Run `quorum auth login` to start again."
                    .into(),
            ));
        }
        match flow.verify(&SecondFactor::from_input(&input)).await {
            Ok(verified) => {
                if let Some(left) = verified.recovery_codes_remaining {
                    if left <= FEW_RECOVERY_CODES {
                        prompt.say(&format!(
                            "Note: {left} recovery code{} left for this account.",
                            if left == 1 { "" } else { "s" }
                        ));
                    }
                }
                return Ok(Ok(verified.session));
            }
            Err(VerifyError::InvalidCode) if attempt == 1 => prompt.say(
                "That code was not accepted. Wrong codes count toward the same \
                 lockout as wrong passwords.",
            ),
            Err(VerifyError::InvalidCode) => prompt.say("That code was not accepted."),
            Err(e) => return Err(verify_error(e)),
        }
    }
    Err(CliError::SecondFactor(format!(
        "Stopped after {MAX_CODE_ATTEMPTS} codes were not accepted. \
         Run `quorum auth login` to try again."
    )))
}

fn verify_error(e: VerifyError) -> CliError {
    match e {
        VerifyError::SessionExpired => CliError::SecondFactor(
            "The code was not entered within Lippa's 10-minute window after the \
             password. Run `quorum auth login` again."
                .into(),
        ),
        VerifyError::RateLimited => CliError::SecondFactor(
            "Lippa is limiting sign-in attempts (HTTP 429). \
             Wait, then run `quorum auth login` again."
                .into(),
        ),
        VerifyError::Rejected(status) => {
            CliError::SecondFactor(format!("Lippa refused the two-factor step ({status})."))
        }
        VerifyError::Auth(AuthError::Transport(m)) => CliError::Network(m),
        VerifyError::Auth(AuthError::NoSessionCookie) => CliError::LoginRejected(
            "two-factor step accepted, but the server returned no session cookie".into(),
        ),
        VerifyError::Auth(other) => CliError::LoginRejected(other.to_string()),
        VerifyError::InvalidCode => CliError::SecondFactor("That code was not accepted.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Matcher;
    use std::collections::VecDeque;

    const PENDING: &str = "PENDING-MFA-COOKIE-CANARY";
    const FULL: &str = "full-session-after-verify";
    const PROMPT: &str = "Authentication code (or a recovery code): ";

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Event {
        Ask(String),
        Say(String),
    }

    /// A terminal whose answers are scripted, recording what was asked and
    /// said, in order.
    struct Scripted {
        tty: bool,
        answers: VecDeque<String>,
        events: Vec<Event>,
    }

    impl Scripted {
        fn tty(answers: &[&str]) -> Self {
            Self {
                tty: true,
                answers: answers.iter().map(|s| s.to_string()).collect(),
                events: Vec::new(),
            }
        }
        fn no_tty() -> Self {
            Self {
                tty: false,
                answers: VecDeque::new(),
                events: Vec::new(),
            }
        }
        fn asked(&self) -> usize {
            self.events
                .iter()
                .filter(|e| matches!(e, Event::Ask(_)))
                .count()
        }
        fn said(&self) -> Vec<&str> {
            self.events
                .iter()
                .filter_map(|e| match e {
                    Event::Say(s) => Some(s.as_str()),
                    Event::Ask(_) => None,
                })
                .collect()
        }
    }

    impl CodePrompt for Scripted {
        fn ask(&mut self, prompt: &str) -> Option<std::io::Result<String>> {
            if !self.tty {
                return None;
            }
            self.events.push(Event::Ask(prompt.to_string()));
            Some(Ok(self
                .answers
                .pop_front()
                .expect("prompted more often than scripted")))
        }
        fn say(&mut self, line: &str) {
            self.events.push(Event::Say(line.to_string()));
        }
    }

    /// Lippa's step 1 for an enrolled account: 403 `mfa_required` plus the
    /// pending cookie, with the attributes Lippa sets (domain aside).
    async fn mfa_login(server: &mut mockito::ServerGuard) -> mockito::Mock {
        server
            .mock("POST", "/api/v1/auth/login")
            .with_status(403)
            .with_header("content-type", "application/json")
            .with_header(
                "set-cookie",
                &format!(
                    "session={PENDING}; path=/; Max-Age=1209600; httponly; samesite=lax; secure"
                ),
            )
            .with_body(r#"{"detail":{"error":"mfa_required"}}"#)
            .expect(1)
            .create_async()
            .await
    }

    /// The verify route, matched only when the pending cookie came with it
    /// and the body is `expected`.
    async fn verify(
        server: &mut mockito::ServerGuard,
        expected: serde_json::Value,
        status: usize,
        body: &str,
        hits: usize,
    ) -> mockito::Mock {
        let mut m = server
            .mock("POST", "/api/v1/auth/mfa/verify")
            .match_header("cookie", Matcher::Regex(format!("session={PENDING}")))
            .match_body(Matcher::Json(expected))
            .with_status(status)
            .with_header("content-type", "application/json")
            .with_body(body)
            .expect(hits);
        if status == 200 {
            m = m.with_header(
                "set-cookie",
                &format!("session={FULL}; path=/; Max-Age=1209600; httponly; samesite=lax; secure"),
            );
        }
        m.create_async().await
    }

    fn verified(remaining: u32) -> String {
        format!(r#"{{"ok":true,"method":"totp","recovery_codes_remaining":{remaining}}}"#)
    }

    /// Nothing in the outcome or in what was said carries the pending cookie.
    fn assert_pending_never_surfaces(
        outcome: &Result<Result<Secret, AuthError>, CliError>,
        prompt: &Scripted,
    ) {
        let text = match outcome {
            Ok(Ok(s)) => {
                assert_ne!(s.expose(), PENDING, "the pending cookie was returned");
                format!("{s:?}")
            }
            Ok(Err(e)) => format!("{e} {e:?}"),
            Err(e) => format!("{e} {e:?}"),
        };
        assert!(!text.contains(PENDING), "pending cookie in outcome: {text}");
        for line in prompt.said() {
            assert!(!line.contains(PENDING), "pending cookie said: {line}");
        }
    }

    #[tokio::test]
    async fn totp_code_completes_the_sign_in() {
        let mut server = mockito::Server::new_async().await;
        let login = mfa_login(&mut server).await;
        let verify = verify(
            &mut server,
            serde_json::json!({ "code": "123456" }),
            200,
            &verified(10),
            1,
        )
        .await;
        let mut prompt = Scripted::tty(&[" 123 456 "]);
        let out = sign_in(&server.url(), "a@b", "pw", &mut prompt).await;
        login.assert_async().await;
        verify.assert_async().await;
        match &out {
            Ok(Ok(s)) => assert_eq!(s.expose(), FULL),
            o => panic!("expected the verified session, got {o:?}"),
        }
        assert_eq!(prompt.events, vec![Event::Ask(PROMPT.to_string())]);
        assert_pending_never_surfaces(&out, &prompt);
    }

    #[tokio::test]
    async fn recovery_code_completes_the_sign_in_and_reports_few_left() {
        let mut server = mockito::Server::new_async().await;
        let _login = mfa_login(&mut server).await;
        let verify = verify(
            &mut server,
            serde_json::json!({ "recovery_code": "abcde-fghij" }),
            200,
            &verified(2),
            1,
        )
        .await;
        let mut prompt = Scripted::tty(&["abcde-fghij"]);
        let out = sign_in(&server.url(), "a@b", "pw", &mut prompt).await;
        verify.assert_async().await;
        assert!(matches!(&out, Ok(Ok(s)) if s.expose() == FULL), "{out:?}");
        assert!(
            prompt
                .said()
                .iter()
                .any(|l| l.contains("2 recovery codes left")),
            "said: {:?}",
            prompt.said()
        );
        assert_pending_never_surfaces(&out, &prompt);
    }

    #[tokio::test]
    async fn plenty_of_recovery_codes_are_not_mentioned() {
        let mut server = mockito::Server::new_async().await;
        let _login = mfa_login(&mut server).await;
        let _verify = verify(
            &mut server,
            serde_json::json!({ "code": "123456" }),
            200,
            &verified(10),
            1,
        )
        .await;
        let mut prompt = Scripted::tty(&["123456"]);
        let out = sign_in(&server.url(), "a@b", "pw", &mut prompt).await;
        assert!(matches!(&out, Ok(Ok(_))), "{out:?}");
        assert!(prompt.said().is_empty(), "said: {:?}", prompt.said());
    }

    #[tokio::test]
    async fn three_wrong_codes_stop_and_the_lockout_is_named_on_the_first() {
        let mut server = mockito::Server::new_async().await;
        let _login = mfa_login(&mut server).await;
        let mut wrong = Vec::new();
        for code in ["111111", "222222", "333333"] {
            wrong.push(
                verify(
                    &mut server,
                    serde_json::json!({ "code": code }),
                    401,
                    r#"{"detail":{"error":"invalid_code"}}"#,
                    1,
                )
                .await,
            );
        }
        // A fourth answer is scripted but must never be asked for.
        let mut prompt = Scripted::tty(&["111111", "222222", "333333", "444444"]);
        let out = sign_in(&server.url(), "a@b", "pw", &mut prompt).await;
        for m in &wrong {
            m.assert_async().await;
        }
        assert_eq!(prompt.asked(), 3, "events: {:?}", prompt.events);
        let lockout =
            |e: &Event| matches!(e, Event::Say(s) if s.contains("same lockout as wrong passwords"));
        assert!(
            lockout(&prompt.events[1]),
            "the first failure must name the lockout: {:?}",
            prompt.events
        );
        assert_eq!(prompt.events.iter().filter(|e| lockout(e)).count(), 1);
        match &out {
            Err(CliError::SecondFactor(m)) => assert!(m.contains("3 codes"), "{m}"),
            o => panic!("expected a stop after three codes, got {o:?}"),
        }
        assert_pending_never_surfaces(&out, &prompt);
    }

    #[tokio::test]
    async fn expired_pending_sign_in_says_run_login_again() {
        let mut server = mockito::Server::new_async().await;
        // expect(1) on the login: the password is not silently re-sent.
        let login = mfa_login(&mut server).await;
        let _verify = verify(
            &mut server,
            serde_json::json!({ "code": "123456" }),
            401,
            r#"{"detail":{"error":"mfa_session_expired"}}"#,
            1,
        )
        .await;
        let mut prompt = Scripted::tty(&["123456"]);
        let out = sign_in(&server.url(), "a@b", "pw", &mut prompt).await;
        login.assert_async().await;
        assert_eq!(prompt.asked(), 1);
        match &out {
            Err(CliError::SecondFactor(m)) => {
                assert!(m.contains("10-minute"), "{m}");
                assert!(m.contains("quorum auth login"), "{m}");
            }
            o => panic!("expected the expiry message, got {o:?}"),
        }
        assert_pending_never_surfaces(&out, &prompt);
    }

    #[tokio::test]
    async fn rate_limit_reports_and_stops_without_waiting() {
        let mut server = mockito::Server::new_async().await;
        let _login = mfa_login(&mut server).await;
        let limited = server
            .mock("POST", "/api/v1/auth/mfa/verify")
            .with_status(429)
            .with_header("retry-after", "3600")
            .with_body(r#"{"detail":{"error":"rate_limited"}}"#)
            .expect(1)
            .create_async()
            .await;
        let mut prompt = Scripted::tty(&["123456", "654321"]);
        let started = std::time::Instant::now();
        let out = sign_in(&server.url(), "a@b", "pw", &mut prompt).await;
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "waited {:?}",
            started.elapsed()
        );
        limited.assert_async().await;
        assert_eq!(prompt.asked(), 1);
        match &out {
            Err(CliError::SecondFactor(m)) => assert!(m.contains("429"), "{m}"),
            o => panic!("expected the rate-limit message, got {o:?}"),
        }
        assert_pending_never_surfaces(&out, &prompt);
    }

    #[tokio::test]
    async fn no_terminal_refuses_without_prompting() {
        let mut server = mockito::Server::new_async().await;
        let _login = mfa_login(&mut server).await;
        let never = server
            .mock("POST", "/api/v1/auth/mfa/verify")
            .expect(0)
            .create_async()
            .await;
        let mut prompt = Scripted::no_tty();
        let out = sign_in(&server.url(), "a@b", "pw", &mut prompt).await;
        never.assert_async().await;
        assert!(prompt.events.is_empty(), "{:?}", prompt.events);
        // Commit 1's message: the caller maps this through login_error.
        assert!(matches!(&out, Ok(Err(AuthError::MfaRequired))), "{out:?}");
        assert_pending_never_surfaces(&out, &prompt);
    }

    #[tokio::test]
    async fn an_empty_answer_sends_nothing() {
        let mut server = mockito::Server::new_async().await;
        let _login = mfa_login(&mut server).await;
        let never = server
            .mock("POST", "/api/v1/auth/mfa/verify")
            .expect(0)
            .create_async()
            .await;
        let mut prompt = Scripted::tty(&["   "]);
        let out = sign_in(&server.url(), "a@b", "pw", &mut prompt).await;
        never.assert_async().await;
        assert!(
            matches!(&out, Err(CliError::SecondFactor(m)) if m.contains("No code entered")),
            "{out:?}"
        );
        assert_pending_never_surfaces(&out, &prompt);
    }

    #[tokio::test]
    async fn verify_ok_without_a_session_cookie_keeps_nothing() {
        // The jar still holds the pending cookie here; it must not be used
        // as a stand-in for the missing real one.
        let mut server = mockito::Server::new_async().await;
        let _login = mfa_login(&mut server).await;
        let _verify = server
            .mock("POST", "/api/v1/auth/mfa/verify")
            .match_header("cookie", Matcher::Regex(format!("session={PENDING}")))
            .with_status(200)
            .with_body(verified(10))
            .create_async()
            .await;
        let mut prompt = Scripted::tty(&["123456"]);
        let out = sign_in(&server.url(), "a@b", "pw", &mut prompt).await;
        assert!(
            matches!(&out, Err(CliError::LoginRejected(m)) if m.contains("no session cookie")),
            "{out:?}"
        );
        assert_pending_never_surfaces(&out, &prompt);
    }

    #[tokio::test]
    async fn other_verify_refusal_keeps_nothing() {
        let mut server = mockito::Server::new_async().await;
        let _login = mfa_login(&mut server).await;
        let _verify = server
            .mock("POST", "/api/v1/auth/mfa/verify")
            .with_status(403)
            .with_body(r#"{"error":"csrf_origin_rejected"}"#)
            .create_async()
            .await;
        let mut prompt = Scripted::tty(&["123456"]);
        let out = sign_in(&server.url(), "a@b", "pw", &mut prompt).await;
        assert!(
            matches!(&out, Err(CliError::SecondFactor(m)) if m.contains("403")),
            "{out:?}"
        );
        assert_pending_never_surfaces(&out, &prompt);
    }

    #[tokio::test]
    async fn account_without_two_factor_is_unchanged() {
        let mut server = mockito::Server::new_async().await;
        let _login = server
            .mock("POST", "/api/v1/auth/login")
            .with_status(200)
            .with_header("set-cookie", "session=plain-login; path=/; httponly")
            .with_body("{}")
            .create_async()
            .await;
        let mut prompt = Scripted::tty(&[]);
        let out = sign_in(&server.url(), "a@b", "pw", &mut prompt).await;
        assert!(
            matches!(&out, Ok(Ok(s)) if s.expose() == "plain-login"),
            "{out:?}"
        );
        assert!(prompt.events.is_empty());
    }

    #[tokio::test]
    async fn other_login_refusals_pass_through_without_prompting() {
        let mut server = mockito::Server::new_async().await;
        let _login = server
            .mock("POST", "/api/v1/auth/login")
            .with_status(403)
            .with_body(r#"{"detail":{"error":"email_not_verified"}}"#)
            .create_async()
            .await;
        let mut prompt = Scripted::tty(&[]);
        let out = sign_in(&server.url(), "a@b", "pw", &mut prompt).await;
        assert!(
            matches!(&out, Ok(Err(AuthError::EmailNotVerified))),
            "{out:?}"
        );
        assert!(prompt.events.is_empty());
    }
}
