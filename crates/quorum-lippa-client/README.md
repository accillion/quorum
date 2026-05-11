# quorum-lippa-client

Cookie-authenticated client for Lippa's Consensus API
(`/api/v1/auth/login`, `/api/v1/consensus/sessions/*`,
`/api/v1/me`). Async via tokio's `current_thread` runtime.

Public surface:

- `Secret` — cookie-value newtype with redacting `Debug`/`Display`.
- `AuthMethod::Cookie(Secret)` / `AuthMethod::Bearer(Secret)` — only
  Cookie is wired in Phase 1B; Bearer is constructible but `apply`
  returns `BearerNotYetSupported` until Lippa's M-ExternalAuth lands.
- `login_with_cookie(LoginRequest) -> Secret` — JSON login that
  extracts the `session=...` cookie from the response.
- `LippaClient::new(base_url, AuthMethod)` — seeds a
  `reqwest::cookie::Jar` with the session and uses
  `cookie_provider` for outbound carriage and renewal capture.
- `create_session` / `poll_status` / `fetch_detail` / `whoami` /
  `logout_server_side` — the Consensus surface.
- `Storage` (OS keyring or `--no-keyring` file fallback) for cookie
  persistence between invocations.

No dependency on `quorum-core`. Wire schemas are private to this
crate except for `SessionStatus`, exposed because the polling loop
branches on it.

For the user-facing `quorum` CLI, see
[`quorum-cli`](https://crates.io/crates/quorum-cli) and the
[repository root README](https://github.com/accillion/quorum#readme).

License: Apache-2.0.
