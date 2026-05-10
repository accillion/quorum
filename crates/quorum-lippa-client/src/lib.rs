//! Quorum's Lippa API client. Cookie-authenticated, async via tokio's
//! `current_thread` runtime.
//!
//! Public surface:
//!   - `Secret`             — newtype around cookie value with redacted Debug/Display.
//!   - `AuthMethod`         — Cookie(Secret) | Bearer(Secret); only Cookie is wired.
//!   - `AuthError`          — typed login/cookie failure modes.
//!   - `LoginRequest`       — email/password login input.
//!   - `login_with_cookie`  — async POST /api/v1/auth/login.
//!   - `LippaClient`        — Consensus session create/poll/fetch.
//!   - `keyring::Storage`   — OS keychain | `--no-keyring` file fallback.
//!
//! No dependency on `quorum-core`. Wire schemas (`SessionStatus`,
//! `SessionDetail`) are private to `wire.rs` except for `SessionStatus`,
//! which is publicly exported because the polling loop branches on it.

pub mod auth;
pub mod client;
pub mod consensus;
pub mod keyring;
pub mod secret;
mod wire;

pub use auth::{extract_session_cookie, login_with_cookie, AuthError, AuthMethod, LoginRequest};
pub use client::{ClientError, LippaClient, SessionId};
pub use consensus::{SessionCreateRequest, SessionStatus};
pub use keyring::{KeyringError, Storage};
pub use secret::Secret;
