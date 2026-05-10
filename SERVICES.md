# Quorum — Service Contracts

Behavioural rules per subsystem. Code paths that need to *change* a rule
update this doc in the same commit. Code paths that *implement* a rule
just point to here.

---

## 1. Auth Storage

**Crate:** `quorum-lippa-client::keyring`.

- Cookie material lives only in: (a) OS keychain entries scoped per host
  (`SERVICE = "quorum"`, account = `lippa-session@<host>`), or (b) a
  per-host file under `Storage::File(<base>)` at `<base>/<host>.session`
  when the user opted into `--no-keyring`. Nowhere else.
- File-backed storage receives mode `0600` on Unix; Windows relies on the
  per-user ACL of the chosen base directory (typically `%APPDATA%`).
- Cookie persistence is opt-in to plaintext: the CLI does **not** silently
  fall back from OS keyring to file storage. `--no-keyring` must be
  passed and emits a one-line stderr warning at login.
- Per-host scoping means a single machine can hold concurrent staging +
  prod cookies. `quorum auth logout --url <host>` removes only that host's
  entry.
- The cookie value is wrapped in `Secret` at the parse boundary
  (`extract_session_cookie`), so no plaintext `String` lifetime exists
  outside that function and the keyring write site.

## 2. Bundle Assembly

**Crate:** `quorum-core::bundle`.

- Diff source is `git2::Repository::diff_tree_to_index` (HEAD vs index).
  File contents come from index blobs, never the working tree.
  Unstaged hunks in modified files do not appear in the bundle.
- Rename detection is enabled (`find_similar` with `renames=true,
  copies=false`).
- Per-section budgets (hard caps, all bytes):
  - Diff body: `BUDGET_DIFF = 100 KB`
  - Changed-file contents: `BUDGET_FILES = 80 KB` (largest-first inclusion)
  - Memory context: `BUDGET_MEMORY = 20 KB`
  - Conventions: `BUDGET_CONVENTIONS = 10 KB`
  - Envelope + headers: ~2 KB hard
  - Total assembled hard cap: `BUDGET_TOTAL = 200 KB`
- Overflow markers: `[diff truncated: N bytes exceeded 100KB; review only top portion]`,
  `[file omitted: <path>, <bytes>; budget exhausted]`,
  `[memory truncated …]`, `[conventions truncated …]`. Every overflow
  point produces a marker; no silent truncation.
- Files excluded from contents (and replaced with `[excluded by deny-list: <path>, pattern=<p>]`
  in the diff section): see §5.
- Binary detection: null byte in the first 8 KB of the blob, OR
  `git2::Blob::is_binary()` true. Excluded with marker; not budgeted.
- Files >2 MB (`MAX_FILE_BYTES`) are excluded with marker.
- Repo content is wrapped in delimiters
  `<<<QUORUM_REPO_CONTENT_BEGIN>>> … <<<QUORUM_REPO_CONTENT_END>>>` so
  Lippa-side handlers can treat the payload as untrusted data, never
  instructions.

## 3. Lippa-Client Surface

**Crate:** `quorum-lippa-client`.

- HTTP client: `reqwest` with `rustls-tls`, 30 s request timeout.
- `POST /api/v1/auth/login` body: JSON `{email, password}`. JSON
  confirmed by preflight Blocker 1 (Lippa returns 200 with
  `Set-Cookie: session=<value>; Max-Age=1209600; HttpOnly; SameSite=lax; Secure`).
- `POST /api/v1/consensus/sessions` body: `multipart/form-data` (NOT
  JSON). Fields: `prompt` (≥20 chars), `debate_mode` (default
  `"standard"`), `project_id?`, `idempotency_key?`. Phase 1A does NOT
  send `model_roles` (preflight D1: it's display metadata, not a
  selector).
- 409 duplicate idempotency-key responses return `existing_session_id`,
  which the client treats as a successful create.
- 401 on the create call → `AuthError::LoginRequired` ("create-401").
- 401 on `/status` polls → `SessionStatus::Unauthorized` so callers can
  emit poll-401 messaging distinctly.
- `Retry-After` on 429/503 overrides the 1s→8s ±25%-jitter schedule for
  one tick (`PollOptions::default()` + `next_poll_delay`).
