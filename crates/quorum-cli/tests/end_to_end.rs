//! End-to-end LippaClient flow against a mock server.
//!
//! Exercises the create-session → poll → fetch-detail chain, the terminal-
//! state taxonomy (failed/cancelled/error), the create-401 vs poll-401
//! distinction, and `Retry-After` precedence in `next_poll_delay`.

use quorum_lippa_client::{
    client::{next_poll_delay, PollOptions},
    AuthMethod, ClientError, LippaClient, Secret, SessionCreateRequest, SessionId, SessionStatus,
};
use std::time::Duration;

fn cookie_client(url: String) -> LippaClient {
    LippaClient::new(url, AuthMethod::Cookie(Secret::new("mock".into()))).unwrap()
}

#[tokio::test]
async fn create_session_happy_path_returns_id() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("POST", "/api/v1/consensus/sessions")
        .with_status(201)
        .with_body(
            r#"{"id":"sess_001","status":"pending","credits_reserved":3,"credits_remaining":197}"#,
        )
        .create_async()
        .await;
    let client = cookie_client(server.url());
    let id = client
        .create_session(SessionCreateRequest {
            prompt: "x".repeat(40),
            project_id: None,
            debate_mode: "standard".into(),
            idempotency_key: None,
        })
        .await
        .expect("create succeeds");
    assert_eq!(id.as_str(), "sess_001");
}

#[tokio::test]
async fn create_401_maps_to_login_required() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("POST", "/api/v1/consensus/sessions")
        .with_status(401)
        .with_body(r#"{"detail":"unauthorized"}"#)
        .create_async()
        .await;
    let client = cookie_client(server.url());
    let err = client
        .create_session(SessionCreateRequest {
            prompt: "x".repeat(40),
            project_id: None,
            debate_mode: "standard".into(),
            idempotency_key: None,
        })
        .await
        .expect_err("401 → LoginRequired");
    assert!(matches!(
        err,
        ClientError::Auth(quorum_lippa_client::AuthError::LoginRequired(_))
    ));
}

#[tokio::test]
async fn poll_status_converged_and_terminal_taxonomy() {
    let mut server = mockito::Server::new_async().await;
    let _conv = server
        .mock("GET", "/api/v1/consensus/sessions/sess/status")
        .with_status(200)
        .with_body(r#"{"status":"converged","rounds_completed":3,"rounds_total":3,"models_active":3,"models_dropped":0,"elapsed_seconds":42.0,"follow_up_questions":null}"#)
        .expect(1)
        .create_async()
        .await;
    let client = cookie_client(server.url());
    let (status, _) = client.poll_status(&SessionId("sess".into())).await.unwrap();
    assert_eq!(status, SessionStatus::Converged);

    // Failed mapping
    let mut s2 = mockito::Server::new_async().await;
    let _m = s2
        .mock("GET", "/api/v1/consensus/sessions/sess/status")
        .with_status(200)
        .with_body(r#"{"status":"failed","rounds_completed":0,"rounds_total":3,"models_active":3,"models_dropped":0,"elapsed_seconds":5.2,"follow_up_questions":null}"#)
        .create_async()
        .await;
    let c2 = cookie_client(s2.url());
    let (status, _) = c2.poll_status(&SessionId("sess".into())).await.unwrap();
    assert!(matches!(status, SessionStatus::Failed(_)));
}

#[tokio::test]
async fn poll_401_maps_to_unauthorized_status() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/api/v1/consensus/sessions/sess/status")
        .with_status(401)
        .with_body("")
        .create_async()
        .await;
    let client = cookie_client(server.url());
    let (status, _) = client.poll_status(&SessionId("sess".into())).await.unwrap();
    assert_eq!(status, SessionStatus::Unauthorized);
}

#[tokio::test]
async fn poll_429_with_retry_after_honored_in_delay_computation() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/api/v1/consensus/sessions/sess/status")
        .with_status(429)
        .with_header("Retry-After", "13")
        .with_body("")
        .create_async()
        .await;
    let client = cookie_client(server.url());
    let (status, hints) = client.poll_status(&SessionId("sess".into())).await.unwrap();
    assert_eq!(status, SessionStatus::InProgress);
    assert_eq!(hints.retry_after, Some(Duration::from_secs(13)));
    let opts = PollOptions::default();
    let delay = next_poll_delay(opts.initial_delay, opts.max_delay, hints.retry_after, 0.5);
    assert_eq!(delay, Duration::from_secs(13));
}

#[tokio::test]
async fn fetch_detail_returns_raw_json_value() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/api/v1/consensus/sessions/sess")
        .with_status(200)
        .with_body(r#"{"id":"sess","status":"converged","summary_text":"x","agreement":[],"divergence":[],"assumptions":[],"models":[]}"#)
        .create_async()
        .await;
    let client = cookie_client(server.url());
    let v = client
        .fetch_detail(&SessionId("sess".into()))
        .await
        .unwrap();
    assert_eq!(v.get("id").and_then(|x| x.as_str()), Some("sess"));
}
