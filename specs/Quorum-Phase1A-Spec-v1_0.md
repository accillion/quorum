# Quorum — Phase 1A: Walking-skeleton CLI review against Lippa Consensus

**Status:** v1.0 — production-ready, post peer-review + re-review.
**Spec home:** `specs/Quorum-Phase1A-Spec-v1_0.md`
**Estimated CC work:** 4-6 days (single CC session, possibly two; revised up from v0.1's 3-5 day estimate to reflect added scope around bundle deny-list, cookie lifecycle, headless keyring fallback, and expanded test fixtures). Re-review noted a realistic estimate is closer to 6-8 days given the substantive v0.2 additions; flagged for calendar planning, not a spec change.
**Driver:** Quorum (this repo). External client of Lippa's `/api/v1/*` surface.
**References:** `Quorum-Phase1A-Drafting-Handoff.md`; `Quorum-Recon-v0_findings.md` (verdict GO-PAT-PARTIAL); `M-ExternalAuth-Phase1-Spec-v1.md` (Lippa-side milestone, committed but not yet implemented); `Quorum-Phase1A-v0_1-adjudication.md` (adjudication of v0.1 peer reviews; full v0.1 → v0.2 changelog history); `Quorum-Phase1A-v0_2-rereview.md` (Lippa Doc Reviewer single-reviewer re-review, ACCEPT WITH MINOR PATCH).

**Changelog v0.2 → v1.0** (8 inline patches from re-review):

1. **Crate-boundary fix, take two** (§3, §4.2.4, §4.3.1, criterion 36): v0.2's `SessionDetailLike` trait pattern reintroduced a hidden coupling — Rust coherence requires the trait `impl` to live in either the trait's crate or the type's crate, and the impl needed to import the trait from `quorum-core`, forcing `quorum-lippa-client → quorum-core` dependency. v1.0 drops the trait entirely. `LippaClient::fetch_detail` returns `serde_json::Value`. `quorum-core::review_from_json(&Value, &RepoMetadata) -> Result<Review, ParseError>` is the only mapping site, called from `quorum-cli`. Both libraries remain independent; `cargo tree` verifies. (Resolves Critical #1 from re-review.)
2. **`extract_session_cookie` returns `Result<Secret, AuthError>` directly** (§4.2.2). Wrapping at the parse boundary eliminates the brief plaintext `String` lifetime between extraction and `Secret::new`. (Resolves Important #1.)
3. **`parking_lot::Mutex` replaced with `std::sync::Mutex`** (§4.0, §4.2.4). Current-thread tokio runtime doesn't need the third-party lock; `std::sync::Mutex` keeps the dependency footprint smaller. (Resolves Important #2.)
4. **`Secret` derives `Clone`** (§4.2.3). Required for `renewed_cookie() -> Option<Secret>` to return without moving from under the mutex. Cloning a `String` newtype is cheap and doesn't weaken redaction. (Resolves Important #6.)
5. **Deny-list glob semantics specified** (§4.3.2). Patterns are basename-globbed at any depth: `.env` matches `**/.env` (catches `infra/staging/.env`); `secrets.yml` matches `**/secrets.yml` (catches `services/api/secrets.yml`). Test fixtures cover nested paths. (Resolves Important #3.)
6. **Cookie persistence wording fixed** (§4.2.4, §4.7). "Persisted to the active `Storage`" replaces "persisted to keyring" — applies under both OS keyring and `--no-keyring` file paths. New test fixture for `--no-keyring` cookie renewal. (Resolves Important #4.)
7. **`*.key` false-positive class documented** (§4.3.2). License-key files in some Java/.NET projects may match the pattern; defense-in-depth trade-off, called out so users don't hit confusion. `RawFinding` reference removed (no longer needed after Critical #1 fix; resolves Important #5 by elimination).
8. Editorial: status updated to v1.0; references list updated to include re-review document; `:` → `-` filename substitution explicitly noted as a Windows-filesystem-compat change in §4.3.3 (not a format change in the JSON itself).

---

## 1. Mission

> Ship the smallest possible end-to-end loop: developer runs `quorum review` in a git repo with staged changes, Quorum bundles the diff plus context, submits it to Lippa's Consensus API as a single-model session, polls until convergence, and renders findings to stdout. Plus a JSON archive on disk. Nothing more.

Phase 1A is a minimal end-to-end loop with the project-mandated three-crate structure from `CLAUDE.md`. The ~1,700 LOC production footprint reflects greenfield Rust across three crates plus a deny-list, an OS-keychain abstraction with a headless fallback, and a cookie-lifecycle handler — not a bloated walking skeleton. The scope is brutal in feature surface (no TUI, no interactivity, no multi-model fan-out, no memory writes, no hooks); the line count is honest about what a Rust CLI with three boundary-clean crates costs.

Auth is the load-bearing temporary detail: cookie-only in Phase 1A, with a `Bearer` enum branch returning a typed `AuthError::BearerNotYetSupported` and ready to swap when Lippa's `M-ExternalAuth Phase 1` milestone lands and its cluster flag flips on.

---

## 2. Non-Goals

- **No TUI.** Plain stdout markdown. `ratatui` is not pulled in.
- **No interactive accept/dismiss/edit.** Findings render and the binary exits. Phase 1B.
- **No local dismissal store.** No SQLite database written by Phase 1A. The JSON archive under `.quorum/reviews/` is the only persistent review artifact.
- **No git hook installer.** No `quorum install --hook=pre-push`. Manual `quorum review` only. Phase 1B.
- **No multi-model Consensus.** `model_roles` carries exactly one entry. Phase 1C.
- **No memory writes back to Lippa.** The Consensus session call is read-only in effect; memory propose endpoints are not invoked. Phase 2+.
- **No privacy-mode redaction.** Submitted bundles contain raw diff and raw context, with one important caveat: a small **deny-list of secret-bearing files** (`.env*`, `*.pem`, `id_rsa`, `.netrc`, etc.) is excluded from the bundle. The deny-list is defense-in-depth, not a security guarantee — Phase 2 redaction is the canonical solution. Out-of-scope: regex/AST-based redaction of file *contents*, customizable deny-lists, local-LLM validation passes.
- **No conventions-write-back.** `.quorum/conventions.md` feeds INTO reviews. It is never written FROM reviews. Phase 1B.
- **No cost-tier routing.** Single hardcoded model role; no small/medium/large diff classification.
- **No GitHub PR commenting, no IDE plugin, no standalone BYO-keys mode.** v2+.
- **No Bearer-token auth path implemented.** The `AuthMethod::Bearer` arm of `apply` returns `Err(AuthError::BearerNotYetSupported)`. The variant exists, can be constructed, and panics nowhere — but no code path produces it. Phase 1A ships against cookie auth.
- **No 2FA flow handling.** If recon (CC-Recon-1) reveals Lippa requires 2FA at login, Phase 1A surfaces a clear error and defers 2FA support to Phase 1B.
- **No CI / non-interactive auth.** `quorum auth login` requires a TTY. Env-var-based credentials and `--non-interactive` flow are deferred to Phase 1B alongside hook integration, where the threat model gets explicit treatment.

---

## 3. Architecture

```
                  ┌──────────────────────────────────┐
                  │         quorum-cli (bin)         │
                  │  ─ clap arg parsing              │
                  │  ─ stdout rendering              │
                  │  ─ exit-code mapping             │
                  │  ─ wire→Review mapping site      │
                  └────────────┬─────────────────────┘
                               │
              ┌────────────────┴──────────────────┐
              │                                   │
              ▼                                   ▼
   ┌──────────────────────┐         ┌─────────────────────────────┐
   │   quorum-core (lib)  │         │ quorum-lippa-client (lib)   │
   │  ─ Review/Finding    │         │  ─ AuthMethod + AuthError   │
   │  ─ bundle assembly   │         │  ─ Secret newtype           │
   │  ─ git2 staged diff  │         │  ─ session login (cookie)   │
   │  ─ deny-list filter  │         │  ─ Consensus create/poll    │
   │  ─ conventions read  │         │  ─ SessionDetail wire type  │
   │  ─ markdown render   │         │  ─ keyring + fallback       │
   │  ─ JSON archive      │         └─────────────────────────────┘
   │  ─ review_from_json()
   └──────────────────────┘
```

**Crate boundary rules** (per `CLAUDE.md`): `quorum-cli` depends on both libraries. `quorum-core` and `quorum-lippa-client` do NOT depend on each other. Cross-crate calls go through public APIs only. The `Review` mapping is a `quorum-core::review_from_json(value: &serde_json::Value, repo: &RepoMetadata) -> Result<Review, ParseError>` free function — `quorum-cli` calls it after `quorum-lippa-client::LippaClient::fetch_detail()` returns a `serde_json::Value`. The wire types `SessionDetail`/`SessionStatus`/`RoundData` exist privately inside `quorum-lippa-client::wire` for in-client parsing (e.g. `poll_status` distinguishes terminal states from in-progress); they are never exposed across the boundary. Both libraries already have a `serde_json` dependency (the client for HTTP, the core for archive serialization), so no new dependency is introduced. Verifiable via `cargo tree` — neither library appears in the other's dependency tree.

**Architectural principles** (governing every section below):

- **Tracked vs untracked input distinction.** `.quorum/conventions.md` is review *input* and must be tracked in git (uncommitted changes ignored). `.quorum/config.toml` is *local configuration* and may or may not be tracked — both states are valid. The CLI never blocks on missing config; it produces a clear error directing the user to `quorum link`.
- **Repo content is data, never instructions.** All bundle content is wrapped in delimiters. No part of the CLI ever interprets repo text as a directive.
- **Cookie auth is temporary infrastructure.** Every cookie-related design decision is documented in `§4.7 Cookie lifecycle` so the eventual Bearer swap touches one section, not many.

**`quorum review` flow:**

```
  config.toml? ──► no ──► exit 2 (run `quorum link`)
  keyring?     ──► no ──► exit 3 (run `quorum auth login`)
       │
       ▼
  build AuthMethod::Cookie(Secret) → assemble bundle (diff + conventions + repo meta)
       │                                         │
       │                                         └─► deny-list filter, per-section budget
       ▼
  POST /consensus/sessions ──► 401 ──► exit 3 ("Run quorum auth login" — create-401)
       │
       ▼
  poll /sessions/{id}/status (1s → 8s backoff w/ ±25% jitter, Retry-After respected,
                              5min timeout, terminal-state taxonomy)
       │            ├─ 401 mid-poll ──► exit 3 ("Session ended mid-review — poll-401")
       │            ├─ failed/cancelled ──► exit 2 with detail
       │            └─ converged ──► continue
       ▼
  GET /sessions/{id} → serde_json::Value
       │
       ▼
  quorum-core::review_from_json(value, repo_meta) → Result<Review, ParseError>
       │
       ▼
  render markdown to stdout
       │
       ▼
  capture renewed Set-Cookie if any → in-memory; persist to keyring at clean exit if changed
       │
       ▼
  serialize Review once → buffer → write to .quorum/reviews/<ISO>.json AND (if --json) stdout
       │
       ▼
  exit 0|1 (severity-mapped)
```

---

## 4. Implementation Plan

### 4.0 Async runtime

`quorum-cli` uses `tokio` with features `["rt", "macros"]` only — no multi-threaded scheduler. Entry point is `#[tokio::main(flavor = "current_thread")]`. The reasoning: the workload is one user, one HTTP client, one polling loop at a time — `current_thread` halves binary size vs `rt-multi-thread` and there's no parallelism to exploit.

**Crate sync/async split:**
- `quorum-core` is fully synchronous. `git2`, file I/O, and JSON serialization are sync; `quorum-core` does not depend on `tokio`.
- `quorum-lippa-client` is async (HTTP client).
- `quorum-cli` command handlers are async at the top level and call into `quorum-core` synchronously and `quorum-lippa-client` with `.await`. Long sync work (git diff extraction) runs inline; if it ever becomes meaningfully blocking, `tokio::task::spawn_blocking` can wrap it without restructure.

This split is enforced by `quorum-core/Cargo.toml` having no `tokio` dependency. Reviewable by `cargo tree`.

**Synchronization primitive choice:** under the `current_thread` runtime, `quorum-lippa-client::LippaClient` uses `std::sync::Mutex` (not `parking_lot::Mutex`) for the in-memory cookie mirror (§4.2.4). One task at a time means contention is impossible by construction; `std::sync` is sufficient and avoids pulling in `parking_lot` for marginal benefit on a CLI binary's size budget.

### 4.1 Cargo workspace layout

```
quorum/
  Cargo.toml                         # workspace root
  Cargo.lock
  crates/
    quorum-cli/
      Cargo.toml
      src/
        main.rs                      # entry, clap, command dispatch
        commands/
          auth.rs                    # login, logout, status (with whoami ping)
          review.rs                  # the review pipeline driver
          link.rs                    # repo→project binding
        render.rs                    # markdown rendering
        exit.rs                      # ExitCode mapping
    quorum-core/
      Cargo.toml
      src/
        lib.rs
        review.rs                    # Review, Finding, Severity, review_from_json
        bundle.rs                    # context-bundle assembly
        deny_list.rs                 # secret-bearing file exclusion
        git.rs                       # staged-diff via diff_tree_to_index
        conventions.rs               # .quorum/conventions.md (committed-only)
        config.rs                    # .quorum/config.toml read
        archive.rs                   # JSON archive writer (deterministic)
        discovery.rs                 # CLAUDE.md / AGENTS.md / .cursorrules
    quorum-lippa-client/
      Cargo.toml
      src/
        lib.rs
        auth.rs                      # AuthMethod, AuthError, login flow
        secret.rs                    # Secret newtype
        client.rs                    # HTTP client, header injection, lifecycle
        consensus.rs                 # session create/poll/fetch
        wire.rs                      # serde structs (SessionDetail, etc.)
        keyring.rs                   # OS-keychain + --no-keyring fallback
  specs/
    Quorum-Phase1A-Spec-v0_2.md
  tests/
    fixtures/                        # golden-file diffs + expected outputs
  CLAUDE.md
  README.md
  LICENSE
  SERVICES.md                        # NEW — first service contracts land here
  HISTORY.md                         # NEW — first milestone close entry
```

### 4.2 `quorum-lippa-client`

#### 4.2.1 `AuthMethod` and `AuthError`

```rust
// crates/quorum-lippa-client/src/auth.rs

use crate::secret::Secret;

/// How a request authenticates to Lippa's /api/v1/* surface.
///
/// Phase 1A implements only `Cookie`. The `Bearer` variant is constructible
/// and the type system permits it, but `apply` returns AuthError on use.
/// The seam is intentionally runtime-instantiable so that when Lippa's
/// M-ExternalAuth Phase 1 lands and the cluster flag flips on, the swap is
/// wiring-only — no feature-flag flip, no signature change.
pub enum AuthMethod {
    Cookie(Secret),
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
    LoginRequired(&'static str),  // create-401 ("Run `quorum auth login`")
    #[error("session ended mid-review: {0}")]
    SessionEnded(&'static str),   // poll-401
}

impl AuthMethod {
    pub(crate) fn apply(
        &self,
        builder: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, AuthError> {
        match self {
            AuthMethod::Cookie(secret) => {
                Ok(builder.header("Cookie", format!("session={}", secret.expose())))
            }
            AuthMethod::Bearer(_) => Err(AuthError::BearerNotYetSupported),
        }
    }
}
```

When M-ExternalAuth Phase 1 lands, the swap is one-line: replace `Err(AuthError::BearerNotYetSupported)` with `Ok(builder.header("Authorization", format!("Bearer {}", secret.expose())))`. Plus a small login-flow change in `quorum-cli` (a `--bearer` flag that fetches `extension_token` instead of POSTing email/password).

#### 4.2.2 Login flow (cookie path) and cookie parsing

```rust
// crates/quorum-lippa-client/src/auth.rs (continued)

pub struct LoginRequest<'a> {
    pub base_url: &'a str,
    pub email: &'a str,
    pub password: &'a str,
}

pub async fn login_with_cookie(req: LoginRequest<'_>) -> Result<Secret, AuthError> {
    // CC-Recon-1: confirms body shape (JSON vs form). Spec assumes JSON pending recon.
    let resp = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?
        .post(format!("{}/api/v1/auth/login", req.base_url))
        .json(&serde_json::json!({"email": req.email, "password": req.password}))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(AuthError::LoginRejected(resp.status()));
    }
    extract_session_cookie(resp.headers())
}

/// Returns Result<Secret, AuthError>, not Option<String> — surfacing parse
/// failures distinctly from "no cookie present" lets the CLI give actionable
/// errors AND wraps the value in Secret at the parse boundary, eliminating
/// the brief plaintext-String lifetime that would otherwise sit between
/// extraction and Secret::new. Three failure modes:
///   (a) no Set-Cookie header at all → NoSessionCookie,
///   (b) Set-Cookie present but no `session=` cookie → NoSessionCookie,
///   (c) `session=` present but value malformed/empty → CookieParseError.
fn extract_session_cookie(
    headers: &reqwest::header::HeaderMap,
) -> Result<Secret, AuthError>;
```

Wrapping at the parse boundary means a panic, log, or backtrace anywhere downstream of `extract_session_cookie` cannot accidentally print the cookie value — by the time the function returns, the value is already a `Secret` and `Debug`/`Display` redact.

#### 4.2.3 `Secret` newtype, keyring storage, headless fallback

```rust
// crates/quorum-lippa-client/src/secret.rs

/// Wraps a sensitive string. Debug/Display redact; only expose() returns the value.
/// Derives Clone so renewed_cookie() can return Option<Secret> without
/// moving from under the std::sync::Mutex<Secret> in LippaClient (§4.2.4).
/// Cloning a String newtype is cheap and doesn't weaken redaction.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: String) -> Self { Self(s) }
    pub fn expose(&self) -> &str { &self.0 }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

impl std::fmt::Display for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}
```

`tracing` instrumentation tags cookie-bearing fields with `Secret`; emitting the field at any log level prints `<redacted>`. Cookie storage moves through `Secret` everywhere except the actual `Cookie:` header construction site in `AuthMethod::apply`.

```rust
// crates/quorum-lippa-client/src/keyring.rs

const SERVICE: &str = "quorum";

/// Per-host account: one machine can hold concurrent staging + prod sessions.
fn account_for(base_url: &str) -> Result<String, KeyringError> {
    let host = url::Url::parse(base_url)
        .map_err(|_| KeyringError::InvalidBaseUrl(base_url.to_string()))?
        .host_str()
        .ok_or_else(|| KeyringError::InvalidBaseUrl(base_url.to_string()))?
        .to_string();
    Ok(format!("lippa-session@{host}"))
}

pub enum Storage {
    OsKeyring,
    File(std::path::PathBuf),  // --no-keyring fallback
}

pub fn store_cookie(storage: &Storage, base_url: &str, value: &Secret) -> Result<(), KeyringError>;
pub fn load_cookie(storage: &Storage, base_url: &str) -> Result<Option<Secret>, KeyringError>;
pub fn delete_cookie(storage: &Storage, base_url: &str) -> Result<(), KeyringError>;
pub fn list_hosts(storage: &Storage) -> Result<Vec<String>, KeyringError>;
```

**OS keyring path** (default): `keyring::Entry::new(SERVICE, &account_for(base_url)?)`. Dispatches to Windows Credential Manager / macOS Keychain / Linux Secret Service. Logout removes the keychain entry for the targeted host; entries for other hosts persist.

**`--no-keyring` fallback** (opt-in via `quorum auth login --no-keyring`): file at `~/.config/quorum/sessions/<host>.session` (Unix: mode `0600`; Windows: ACL restricting to the current user). Stderr emits a security warning at login: `WARNING: --no-keyring stores the session cookie in plaintext at <path>; only use this in trusted environments.` This path covers headless Linux SSH boxes, minimal containers, GitHub Codespaces, and WSL distros without a Secret Service daemon — environments where the default `keyring` crate fails opaquely with `PlatformFailure`. Default behavior tries OS keyring and surfaces `keyring::Error` directly if unavailable; **the CLI does NOT silently fall back** — the user must opt in to plaintext storage.

#### 4.2.4 Consensus session client

```rust
// crates/quorum-lippa-client/src/consensus.rs

pub struct LippaClient {
    inner: reqwest::Client,
    base_url: String,
    auth: AuthMethod,
    /// In-memory mirror of the cookie. Updated on Set-Cookie reissue.
    /// Persisted to the active Storage once at clean exit if changed.
    /// std::sync::Mutex is sufficient under the current_thread runtime
    /// (§4.0); parking_lot would be dead weight.
    session_cookie: std::sync::Mutex<Secret>,
}

impl LippaClient {
    pub fn new(base_url: String, auth: AuthMethod) -> Self { /* timeout: 30s */ }

    pub async fn create_session(
        &self,
        req: SessionCreateRequest,
    ) -> Result<SessionId, ClientError>;

    pub async fn poll_status(
        &self,
        id: &SessionId,
    ) -> Result<SessionStatus, ClientError>;

    /// Returns the raw response payload. The schema-to-Review mapping is
    /// in quorum-core via review_from_json; keeping the boundary at JSON
    /// avoids introducing a quorum-lippa-client → quorum-core dependency
    /// (criterion 36) and makes the wire-format contract a recon item
    /// (CC-Recon-9) rather than a Rust type definition.
    pub async fn fetch_detail(
        &self,
        id: &SessionId,
    ) -> Result<serde_json::Value, ClientError>;

    /// Returns the current cookie value if it has been renewed since
    /// construction. Caller persists it to the active Storage at clean exit.
    pub fn renewed_cookie(&self) -> Option<Secret>;
}

pub enum SessionStatus {
    InProgress,                       // continue polling
    Converged,                        // success, fetch detail
    Failed(String),                   // server-reported failure with reason
    Cancelled,                        // user-cancelled or admin-cancelled
    Error(String),                    // unexpected terminal state
}
```

`SessionStatus` is the only typed wire shape exposed across the API — the `poll_status` call genuinely needs a closed enum so the polling loop branches correctly. Detail responses, by contrast, are passed through as JSON to `quorum-core` for parsing.

**Polling cadence:** start at 1s, exponential backoff to 8s ceiling, ±25% jitter on every interval (`base * (0.75 + rand::random::<f32>() * 0.5)`), hard timeout 5 minutes. **`Retry-After` header on 429/503 overrides the backoff schedule** — if Lippa emits one (CC-Recon-15 verifies), the next poll waits exactly that long, ignoring jitter and backoff. CC-Recon-8 confirms typical convergence time and may adjust the initial-delay default.

**Terminal-state taxonomy:** `InProgress` continues. `Converged` proceeds to detail-fetch. `Failed` / `Cancelled` / `Error` exit with code 2 and a stderr message naming the state. `5xx` mid-poll retries with backoff. `401` mid-poll exits 3 with poll-401 messaging (§4.4.1).

**Cookie auto-renewal capture:** every authenticated response is inspected for `Set-Cookie: session=<new>`. If present and different from the in-memory value, the in-memory value is updated. **The active `Storage`** (OS keyring under default mode, file at mode 0600 under `--no-keyring`) is written once at clean exit, only if the final value differs from the initial loaded value. This avoids 30+ persistence writes per `quorum review` invocation on slow Windows + antivirus environments and applies uniformly across both storage backends.

### 4.3 `quorum-core`

#### 4.3.1 `Review` type and `SessionDetail` mapping

```rust
// crates/quorum-core/src/review.rs

pub struct Review {
    pub session_id: String,
    pub findings: Vec<Finding>,
    pub model_role: String,           // requested role, e.g. "reviewer"
    pub model_name: String,           // resolved model, e.g. "claude-sonnet-4-5"
    pub elapsed: std::time::Duration,
    pub project_id: String,
    pub base_url: String,
}

pub struct Finding {
    pub severity: Severity,
    pub file: Option<String>,
    pub line_range: Option<(u32, u32)>,
    pub title: String,
    pub body: String,                 // markdown
    pub suggestion: Option<String>,
}

pub enum Severity { High, Medium, Low, Info }

pub struct RepoMetadata {
    pub remote_url: Option<String>,   // already credential-stripped
    pub branch: String,
    pub head_sha: String,
    pub project_id: String,
    pub base_url: String,
    pub model_role: String,
}

#[derive(thiserror::Error, Debug)]
pub enum ParseError {
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("unexpected severity value: {0}")]
    UnknownSeverity(String),
    #[error("malformed JSON shape: {0}")]
    Malformed(String),
}

/// Free function — the only legal mapping site between Lippa wire format
/// and the Review type. quorum-cli calls this after
/// quorum-lippa-client::LippaClient::fetch_detail returns a serde_json::Value.
///
/// Taking serde_json::Value (rather than a typed wire struct or a trait
/// the wire struct must implement) eliminates the cross-crate type
/// dependency that would otherwise require quorum-lippa-client to import
/// from quorum-core (Rust trait coherence). The expected JSON shape is
/// pinned by CC-Recon-9; v1.0 ships against that recon outcome.
pub fn review_from_json(
    value: &serde_json::Value,
    repo: &RepoMetadata,
) -> Result<Review, ParseError>;
```

The mapping function does its own structural parsing of the JSON. Field names, severity tokens, and overall shape come from CC-Recon-9; if Lippa's response shape differs from the spec's pre-recon assumption (typed `findings: [{ severity, file, line_range, title, body, suggestion }]`), `review_from_json` is the only place that needs adjustment. `quorum-cli` and `quorum-lippa-client` are untouched by such adjustments.

#### 4.3.2 Bundle assembly

The context bundle submitted to Consensus uses the **HEAD-vs-index** diff primitive (`git2::Repository::diff_tree_to_index`), not index-vs-worktree. File contents are read from **index blobs**, not the working tree, so unstaged edits in modified files do not leak into the review.

**Per-section budget partitioning:**

| Section | Budget | Truncation behavior on overflow |
|---|---|---|
| Diff body (unified, 3 context lines) | 100KB | Tail-truncate with marker `[diff truncated: <bytes> exceeded 100KB; review only top portion]` |
| Changed-file contents (sum, post-exclusion, from index blobs) | 80KB | Largest-file-first inclusion until budget exhausted; remainder marked `[file omitted: <path>, <bytes>]` |
| Memory context (CLAUDE.md / AGENTS.md / .cursorrules; first-match) | 20KB | Tail-truncate with marker |
| Conventions (`.quorum/conventions.md`, committed-only) | 10KB | Tail-truncate with marker |
| Repo metadata + delimiter envelope | ~2KB | Hard, never exceeded |
| **Total assembled bundle hard cap** | **200KB** | Refuse with exit 2 if assembled total exceeds 200KB despite per-section budgets |

The 200KB total is a conservative pre-recon default; CC-Recon-5 may adjust upward if Lippa's server-side cap is materially higher AND there's evidence reviews are getting clipped. Adjusting downward is never necessary.

**Deny-list filter** (applied to both diff and file-contents sections, before budgeting):

```
.env                    .env.*                  *.pem
*.key                   *.p12                   *.pfx
id_rsa                  id_dsa                  id_ecdsa     id_ed25519
.aws/credentials        .npmrc                  .netrc       .pgpass
secrets.yml             secrets.yaml            secrets.json
```

Plus: anything matched by `.gitignore` (except files with `git add -f` force, which the diff naturally excludes). Excluded files appear in the diff as a marker line (`[excluded by deny-list: <path>, <reason>]`).

**Glob semantics.** Each pattern is treated as `**/<pattern>` — basename-globbed at any depth. So `.env` excludes `.env`, `infra/staging/.env`, `services/api/.env.production`; `secrets.yml` excludes `services/api/secrets.yml`; `id_rsa` excludes any file named `id_rsa` at any depth. The relative path `.aws/credentials` excludes `.aws/credentials` at the repo root AND `home/user/.aws/credentials` inside the repo. Patterns containing `/` are matched as suffix-globs (`**/.aws/credentials`); single-token patterns are basename-only. Test fixtures cover both nested and root-level cases.

**Known false-positive class:** `*.key` matches license-key files in some Java/.NET projects (typically `<product>.key` with a non-secret license token). This is an acceptable defense-in-depth trade-off for Phase 1A — Phase 2's customizable redaction patterns will let users opt out. Users with legitimate non-secret `.key` files can inspect the bundle markers in stderr before trusting Phase 1A's exclusions.

The deny-list is hardcoded in Phase 1A; user-customizable patterns are Phase 2 alongside redaction. **This is defense-in-depth, not a security guarantee.** A privacy-conscious user must still review what they stage.

**Repo-memory file discovery:** first-match-wins across `CLAUDE.md`, `AGENTS.md`, `.cursorrules` in the repo root. Stderr at info level emits `using repo memory: <chosen path>` and (if other candidates exist) `ignored: <other path>, <other path>`. The note is emitted only when at least one candidate file exists; absence is silent.

**Conventions file:** `.quorum/conventions.md` is read only if it exists in the working tree AND is tracked AND has no uncommitted changes (matches `HEAD:.quorum/conventions.md` byte-for-byte). If the file exists in the working tree but fails any of the latter two conditions, stderr emits a single note: `note: .quorum/conventions.md present but not committed (or has uncommitted changes); ignored.` The note is suppressed when the file is absent.

All bundle content is wrapped in delimiters (`<<<QUORUM_REPO_CONTENT_BEGIN>>> ... <<<QUORUM_REPO_CONTENT_END>>>`).

#### 4.3.3 JSON archive

```
.quorum/
  config.toml             # written by `quorum link`; may be untracked
  conventions.md          # user-authored; reviewed only if tracked
  reviews/
    2026-05-10T14-23-01Z.json
    2026-05-10T15-08-44Z.json
```

Filename uses `:` → `-` substitution **for Windows filesystem compatibility only** (NTFS treats `:` as an alternate-data-stream separator). The substitution does not change the format inside the JSON: the `started_at` field retains strict RFC 3339 (`T14:23:01Z`) regardless of the filename's flavor.

Archive schema:

```json
{
  "schema_version": 1,
  "session_id": "consensus_session_XXX",
  "started_at": "2026-05-10T14:23:01Z",
  "elapsed_seconds": 47.2,
  "model_role": "reviewer",
  "model_name": "claude-sonnet-4-5",
  "project_id": "proj_abc123",
  "base_url": "https://app.lippa.ai",
  "repo": {
    "remote_url": "git@github.com:org/repo.git",
    "branch": "feature/foo",
    "head_sha": "abc1234"
  },
  "findings": [ /* ... */ ]
}
```

`project_id` and `base_url` are required. They support multi-tenant + multi-environment trace-back (Gemini's catch). `remote_url` is optional (`null` if `quorum link --no-remote-url` was used) and HTTPS credentials (`https://user:pw@host/...`) are stripped to `https://host/...` before serialization.

**Deterministic serialization:** archive uses `serde_json::to_writer_pretty` with sorted keys (via a deterministic key-ordering wrapper). The archive object is constructed once and serialized once into a buffer. `quorum review --json` writes the same buffer to stdout AND to disk. Criterion 17 is byte-identical-by-construction: there is no second serializer.

### 4.4 `quorum-cli` — command surface

```
quorum auth login [--url <URL>] [--no-keyring]    # default URL via CC-Recon-13
quorum auth logout [--url <URL>]                  # local + server-side if endpoint exists
quorum auth status                                # whoami ping; distinguishes valid vs stale

quorum link --project <project_id>                # bind current repo to a Lippa project
quorum link --show
quorum link --no-remote-url                       # opt out of remote_url archiving

quorum review                                     # review staged diff in current repo
quorum review --json                              # stdout = same buffer as archive

quorum --version
quorum --help
```

#### 4.4.1 Exit codes

| Code | Meaning |
|---|---|
| 0 | Review completed; no high-severity findings (or non-review command succeeded). |
| 1 | Review completed; one or more high-severity findings present. |
| 2 | Tooling error — network failure, malformed Lippa response, missing config, oversized bundle, server-reported terminal failure (`failed`/`cancelled`/`error`), non-TTY login. Stderr distinguishes retryable vs local. |
| 3 | Authentication required or expired. Stderr distinguishes create-401 vs poll-401. |

**Stderr taxonomy for exit 2** (so hooks and CI can act):
- Retryable network/server: `network error: <detail>; retry may succeed.`
- Server-reported terminal failure: `Consensus session <state>: <detail>; not retryable as-is.`
- Local usage error: `Run \`quorum link\`` / `Diff too large — bundle size <N>KB exceeds 200KB cap` / `quorum auth login requires an interactive terminal.`

**Stderr taxonomy for exit 3:**
- Create-401: `Authentication required. Run \`quorum auth login\`.`
- Poll-401: `Session ended mid-review (rotation or expiry). Run \`quorum auth login\` and retry.`

Exit codes are stable contract from Phase 1A onward. Phase 1B git-hook integration depends on the create/poll-401 distinction.

#### 4.4.2 TTY detection

```rust
// crates/quorum-cli/src/main.rs
fn require_tty_for_login() -> Result<(), CliError> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return Err(CliError::NoTtyForInteractive);
    }
    Ok(())
}
```

`quorum auth login` is the only Phase 1A command that prompts. It refuses non-TTY contexts (exit 2). `quorum review` reads no interactive input and runs cleanly in any context. CI / hook integration via env-var credentials is Phase 1B.

### 4.5 Output rendering

`render_review_markdown(&Review) -> String` is pure. Same renderer drives stdout in Phase 1A and (Phase 1B) `ratatui`.

```
# Quorum review — feature/foo @ abc1234
Reviewed by claude-sonnet-4-5 (role: reviewer) in 47.2s · session consensus_session_XXX

## High severity (1)
### src/lib.rs:42-58 — Unbounded user input passed to format string
The handler at line 42 passes `user_input` directly into `format!`...
Suggestion: use `format_args!` with a fixed format string and `{}` placeholders.

## Medium severity (2)
...

## Low severity (0)

## Notes (3)
### src/lib.rs:101 — Style preference
...

## Archive
JSON archive written to .quorum/reviews/2026-05-10T14-23-01Z.json
```

`Severity::Info` renders under "Notes" after High/Medium/Low. When the total finding count exceeds 50, stderr emits a one-line note: `note: <N> findings; pipe to less or use --json for programmatic consumption.`

### 4.6 Tests

Five test suites, all mock-only (no live Lippa).

- **`tests/auth_flow.rs`** — login → status (whoami ping) → logout round-trip against `mockito`. Verifies cookie capture/storage/presentation; per-host keyring isolation (staging + prod coexist); expired-session 401 produces exit 3 with create-401 messaging; cookie-parse error variants surface distinctly; `--no-keyring` writes file mode 0600 with stderr warning; **`--no-keyring` cookie renewal** — when running with file-backed storage, a renewed `Set-Cookie` mid-session results in exactly one file write at clean exit (parallel to the keyring case in `tests/end_to_end.rs`).
- **`tests/bundle_assembly.rs`** — golden-file fixtures exercise: small diff, large diff, binary file in diff, missing CLAUDE.md, untracked conventions.md (must be ignored with stderr note), submodule, oversized bundle, and the new git edge cases: **renames, deletes, mode changes, partial staging (file with both staged and unstaged hunks), new staged file, untracked file (excluded by default)**. Plus deny-list fixtures: `.env` at repo root excluded, **`infra/staging/.env` (nested) excluded**, `*.pem` excluded, `services/api/secrets.yml` (nested) excluded, gitignored file excluded, force-added gitignored file included, **`.aws/credentials` at root and at nested path both excluded** (path-pattern matching).
- **`tests/render.rs`** — `Review` instances → markdown strings, snapshot-tested with `insta`. Includes Info-only review (renders under Notes), 50+ finding review (stderr note appears), zero-finding review.
- **`tests/end_to_end.rs`** — `quorum review` driven against `mockito` with canned Consensus responses. Verifies polling cadence + jitter, `Retry-After` honored, terminal-state taxonomy (failed/cancelled/error all surface to exit 2 distinctly), cookie auto-renewal captured but persisted only at clean exit, archive byte-identical to `--json` stdout.
- **`tests/secret_redaction.rs`** — `Secret` newtype Debug/Display emit `<redacted>`; `tracing` output at `RUST_LOG=trace` against mock-server runs contains zero plaintext cookie values.

Live verification against staging Lippa is acceptance-criterion concern (§6.1), not part of `cargo test`.

### 4.7 Cookie lifecycle

This subsection collects all cookie-state behavior in one place. The eventual Bearer swap touches §4.2.1's `apply` and the CLI login flow; this section becomes a note that "Bearer-authed requests carry no session cookie; renewal/logout semantics described here apply only to the cookie path."

**Capture (login).** `quorum auth login` POSTs credentials, captures `Set-Cookie: session=<value>`, wraps the value in `Secret`, stores in keyring (or `--no-keyring` file fallback). Per-host account scoping (§4.2.3) lets one machine hold concurrent staging + prod sessions.

**Use (every authenticated request).** `LippaClient` loads the `Secret` once at construction. `AuthMethod::apply` injects `Cookie: session=<value>` on every outgoing request. The header is constructed from `Secret::expose()` only at the call site; the value is otherwise opaque to the rest of the codebase.

**Renewal (response inspection).** Every authenticated response is inspected for `Set-Cookie: session=<new>`. If present and different from in-memory, the in-memory `Secret` is updated atomically (`std::sync::Mutex`). **Persistence to the active `Storage` happens once at clean exit, only if the final value differs from the initial loaded value.** This applies uniformly under both OS-keyring and `--no-keyring` file-backed storage — `Storage::store_cookie(&storage, base_url, &final_value)` is called once at clean exit from the CLI's top-level command handler. This avoids 30+ persistence writes per `quorum review` invocation. CC-Recon-6 confirms whether `Set-Cookie` is reissued on activity (rolling) or only at login (fixed-window) and whether this code path actually fires in practice; the implementation is correct under both regimes.

**Server-side invalidation (401 mid-anything).** A 401 response on `quorum review`'s create-session call exits 3 with create-401 messaging. A 401 mid-poll exits 3 with poll-401 messaging (distinguishes "session ended during work" from "you're not logged in" — both require `quorum auth login` but the user-facing context differs). Password change, admin invalidation, and rotation all manifest as 401 and follow the same flow. The CLI does not proactively detect server-side invalidation; the contract is "next request after invalidation returns 401, CLI handles gracefully."

**Logout.** `quorum auth logout` removes the keyring entry for the targeted host. **CC-Recon-10 determines whether Lippa exposes a logout endpoint.** If yes, the CLI calls it before deleting the local entry — best-effort, with a 5-second timeout; failure to reach the server still removes the local entry but emits stderr `note: server-side logout call failed (<error>); local session deleted but server may consider the session active until expiry.` If no logout endpoint exists, `quorum auth logout` is documented as **local logout only** and the README explicitly notes that the server session remains valid until expiry. Either way: idempotent (second invocation succeeds with `Already logged out`).

**Status check (`quorum auth status`).** Distinguishes "logged in (keyring entry present, whoami ping returns 200)" from "logged in but stale (keyring entry present, whoami ping returns 401)" from "not logged in (no keyring entry)." Whoami ping uses `GET /api/v1/auth/me` (CC-Recon-1 confirms shape). Stale state suggests `quorum auth login`.

---

## 5. Files Modified (Phase 1A is greenfield — most files are CREATED)

| File | Change | LOC est. |
|---|---|---|
| `Cargo.toml` (workspace root) | New — workspace declaration, shared profile config | +30 |
| `crates/quorum-cli/Cargo.toml` | New | +25 |
| `crates/quorum-cli/src/main.rs` | New — clap, command dispatch, wire→Review mapping site | +130 |
| `crates/quorum-cli/src/commands/{auth,review,link}.rs` | New | +320 |
| `crates/quorum-cli/src/render.rs` | New | +130 |
| `crates/quorum-cli/src/exit.rs` | New | +30 |
| `crates/quorum-core/Cargo.toml` | New | +20 |
| `crates/quorum-core/src/{lib,review,bundle,deny_list,git,conventions,config,archive,discovery}.rs` | New | +650 |
| `crates/quorum-lippa-client/Cargo.toml` | New | +25 |
| `crates/quorum-lippa-client/src/{lib,auth,secret,client,consensus,wire,keyring}.rs` | New | +480 |
| `tests/fixtures/repos/<name>/...` | New — golden-file repo fixtures (incl. rename/delete/mode/partial/submodule/untracked + deny-list cases) | +280 |
| `tests/{auth_flow,bundle_assembly,render,end_to_end,secret_redaction}.rs` | New | +650 |
| `SERVICES.md` | New — auth storage, bundle assembly, Lippa-client surface, cookie lifecycle, deny-list | +160 |
| `HISTORY.md` | New — milestone close entry for Phase 1A | +35 |
| `CLAUDE.md` | Update milestone status, add common commands | ±15 |
| `README.md` | Basic usage + privacy note + uninstalling instructions | +60 |
| `.gitignore` | Add `target/`, `prompts/` (per CLAUDE.md), `.quorum/reviews/` | ±5 |

**Total: ~3,000 LOC of which ~930 are tests.** Production code: ~2,000 LOC. The growth from v0.1's 2,400 reflects the boundary fix (mapping moves crate-side, slight restructure), the deny-list module, the `Secret` newtype, the headless-keyring fallback path, the cookie-lifecycle module, and the expanded test fixtures.

The `prompts/` `.gitignore` entry is per Quorum repo's `CLAUDE.md` — CC session-driver prompts are intentionally untracked. Cross-referenced here to avoid the "unreferenced gitignore entry" question that surfaced in peer review.

---

## 6. Acceptance Criteria

### 6.1 Behavioural

1. `quorum --version` prints a semver string (e.g. `quorum 0.1.0`) and exits 0.
2. `quorum auth login` with valid credentials against staging Lippa stores the session cookie in OS keychain under account `lippa-session@<host>` and prints `Logged in to <URL> as <email>`. Exit 0.
3. `quorum auth login` with invalid credentials prints `Login rejected: <status>` and exits 3. No keychain entry written.
4. `quorum auth login` invoked in a non-TTY context exits 2 with `quorum auth login requires an interactive terminal`.
5. `quorum auth login --no-keyring` writes the session cookie to `~/.config/quorum/sessions/<host>.session` with mode `0600` (Unix) / restricted ACL (Windows) AND emits stderr `WARNING: --no-keyring stores the session cookie in plaintext at <path>; only use this in trusted environments.` Exit 0.
6. `quorum auth status` against a valid session pings `GET /api/v1/auth/me`, prints `Logged in to <URL> as <email>`. Exit 0.
7. `quorum auth status` against a stale session (keyring entry present, ping returns 401) prints `Logged in to <URL>, but session is stale — run \`quorum auth login\`.` Exit 3.
8. `quorum auth status` with no keyring entry prints `Not logged in`. Exit 0.
9. `quorum auth logout` removes the keyring entry for the targeted host AND (if CC-Recon-10 confirms a logout endpoint) calls it best-effort. Idempotent — second invocation prints `Already logged out`. Exit 0.
10. **Per-host isolation:** `quorum auth login --url <staging>` followed by `quorum auth login --url <prod>` produces two distinct keychain entries; `quorum auth logout --url <staging>` removes only the staging entry.
11. `quorum link --project <id>` writes `.quorum/config.toml` with `project_id = "<id>"` and the configured base URL. Exit 0.
12. `quorum link --no-remote-url` writes a `remote_url = false` flag; subsequent reviews omit `remote_url` from archives.
13. `quorum link --show` prints the configured project. Exit 0 if linked, exit 2 if not.
14. `quorum review` invoked in a directory with no `.quorum/config.toml` exits 2 with `Run \`quorum link --project <id>\` first`.
15. `quorum review` with no auth (no keychain entry) exits 3 with `Authentication required. Run \`quorum auth login\``. **(create-401 path)**
16. `quorum review` with no staged changes prints `No staged changes — nothing to review` and exits 0.
17. `quorum review` with staged changes and valid auth submits to Lippa, polls (with jitter and `Retry-After` honored), fetches detail, renders markdown to stdout, writes `.quorum/reviews/<ISO>.json`. Exit 0 if no high-severity findings, 1 if any high-severity findings.
18. `quorum review` with expired session at create exits 3 with create-401 messaging. **(create-401)**
19. `quorum review` with valid create but 401 mid-poll exits 3 with `Session ended mid-review (rotation or expiry). Run \`quorum auth login\` and retry.` **(poll-401)**
20. `quorum review` with terminal `failed`/`cancelled`/`error` Consensus state exits 2 with `Consensus session <state>: <detail>`. Stderr distinguishes from network errors.
21. `quorum review` with a context bundle exceeding 200KB exits 2 with `Diff too large — bundle size <N>KB exceeds 200KB cap` BEFORE submission. No Lippa call made.
22. `quorum review` with per-section overflow truncates and emits markers. Submission proceeds with truncated bundle if total stays under 200KB.
23. `quorum review` invoked in a subdirectory of a git repo correctly resolves the repo root via `git2::Repository::discover` and reads `.quorum/config.toml` from the root.
24. `quorum review` with `.quorum/conventions.md` present in working tree but NOT committed (untracked OR uncommitted-changes) reads the bundle as if conventions are absent. Stderr emits the note exactly once. **The note is suppressed when the file is absent.**
25. `quorum review` with multiple repo-memory candidates emits stderr at info: `using repo memory: CLAUDE.md`, `ignored: AGENTS.md, .cursorrules`. Note suppressed when zero candidates exist.
26. `quorum review --json` writes JSON to stdout. The stdout buffer and `.quorum/reviews/<ISO>.json` file are byte-identical because they are written from the same serialized buffer.
27. The JSON archive includes `schema_version: 1` as the first key (alphabetic ordering would not produce this; the `schema_version` field is documented as serialized-first by convention).
28. Polling timeout (5 minutes default) produces exit 2 with `Consensus session timed out — last status: <state>`.
29. `quorum review` with a 429 response and `Retry-After: <N>` waits exactly `<N>` seconds before the next poll, ignoring jitter and backoff.
30. **Staged-diff git semantics:** review covers (a) modified files (HEAD-vs-index, contents from index blob — not working tree), (b) new staged files, (c) deleted staged files, (d) renamed staged files, (e) mode changes. Excludes (f) unstaged hunks of partially-staged files, (g) untracked files.
31. **Deny-list:** files matching `.env`, `*.pem`, `id_rsa`, `.netrc`, etc. are excluded with `[excluded by deny-list: <path>]` markers. `.gitignore`-matched files are excluded unless `git add -f` was used.

### 6.2 Internal

32. `cargo build --release` succeeds on Windows / Git Bash, macOS, Linux. Verified locally for at least Windows + Linux.
33. `cargo clippy --workspace -- -D warnings` is clean.
34. `cargo fmt --check --all` is clean.
35. `cargo test --workspace` — all five test suites pass.
36. **Crate boundary:** `quorum-core` Cargo.toml has no `quorum-lippa-client` dependency. `quorum-lippa-client` Cargo.toml has no `quorum-core` dependency. `quorum-core` Cargo.toml has no `tokio` dependency. Verified by `cargo tree`.
37. **No-panic auth:** Constructing `AuthMethod::Bearer(Secret::new("test".into()))` and calling `.apply(builder)` returns `Err(AuthError::BearerNotYetSupported)`. Verified by unit test; never panics.
38. **`Secret` redaction mechanism:** Both `Debug` and `Display` for `Secret` emit `<redacted>` (verified by unit test). Cookie storage in `LippaClient`, `AuthMethod`, and `keyring::Storage` all use `Secret`. `tracing` output at `RUST_LOG=trace` against mock-server runs produces 0 hits for the test cookie value (verified by `tests/secret_redaction.rs`).
39. The cookie value is read from / written to the keyring crate (or `--no-keyring` file at mode `0600`) exclusively. `grep -rn "session=" crates/` does not find the cookie value being written to a non-fallback file path.
40. **Cookie auto-renewal write strategy:** `tests/end_to_end.rs` runs a 30-poll session with `Set-Cookie` reissued on every response; verifies exactly 1 keyring write occurs at clean exit (not 30).
41. `CLAUDE.md ≤ 280 lines` after milestone close (`wc -l CLAUDE.md`).
42. `SERVICES.md` exists and contains at least five subsections (auth storage, bundle assembly, Lippa-client surface, cookie lifecycle, deny-list).
43. `HISTORY.md` exists with a Phase 1A close entry.

### 6.3 Security / Privacy

44. Repo content (diff body, file contents, conventions, repo-memory) is wrapped in `<<<QUORUM_REPO_CONTENT_BEGIN>>>` / `<<<QUORUM_REPO_CONTENT_END>>>` delimiters in the submitted prompt. Verified by inspecting the request body in a TestClient run.
45. Files matching binary heuristics (null byte in first 8KB, recognized binary extensions) are excluded from the bundle and replaced with a one-line marker.
46. Files exceeding 2MB are excluded with a marker.
47. Absolute filesystem paths never appear in the submitted bundle. All paths are repo-relative.
48. **Deny-list exclusion:** every file matching the hardcoded deny-list patterns appears in the bundle as a `[excluded by deny-list: <path>, <reason>]` marker, with file content NOT present anywhere in the assembled bundle (verified by fixture).
49. **HTTPS credential stripping:** A `git remote get-url` returning `https://user:pw@github.com/org/repo` is archived as `https://github.com/org/repo`. Verified by fixture.
50. The session cookie value is the only secret material handled. It is never written to `.quorum/`, `~/.quorum/`, environment variables, log files, or the JSON archive.
51. The JSON archive does not contain the user's email, password, or any auth artifacts. It contains: schema_version, session_id, timestamps, model_role, model_name, project_id, base_url, repo metadata (with optional remote_url), findings.

---

## 7. Recon Items (CC must verify before Stage 1)

Each item is a read-only inspection of the Lippa codebase or a single curl probe against staging Lippa. Total recon time: 4-6 hours (revised up from v0.1's 2-4 hours due to expanded items). **Four items are blockers**; the rest can run in parallel with implementation if needed.

- **CC-Recon-1 (BLOCKER): `/api/v1/auth/login` request/response shape AND non-browser-client viability.** Locate the route handler. Document:
  - Request body format: JSON `{email, password}` or form-encoded?
  - Response: `Set-Cookie: session=<value>` with what attributes (`HttpOnly`, `Secure`, `SameSite`, `Max-Age`)?
  - **CSRF middleware:** does any global or route-specific middleware require a CSRF token on POST `/api/v1/auth/login` or any cookie-authenticated `/api/v1/consensus/*` POST? If yes, document the token-acquisition flow.
  - **2FA:** does the endpoint support / require 2FA at the cluster level or per-account? If mandatory, **Phase 1A blocks until Bearer (M-ExternalAuth) lands or 2FA support is added.**
  - **Non-browser-client intent:** is `/api/v1/auth/login` designed for non-browser clients, or is it a frontend-only endpoint? Inspect docs, comments, and any `User-Agent` checks.
  - **Decision branch:** if JSON-bodied, no CSRF, no mandatory 2FA, intended (or at least neutral) toward non-browser clients → proceed with §4.2.2 as drafted. Otherwise file as Phase-1A blocker.

- **CC-Recon-2 (BLOCKER): Single-model Consensus behavior.** Submit a session via curl with `model_roles=[<one>]`. Verify (a) accepted, (b) converges normally, (c) `rounds[]` non-empty and parses with the multi-model schema. **Decision branch:** if rejected or short-circuits → file Lippa-side spec to allow `len(model_roles) == 1` cleanly; gate Phase 1A on it.

- **CC-Recon-3: `project_id` resolution and Consensus session create payload.** Inspect the POST handler. Document `project_id` requirement, full payload shape, error shape on wrong project_id. Phase 1A's `quorum link --project <id>` writes the value to config — see CC-Recon-19 for project_id scope semantics.

- **CC-Recon-4: Target repo discovery edge cases.** Confirm `git2::Repository::discover` walk-up behavior. Test cases: subdirectory, submodule (treats submodule as the repo — document in `--help`), outside any repo (exits 2).

- **CC-Recon-5 (BLOCKER): Diff size / token budget cap.** Locate Lippa's Consensus prompt-budget enforcement. Document server-side cap and error shape. Phase 1A's 200KB local default may adjust upward if server cap is materially higher.

- **CC-Recon-6: Cookie session lifetime + auto-renewal.** Inspect `SessionMiddleware` config. Document `Max-Age`, rolling vs fixed-window, whether `Set-Cookie` is reissued on activity. The auto-renewal capture in §4.2.4 / §4.7 is correct under all three regimes; recon confirms the actual one and whether the code path fires in practice.

- **CC-Recon-7: Consensus model role naming.** Locate the role registry. Document the canonical name of the single role Phase 1A defaults to (`"reviewer"`, `"primary"`, `"claude-sonnet-4-5"`, etc.). Phase 1A writes the chosen role into a constant in `quorum-cli/src/commands/review.rs`. Map `model_role` to `model_name` in the response (CC-Recon-9).

- **CC-Recon-8: Consensus polling cadence and convergence shape.** Run a real session against staging. Observe typical convergence time and status response shape (`status: "running"|"converged"|"failed"|"cancelled"|"error"`). May tune the 1s-initial / 5min-timeout defaults.

- **CC-Recon-9 (BLOCKER): Consensus detail response schema.** Inspect the GET response. Document the exact JSON shape that maps to `Finding`: severity values used, file/line range fields, body format (markdown? plain text?), suggestion field presence. **If Lippa returns prose-only with no typed fields, `Finding`'s typed schema is wrong from line one and the spec needs revision before implementation.**

- **CC-Recon-10: Logout endpoint and invalidation semantics.** Does Lippa expose `POST /api/v1/auth/logout`? If yes, `quorum auth logout` calls it best-effort before deleting the local entry. If no, document `quorum auth logout` as **local logout only** in the README. Either outcome is implementable; the spec branches on the answer.

- **CC-Recon-11: Keyring availability across target environments.** Test `keyring::Entry::new(...)` on macOS, Windows, native Linux desktop, headless Linux SSH (no `gnome-keyring` / `kwallet`), WSL2, GitHub Codespaces, generic dev container. Document failures and confirm `--no-keyring` fallback covers them.

- **CC-Recon-12: Lippa server-error response shape under polling.** Trigger a forced-failure session (model provider 5xx, malformed prompt, etc.). Document the `/sessions/{id}/status` response: which status string? what `detail` field? Used to populate the `SessionStatus::{Failed, Cancelled, Error}` taxonomy in §4.2.4.

- **CC-Recon-13: Default URL — login vs Consensus host.** Confirm whether `/api/v1/auth/login` and `/api/v1/consensus/*` live on `app.lippa.ai`, `api.lippa.ai`, or split. Update `quorum auth login` default URL accordingly.

- **CC-Recon-14: Memory-context file size distribution.** Sample 5-10 representative repos and measure `CLAUDE.md` / `AGENTS.md` / `.cursorrules` sizes. If real-world files routinely exceed 20KB, raise the §4.3.2 memory-context budget at the cost of another section.

- **CC-Recon-15: `Retry-After` header on 429 / 503.** Probe Lippa's rate-limit and overload responses. If the server emits `Retry-After`, polling honors it (§4.2.4); if not, document and proceed with backoff-only.

- **CC-Recon-16: Diff format expected by Consensus prompt.** Verify what Consensus expects: unified diff with how many context lines? `--patience` / `--minimal`? Wrong format degrades model performance silently.

- **CC-Recon-17: `/consensus/sessions` POST idempotency.** Does retrying a create-session call (after network timeout where the request may or may not have been received) duplicate cost/work? Polling/retry logic depends on it.

- **CC-Recon-18: Rate-limit behavior for cookie-authenticated CLI traffic.** Document per-user rate limits on `/api/v1/consensus/*`. Phase 1A's typical session (1 create + ~30 polls + 1 detail) should fit normal limits, but verify.

- **CC-Recon-19: `project_id` scope semantics.** Globally unique? User-scoped? Workspace-scoped? Slug-like or UUID? Cross-tenant wrong-project error shape? `quorum link --project <id>` accepts what the user provides; the API rejects if invalid.

---

## 8. Stage Breakdown

Single stage. ~4-6 days CC work.

**Stage 1 — Walking-skeleton CLI shipped.**

- Resolve all 19 recon items (~4-6 hours total). **Four items are hard prerequisites to implementation: CC-Recon-1, CC-Recon-2, CC-Recon-5, CC-Recon-9.** The rest can run in parallel with non-dependent implementation work.
- Bootstrap Cargo workspace per §4.1; configure tokio per §4.0.
- Implement `quorum-lippa-client` (§4.2): `Secret` newtype, `AuthMethod` + `AuthError`, login with typed cookie parse errors, keyring + `--no-keyring` fallback, `LippaClient` with polling jitter / `Retry-After` / terminal-state taxonomy / cookie auto-renewal capture.
- Implement `quorum-core` (§4.3): `Review`/`Finding`/`Severity`, `review_from_json` free function (parses `serde_json::Value` from `LippaClient::fetch_detail`), bundle assembly via `diff_tree_to_index` with per-section budgets and deny-list, JSON archive with deterministic serialization.
- Implement `quorum-cli` (§4.4): clap parsing, command dispatch, wire→Review mapping site, exit-code mapping with create/poll-401 distinction, TTY detection.
- Cookie-lifecycle plumbing (§4.7): in-memory mirror, end-of-invocation persistence, server-side logout (gated on CC-Recon-10).
- Write five test suites (§4.6) including the new git-edge-case fixtures and the `Secret` redaction suite.
- Update `CLAUDE.md` milestone status; line-cap check.
- Create `SERVICES.md` per §"Documentation Routing": auth storage, bundle assembly, Lippa-client surface, cookie lifecycle, deny-list.
- Create `HISTORY.md` Phase 1A close entry.
- Create `README.md` basic usage section + privacy note + uninstalling instructions.
- Live verification against staging Lippa: behavioural acceptance criteria 1-31 (§6.1).

If any acceptance criterion fails post-build: do not ship the binary. Phase 1A is a greenfield walking skeleton; partial-ship is worse than delay.

---

## 9. Rollback Plan

Phase 1A introduces no Lippa-side change and no persistent local state beyond `.quorum/config.toml`, `.quorum/reviews/*.json`, and the OS-keychain entry (or `--no-keyring` fallback file). Rollback is correspondingly simple.

### 9.1 Code rollback (clean)

`git revert` the Phase 1A merge commit in `accillion/quorum`. The `accillion/lippa` repo is untouched. Local users with stale keychain entries can run `quorum auth logout` to clean up; their `.quorum/` directories are local artifacts they can `rm -rf`. Total rollback duration: <10 minutes.

### 9.2 User-side cleanup (optional)

`quorum auth logout && rm -rf .quorum/` works even after the binary is removed. Manual keychain cleanup documented in the README for users who lose the binary first: `security delete-generic-password` (macOS), `secret-tool clear` (Linux), Credential Manager (Windows). For `--no-keyring` users: `rm ~/.config/quorum/sessions/*.session`.

### 9.3 No contract rollback risk

Quorum has no API contract with anyone in Phase 1A. The only direction of dependency is Quorum → Lippa, and Quorum makes no guarantees about its own surface in this phase.

---

## 10. Open Questions

These are flagged for re-review. Distinct from recon items, which CC verifies against Lippa code.

1. **Polling cadence default.** 1s → 8s exponential backoff with ±25% jitter and 5-minute timeout. CC-Recon-8 may refine.

2. **`quorum review --json` format.** Pretty-printed (sorted keys, 2-space indent) vs minified. v0.2 chose pretty for human inspection of archives; minified would save bytes but is less debuggable. Position: pretty wins; archive is for humans first, machines second.

3. **`.quorum/conventions.md` in `.gitignore`.** Spec says ignored if not committed. If gitignored, the user has explicitly opted out. Phase 1A treats as absent. Phase 1B's promotion flow makes this explicit.

4. **`quorum config edit` for `.quorum/config.toml`.** Currently no — users edit directly. Marginal value, defer to Phase 1B+.

5. **Discovery order.** First-match-wins across CLAUDE.md / AGENTS.md / .cursorrules with stderr logging. Resolved in v0.2 (Gemini's concatenation pushback rejected; ChatGPT's logging pushback adopted). Re-review may revisit if data emerges.

6. **`Secret` newtype crate placement.** Currently in `quorum-lippa-client::secret`. If `quorum-core` ever needs to handle secrets (Phase 2 redaction may), promote to a tiny shared crate. Not a Phase 1A decision.

---

## 11. Forward Compatibility — Phase 1B+

The Phase 1A architecture must accommodate these additions without restructure:

- **Interactive dismiss layer (Phase 1B).** `render_review_markdown` is pure (returns `String`). `Review`, `Finding`, `Severity` are public types in `quorum-core`; the markdown renderer is one consumer among future others.
- **Local dismissal store (Phase 1B).** `.quorum/dismissals.sqlite` lands alongside `.quorum/reviews/`. Each `Finding` has enough identity (file + line range + title) for a dismissals row. Phase 1A's archive schema is non-breaking under that addition.
- **Hook installer (Phase 1B).** `quorum install --hook=pre-push` lands. Phase 1A's binary already runs cleanly in non-TTY contexts (criterion 4 + TTY detection in §4.4.2), so hook integration is a wrapper around `quorum review`.
- **CI / non-interactive auth (Phase 1B alongside hooks).** `--non-interactive` + env-var credentials for hook contexts. Threat model gets explicit treatment (env-var leakage via shell history, `/proc/<pid>/environ`, etc.). Deferred to Phase 1B because hooks are the use case.
- **Multi-model Consensus (Phase 1C).** `model_roles` becomes a list of two or three. `quorum-lippa-client::wire` types already support `Vec<String>` for roles. The `Review` type adds `per_model_findings` next to the aggregated `findings`; defaults to empty for Phase 1A archives.
- **Bearer-token auth swap (after Lippa M-ExternalAuth Phase 1 ships).** The `apply` method's `Bearer` arm goes from `Err(AuthError::BearerNotYetSupported)` to a one-line header builder. `quorum auth login --bearer` is added (~5 LOC) to fetch `extension_token` from the appropriate Lippa endpoint. Cookie path stays for one release cycle as fallback, then deprecated. The §4.7 cookie-lifecycle subsection becomes a note: "Bearer-authed requests carry no session cookie; renewal/logout semantics described here apply only to the cookie path."
- **Memory writes (Phase 2+).** `quorum-lippa-client::memory` module added. `quorum-core` acquires a `MemoryStore` trait; Phase-2 implementation calls `/api/v1/memory/propose`. The `Review` type is unaffected.
- **Privacy-mode redaction (Phase 2+).** Bundle assembly gains a redaction pass between content collection and submission. The Phase 1A deny-list (§4.3.2) becomes the trivial baseline; regex/AST redaction layers on top. Existing call sites unchanged.
- **Conventions write-back (Phase 1B).** `.quorum/conventions.md` becomes write-target as well as read-target. Phase 1A's read path treats the file as read-only; Phase 1B's write path goes through a state machine (`candidate` → `local_only` → `promoted_convention`) per `CLAUDE.md`.
- **Binary distribution (Phase 1B).** `cargo install quorum-cli`, GitHub Releases with prebuilt binaries, `brew install quorum` / `winget install quorum`. Out of scope for Phase 1A (developer-built only); Phase 1B with hooks is when distribution becomes user-facing.

The boundary that matters most: keep `quorum-core` independent of the HTTP layer, and keep cookie-specific behavior in §4.7 + `AuthMethod::apply`. Anything that grows from Phase 1A into 1B / 1C / 2+ stays on the right side of those boundaries.

---

## 12. Relationship to Roadmap

Phase 1A is the first work product in `accillion/quorum`. It does NOT depend on:

- Lippa's `M-ExternalAuth Phase 1` shipping. Phase 1A ships against cookie auth; Bearer is a typed-error stub for later swap.
- Lippa's `M-MCPLaunch` / `M-DeveloperPlatform`. Those are Phase 2+ on Lippa side.
- Any Lippa user-facing milestone.

Phase 1A DOES depend on:

- Lippa's `/api/v1/auth/login`, `/api/v1/consensus/sessions{,/...}` being live and stable on staging at recon time. CC-Recon-1, -2, -3, -9 verify.

Phase 1A is a hard prerequisite for:

- Phase 1B (interactive dismiss + git hook installer + CI auth + dismissals store + binary distribution).
- Phase 1C (multi-model Consensus).
- Phase 2+ (memory writes, privacy redaction, conventions write-back).
- Bearer-auth swap (when M-ExternalAuth lands).

If Phase 1A fails to ship: nothing in Phase 1B+ is unblocked. The entire downstream sequence waits.

---

*— end of spec v0.2 —*