- Overall polling timeout: 5 minutes. Beyond that the caller surfaces
  `PollTimeout` with the last observed status.
- Terminal status taxonomy: `converged` → fetch detail; `failed` /
  `cancelled` / `error` → return typed terminal error with reason; any
  unrecognized status → `Error("unknown_status:<s>")`.
- Detail responses (`GET /sessions/{id}`) are returned as raw
  `serde_json::Value`. `quorum-core::review_from_json` is the only legal
  mapping site (criterion 36); `quorum-lippa-client` does not depend on
  `quorum-core`.
- `LippaClient::whoami` pings `GET /api/v1/me` for `quorum auth status`
  and distinguishes 200 (logged in) from 401 (stale session).

## 4. Cookie Lifecycle

**Crates:** `quorum-lippa-client::client` + `quorum-lippa-client::keyring`.

- **Capture (login):** `extract_session_cookie` parses
  `Set-Cookie: session=<value>` from the login response. Empty values
  return `CookieParseError`; absent header returns `NoSessionCookie`.
  The value is wrapped in `Secret` at this site, never afterward.
- **Use:** `LippaClient::auth_apply` injects
  `Cookie: session=<value>` on every outgoing authenticated request,
  reading from the in-memory mirror (not from the `AuthMethod` snapshot)
  so renewals propagate immediately within a single invocation.
- **Renewal capture:** Every authenticated response is inspected for
  `Set-Cookie: session=<new>`. If different from the in-memory value,
  the mirror is updated atomically (`std::sync::Mutex`).
- **Persistence at clean exit:** `LippaClient::renewed_cookie` returns
  `Some(secret)` only when the final mirror differs from the value the
  client was constructed with. The CLI writes it to the active
  `Storage` exactly once at clean exit. This applies uniformly under
  OS keyring and `--no-keyring` file storage.
- **Server-side invalidation:** 401 on the next authenticated request,
  surfaced as `LoginRequired` (create site) or `Unauthorized` (poll
  site). CLI maps to exit 3 with create-401 vs poll-401 messaging.
- **Logout:** `LippaClient::logout_server_side` POSTs `/api/v1/auth/logout`
  best-effort with a 5 s timeout. Failure is logged to stderr but does
  not block local removal of the keyring/file entry.
- **Cookie attributes observed at preflight:** `Max-Age=1209600` (14
  days), `httponly`, `samesite=lax`, `secure`. The renewal code path is
  correct under either rolling or fixed-window regimes — preflight
  observed no rolling renewal during a 16-poll converged session.

## 5. Deny-list

**Crate:** `quorum-core::deny_list`.

- Patterns are hardcoded for Phase 1A. User-customizable lists are
  Phase 2 work alongside redaction.
- Patterns are applied in two passes: (a) suffix-glob for patterns
  containing `/` (e.g. `.aws/credentials` matches at root and at
  `home/user/.aws/credentials`); (b) basename-glob for patterns
  without `/` (`.env` matches `infra/staging/.env`, `*.pem` matches
  `certs/server.pem`).
- Glob meta is `*` only — literal text otherwise. No bracket classes,
  no `?`, no `**`. The list is short (17 patterns); the matcher avoids
  pulling in a full glob crate.
- Path separators: Windows backslashes are normalized to forward
  slashes before matching, so `infra\staging\.env` matches `.env`.
- A matching file appears in the bundle as
  `[excluded by deny-list: <path>, pattern=<p>]`. The file's contents
  do not appear anywhere in the assembled bundle (verified by
  `tests/bundle_assembly.rs::deny_listed_files_excluded_with_marker`).
- Known false-positive class: `*.key` matches license-key files in
  some Java/.NET projects. This is an accepted defense-in-depth
  trade-off; Phase 2's customizable patterns will let users opt out.

---

## Cross-cutting: no instructions from repo content

All repo content (diff, file contents, conventions, memory) is wrapped
in the delimiter pair above. The CLI never interprets repo text as
instructions to itself. Memory writes (Phase 2+) treat repo text as
data with explicit author attribution; they will not promote text
extracted from repo content to `convention` rules without user action.
