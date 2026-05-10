//! Auth-flow round trip against a mock Lippa server.
//!
//! Login → store-cookie → whoami → logout → load-cookie. Verifies cookie
//! capture, keyring persistence (file-backed for hermetic test), and the
//! whoami ping (LippaClient::whoami).

use quorum_lippa_client::{
    keyring::{load_cookie, store_cookie, Storage},
    login_with_cookie, AuthMethod, LippaClient, LoginRequest, Secret,
};
use tempfile::tempdir;

#[tokio::test]
async fn login_captures_cookie_and_persists_to_file_storage() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("POST", "/api/v1/auth/login")
        .with_status(200)
        .with_header("Set-Cookie", "session=mock-cookie-value; Path=/; HttpOnly")
        .with_body(
            r#"{"id":"u","email":"a@b","display_name":"A","role":"member","workspace_id":"w"}"#,
        )
        .create_async()
        .await;
    let url = server.url();
    let secret = login_with_cookie(LoginRequest {
        base_url: &url,
        email: "a@b",
        password: "x",
    })
    .await
    .expect("login succeeds");
    assert_eq!(secret.expose(), "mock-cookie-value");

    let dir = tempdir().unwrap();
    let storage = Storage::File(dir.path().to_path_buf());
    store_cookie(&storage, &url, &secret).unwrap();
    let got = load_cookie(&storage, &url).unwrap().unwrap();
    assert_eq!(got.expose(), "mock-cookie-value");
}

#[tokio::test]
async fn login_failure_returns_typed_error() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("POST", "/api/v1/auth/login")
        .with_status(401)
        .with_body(r#"{"detail":"Invalid email or password"}"#)
        .create_async()
        .await;
    let url = server.url();
    let err = login_with_cookie(LoginRequest {
        base_url: &url,
        email: "a@b",
        password: "wrong",
    })
    .await
    .expect_err("401 -> AuthError::LoginRejected");
    assert!(matches!(
        err,
        quorum_lippa_client::AuthError::LoginRejected(_)
    ));
}

#[tokio::test]
async fn whoami_distinguishes_stale_from_valid() {
    let mut server = mockito::Server::new_async().await;
    let _ok = server
        .mock("GET", "/api/v1/me")
        .match_header("cookie", "session=valid")
        .with_status(200)
        .with_body(
            r#"{"id":"u","email":"a@b","display_name":"A","role":"member","workspace_id":"w"}"#,
        )
        .create_async()
        .await;
    let _stale = server
        .mock("GET", "/api/v1/me")
        .match_header("cookie", "session=stale")
        .with_status(401)
        .with_body(r#"{"detail":"unauthorized"}"#)
        .create_async()
        .await;

    let url = server.url();
    let valid_client =
        LippaClient::new(url.clone(), AuthMethod::Cookie(Secret::new("valid".into()))).unwrap();
    let body = valid_client.whoami().await.expect("200 path");
    assert_eq!(body.get("email").and_then(|v| v.as_str()), Some("a@b"));

    let stale_client =
        LippaClient::new(url, AuthMethod::Cookie(Secret::new("stale".into()))).unwrap();
    let err = stale_client.whoami().await.expect_err("401 path");
    assert!(matches!(
        err,
        quorum_lippa_client::ClientError::Auth(quorum_lippa_client::AuthError::LoginRequired(_))
    ));
}
