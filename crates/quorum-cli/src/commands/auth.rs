//! `quorum auth login/logout/status`.

use crate::exit::CliError;
use quorum_lippa_client::keyring::{delete_cookie, load_cookie, store_cookie, Storage};
use quorum_lippa_client::{
    login_with_cookie, AuthError, AuthMethod, ClientError, LippaClient, LoginRequest,
};

pub async fn login(base_url: &str, storage: &Storage, interactive: bool) -> Result<(), CliError> {
    if !interactive {
        return Err(CliError::NoTtyForInteractive);
    }

    if matches!(storage, Storage::File(_)) {
        eprintln!(
            "WARNING: --no-keyring stores the session cookie in plaintext; only use this in trusted environments."
        );
    }

    eprint!("Email: ");
    use std::io::Write;
    std::io::stderr().flush().ok();
    let mut email = String::new();
    std::io::stdin()
        .read_line(&mut email)
        .map_err(|e| CliError::Io(e.to_string()))?;
    let email = email.trim().to_string();

    let password =
        rpassword::prompt_password("Password: ").map_err(|e| CliError::Io(e.to_string()))?;

    match login_with_cookie(LoginRequest {
        base_url,
        email: &email,
        password: &password,
    })
    .await
    {
        Ok(secret) => {
            store_cookie(storage, base_url, &secret)
                .map_err(|e| CliError::Keyring(e.to_string()))?;
            println!("Logged in to {base_url} as {email}");
            Ok(())
        }
        Err(AuthError::LoginRejected(s)) => Err(CliError::LoginRejected(s.to_string())),
        Err(AuthError::NoSessionCookie) => Err(CliError::LoginRejected(
            "server returned 200 but no session cookie".into(),
        )),
        Err(AuthError::CookieParseError(m)) => Err(CliError::LoginRejected(m)),
        Err(AuthError::Transport(m)) => Err(CliError::Network(m)),
        Err(other) => Err(CliError::LoginRejected(format!("{other}"))),
    }
}

pub async fn logout(base_url: &str, storage: &Storage) -> Result<(), CliError> {
    let pre_existed = load_cookie(storage, base_url)
        .map_err(|e| CliError::Keyring(e.to_string()))?
        .is_some();
    if !pre_existed {
        println!("Already logged out");
        return Ok(());
    }
    // Best-effort server-side call before deleting locally.
    if let Some(cookie) =
        load_cookie(storage, base_url).map_err(|e| CliError::Keyring(e.to_string()))?
    {
        if let Ok(client) = LippaClient::new(base_url.into(), AuthMethod::Cookie(cookie)) {
            if client.logout_server_side().await.is_err() {
                eprintln!("note: server-side logout call failed; local session deleted but server may consider the session active until expiry.");
            }
        }
    }
    delete_cookie(storage, base_url).map_err(|e| CliError::Keyring(e.to_string()))?;
    println!("Logged out of {base_url}");
    Ok(())
}

pub async fn status(base_url: &str, storage: &Storage) -> Result<(), CliError> {
    let cookie = match load_cookie(storage, base_url) {
        Ok(Some(c)) => c,
        Ok(None) => {
            println!("Not logged in");
            return Ok(());
        }
        Err(e) => return Err(CliError::Keyring(e.to_string())),
    };

    let client = LippaClient::new(base_url.into(), AuthMethod::Cookie(cookie))
        .map_err(|e| CliError::Network(e.to_string()))?;
    match client.whoami().await {
        Ok(body) => {
            let email = body
                .get("email")
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown>");
            println!("Logged in to {base_url} as {email}");
            Ok(())
        }
        Err(ClientError::Auth(AuthError::LoginRequired(_))) => {
            eprintln!("Logged in to {base_url}, but session is stale — run `quorum auth login`.");
            Err(CliError::NotAuthenticated)
        }
        Err(ClientError::Transport(m)) => Err(CliError::Network(m)),
        Err(e) => Err(CliError::Network(e.to_string())),
    }
}
