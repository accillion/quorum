//! `quorum auth login/logout/status` + the Phase 1B non-interactive
//! variants (`login --non-interactive`, `status --show-session [-y]`).

use crate::exit::CliError;
use quorum_lippa_client::keyring::{delete_cookie, load_cookie, store_cookie, Storage};
use quorum_lippa_client::{
    login_with_cookie, AuthError, AuthMethod, ClientError, LippaClient, LoginRequest, Secret,
};

/// `quorum auth login --non-interactive`.
///
/// Reads `QUORUM_LIPPA_EMAIL` + `QUORUM_LIPPA_PASSWORD` from the
/// environment. Both values are wrapped in [`Secret`] at the read
/// boundary; the password is explicitly dropped immediately after
/// [`login_with_cookie`] returns so no in-memory mirror lives beyond
/// the login call site (spec §4.6.1, AC 128/129).
///
/// Exit codes (mapped by [`crate::exit::CliError::exit`]):
/// * `Ok(())`            on success — cookie persisted per storage policy.
/// * `CliError::Config(...)` (exit 2) when either env var is missing.
/// * `CliError::LoginRejected(...)` (exit 3) on Lippa rejection.
pub async fn login_non_interactive(base_url: &str, storage: &Storage) -> Result<(), CliError> {
    if matches!(storage, Storage::File(_)) {
        eprintln!(
            "WARNING: --no-keyring stores the session cookie in plaintext; only use this in trusted environments."
        );
    }
    let email = require_env("QUORUM_LIPPA_EMAIL")?;
    // Wrap password in Secret immediately; only `expose()` once when
    // passing into `login_with_cookie`. The `drop(password)` after the
    // login call is the explicit lifetime cap (P36 / AC 128).
    let password = Secret::new(require_env("QUORUM_LIPPA_PASSWORD")?);
    let result = login_with_cookie(LoginRequest {
        base_url,
        email: &email,
        password: password.expose(),
    })
    .await;
    drop(password);
    match result {
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

/// `quorum auth status --show-session [-y]`.
///
/// Prints the raw cookie value to **stdout** on a line by itself
/// (no labels, no host — easy to pipe to `gh secret set`,
/// `op item create`, etc.) AFTER a yes/no confirm prompt on stderr.
/// `-y` skips the confirm. Non-TTY without `-y` exits 2 because there
/// is no safe way to gate the secret leak.
///
/// Exit codes:
/// * `Ok(())`                    success (cookie printed).
/// * `CliError::Config(...)`     (exit 2) when no auth available, non-TTY without `-y`, or user declined.
pub fn status_show_session(
    base_url: &str,
    storage: &Storage,
    is_tty: bool,
    yes: bool,
) -> Result<(), CliError> {
    let cookie = match load_cookie(storage, base_url) {
        Ok(Some(c)) => c,
        Ok(None) => {
            return Err(CliError::Config(format!(
                "no session stored for {base_url}; run `quorum auth login` first"
            )));
        }
        Err(e) => return Err(CliError::Keyring(e.to_string())),
    };
    if !yes {
        if !is_tty {
            return Err(CliError::Config(
                "--show-session in a non-TTY context requires `-y` to acknowledge the secret leak"
                    .into(),
            ));
        }
        eprint!("About to print the {base_url} session cookie to stdout. Continue? [y/N] ");
        use std::io::Write;
        std::io::stderr().flush().ok();
        let mut buf = String::new();
        std::io::stdin()
            .read_line(&mut buf)
            .map_err(|e| CliError::Io(e.to_string()))?;
        let answer = buf.trim().to_ascii_lowercase();
        if answer != "y" && answer != "yes" {
            return Err(CliError::Config("cancelled".into()));
        }
    }
    // Single line, just the cookie value. No trailing newline format
    // changes — `println!` is the right shape so a piped consumer can
    // `read -r SESSION`.
    println!("{}", cookie.expose());
    Ok(())
}

fn require_env(name: &str) -> Result<String, CliError> {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => Ok(v),
        Ok(_) => Err(CliError::Config(format!(
            "missing required env var: {name} (empty value)"
        ))),
        Err(_) => Err(CliError::Config(format!(
            "missing required env var: {name}"
        ))),
    }
}

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
