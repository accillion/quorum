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
- Per-section budgets are **derived at runtime** from `[bundle]
  total_budget_kb` (default 200, range 100..=1024) via
  `Budgets::from_total_kb`, not hard-coded. Every section scales by the
  same ratio, rounded down, with the ~2 KB envelope overhead held
  constant. At the 200 KB default they evaluate to the historic values:
  - Diff body: 100 KB (`BUDGET_DIFF`)
  - Changed-file + related contents: 80 KB (`BUDGET_FILES`)
  - Memory context: 20 KB (`BUDGET_MEMORY`)
  - Conventions: 10 KB (`BUDGET_CONVENTIONS`)
  - Envelope + headers: ~2 KB, constant
  - Total assembled hard cap: `total_budget_kb` (`BUDGET_TOTAL` = 200 KB
    default). Only the **total** is enforced by `assemble()`; the section
    budgets are maxima, not reservations, and deliberately oversubscribe.
- **Only the total is a hard error.** `assemble()` returns
  `BundleError::BundleTooLarge(bytes, cap_kb)`; sections overflow into
  their own markers instead.
- Lippa applies **no maximum** prompt size (Phase 1A preflight, Blocker 3):
  an oversized bundle is not rejected — it reserves credits, runs, and
  returns session status `failed`. The real ceiling is the narrowest
  context window in a model roster Quorum cannot see. Above the 200 KB
  default, `quorum review` therefore emits a one-line stderr warning
  naming that risk; it is suppressed under `--hook-mode=*`.

### 2.1 Changed-file ordering (v0.4 WI-1)

The changed-file section is **not** filled largest-first. Candidates are
ordered by a deterministic lexicographic key, most significant first:

1. **Class rank** — `class_rank(path)`, a pure function of the path
   string: `Source` (0), `Config` (1), `Test` (2), `Docs` (3). Evaluated
   most-specific-first; `*.toml` counts as config only at the repo root.
2. **Changed before context** — every file in the diff precedes every
   unchanged context file *of the same class*, including when the changed
   file has `hunk_count == 0`.
3. **Hunk count, descending**, capped at 99. Sourced from libgit2's own
   `Diff::foreach` hunk callback; deleted and binary files carry 0.
4. **Size ascending** — so a tie admits more files rather than fewer.
5. **Path**, lexicographically — ordering is fully deterministic.

Size is a tiebreak and never a primary key: this is what stops two large
markdown files evicting the code under review.

The ordering actually used is emitted as a `## File inclusion order`
block (path, class, hunk count, bytes, origin, included/omitted) so it is
auditable from the archived prompt without re-running the review.

### 2.2 File bodies carry line numbers (v0.4 AC 182a/182b)

Every file body in the changed-file section is emitted with 1-based line
numbers in a fixed `%6d | ` gutter — changed and context files alike. The
gutter costs exactly 9 bytes per line (verified against this repo) and is
charged against the section budget before a file is admitted. The section
preamble states that the numbers are not part of the file, and that
`(unchanged, context)` files are numbered against the working tree at
HEAD rather than against the diff.

### 2.3 Related (unchanged) files (v0.4 WI-2)

Two bounded sources of unchanged context, both reported, both off when
`[bundle] related_files = false`:

- **Config allowlist.** A changed file's extension implies its repo-root
  config files (`.ts/.tsx/.js/.jsx/.mjs/.cjs` → eslint + tsconfig +
  package.json; `.rs` → Cargo.toml; `.py` → pyproject.toml + setup.cfg;
  `.go` → go.mod). These enter at `Config` rank even though unchanged.
  An allowlist hit must be **tracked at HEAD**: an untracked or
  gitignored `package.json` beside the repo root is never offered.
- **One-hop neighbours.** First-party relative specifiers in each changed
  file, resolved textually and conservatively: JS/TS `./` and `../`
  specifiers against a fixed extension-candidate order, and Rust `mod x;`,
  `use crate::…`, `use super::…`. **One hop only** — neighbours of
  neighbours are never followed, which is structural: only changed files
  are ever scanned.

Rules that apply identically to related files: the deny-list, the
`MAX_FILE_BYTES` ceiling, and the binary check. Related files are read
from the **working tree**, must resolve to a path **tracked at HEAD**, and
may never leave the repository root — a specifier that climbs out is
dropped, never clamped. At most `related_file_max` (default 12, range
0..=64) are offered; when the cap binds it is reported both on stderr and
in the inclusion-order block.

`[bundle] include` globs are matched against repo-relative paths by a
hand-rolled matcher (`*`, `**`, `?`) — no glob crate is in the dependency
set and CLAUDE.md forbids adding one. Matched files enter at the class
their path implies, never at a privileged rank. `include` is an explicit
user instruction and is therefore independent of `related_files`.

No file is ever included twice: a path in the diff is registered before
related discovery runs and can never be offered again.
- Overflow markers (the KB figure tracks the derived section budget):
  `[diff truncated: N bytes exceeded 100KB; review only top portion]`,
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

### 2.4 Working-tree reads never follow a link out of the repo (SEC-1)

git stores a symlink as a blob, so a committed `tsconfig.json ->
~/.aws/credentials` is a **tracked** path. The changed-file section is safe
by construction: it reads index blobs, and a link's blob is only its target
string. Every **working-tree** read (related files: allowlist, one-hop,
`include`) goes through one gate, `resolve_working_tree_file`:

1. `symlink_metadata` on the path itself. Anything that is neither a
   regular file nor a symlink (directory, FIFO, socket, device) is refused.
2. `canonicalize`, which resolves links in every component. The result
   must `starts_with` the canonicalized repository root.
3. The resolved target must be a regular file.
4. If the path resolves anywhere other than itself, the **target's**
   repo-relative path must also pass the deny-list and be tracked. Those
   checks ran on the name, and the link chose the name.
5. The read uses the canonical path, so it never re-resolves a link.

A link to a tracked, non-denied file inside the repository is normal and
is followed. A refused path is never silent: it is recorded as
`FileExclusionReason::UnsafePath(<why>)` and printed as `excluded: <path>
(<why>)`. A path missing from the working tree is skipped silently, since
nothing was read. If the repository root cannot be canonicalized, related
discovery is off.

## 3. Lippa-Client Surface

**Crate:** `quorum-lippa-client`.

- HTTP client: `reqwest` with `rustls-tls`, 30 s request timeout.
- `POST /api/v1/auth/login` body: JSON `{email, password}`. JSON
  confirmed by preflight Blocker 1 (Lippa returns 200 with
  `Set-Cookie: session=<value>; Max-Age=1209600; HttpOnly; SameSite=lax; Secure`).
- **A refused login is classified, never a cookie source.** Only a 403
  body is read, at most 16 KiB within 10 s. It maps as follows:
  - `{"detail":{"error":"mfa_required"}}` → `AuthError::MfaRequired`;
  - `email_not_verified` → `EmailNotVerified`;
  - a string `detail` starting "Your account has been suspended."
    (Lippa sends no code for it) → `AccountSuspended`;
  - any other code under `detail.error`, `detail.code` or a top-level
    `error` → `LoginRejectedWithCode`, but only when the code is plain
    snake_case, since the CLI prints it.
  Everything else, any non-403 status included, is
  `LoginRejected(status)` as before, and nothing on that path reads
  `Set-Cookie`. The CLI's one `login_error` mapping gives each its own
  message, exit 3, on both login paths.
- `POST /api/v1/consensus/sessions` body: `multipart/form-data` (NOT
  JSON). Fields: `prompt` (≥20 chars), `debate_mode` (default
  `"standard"`), `project_id?`, `idempotency_key?`, `tier?`. Phase 1A
  does NOT send `model_roles` (preflight D1: it's display metadata, not
  a selector).
- **Quorum always sends `tier`.** Lippa runs a fixed three-model panel
  per tier (`low`, `medium`, `high`), defined on Lippa's side. A create
  without `tier` gets the admin setting `CONSENSUS_TIER_DEFAULT`, so
  leaving it out would let one Lippa setting silently switch every
  review's models. `quorum review` resolves the tier as `--tier`, else
  the `tier` key in `.quorum/config.toml`, else **`high`**, and sends it
  on every create, hooks included. Hook mode reads the config only;
  `--tier` with `--hook-mode` is refused. A config value other than the
  three is a config error at load (exit 2) that lists them.
  `SessionCreateRequest.tier` is `Option<String>` because the client
  can't name `quorum-core`'s `ConsensusTier`. `None` sends no field,
  and only library callers can choose it.
- **A 422 that names `tier`** (Lippa's `{"detail": "tier must be one
  of: …"}`, or a validation item whose `loc` contains `"tier"`) is
  `CliError::TierRejected`, exit 2. Its message says Lippa rejected the
  tier and shows what Quorum sent. Any other 422 body keeps the generic
  HTTP-status message.
- **The tier is recorded, never identity.** `Review.tier` holds the
  tier sent. The archive writes it as the last top-level key, `"tier"`,
  and the markdown header shows it beside the models as
  `(tier: high)`. With no tier (every archive before it existed) there
  is no key and the header is unchanged. Nothing reads archives back.
  `finding_identity_hash` and `normalize_title` never see the tier
  (v0.4 Stage 4, AC 226).
- 409 duplicate idempotency-key responses return `existing_session_id`,
  which the client treats as a successful create.
- 401 on the create call → `AuthError::LoginRequired` ("create-401").
- 401 on `/status` polls → `SessionStatus::Unauthorized` so callers can
  emit poll-401 messaging distinctly.
- `Retry-After` on 429/503 overrides the 1s→8s ±25%-jitter schedule for
  one tick (`PollOptions::default()` + `next_poll_delay`), **clamped to
  `max_delay` (8 s)**. The header comes from whoever answered (Lippa, a
  proxy, a captive portal), so it never sets a longer sleep. AC 29's
  "waits exactly N seconds" holds for N ≤ 8; above that the wait is 8 s.
- Overall polling timeout: 5 minutes. Beyond that the caller surfaces
  `PollTimeout` with the last observed status. The deadline holds
  while sleeping too: the CLI's `poll_until_converged` caps every sleep
  at the time left before it, so the deadline check always runs. (A
  `Retry-After: 100000000` once made `git push` sleep for years.) One
  in-flight request can still overrun the deadline by up to the 30 s
  request timeout.
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
  `Set-Cookie: session=<value>` from a 2xx login response, or from the
  verify response of a two-factor login (§4.2). Empty values
  return `CookieParseError`; absent header returns `NoSessionCookie`.
  The value is wrapped in `Secret` at this site, never afterward.
- **Use (jar-driven):** `LippaClient::new` seeds a `reqwest::cookie::Jar`
  with `session=<value>; Path=/` scoped to the parsed `base_url`. The
  reqwest client is built with `.cookie_provider(jar)`, so every outgoing
  request auto-carries the cookie. `auth_apply` is a near-noop for the
  Cookie arm (it still gates Bearer behind `BearerNotYetSupported`).
  Manual `Cookie: ...` header injection is no longer used: the Lippa
  edge (observed 2026-05-11 during Phase 1B preflight) rejects manually-
  attached cookies with HTTP 403; the jar path is the supported pattern.
  See Phase 1B preflight D7 for the discovery trail.
- **Renewal capture (jar + mirror):** reqwest automatically writes
  every response's `Set-Cookie: session=<new>` back into the jar, so
  subsequent outbound requests carry the renewed value without further
  code. In parallel, `capture_renewed_cookie` continues to scan response
  headers and update the in-memory `Mutex<Secret>` mirror — the mirror
  is what `LippaClient::renewed_cookie` reports for keyring persistence.
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

### 4.1 `base_url` is repository-chosen (SEC-1)

**Crates:** `quorum-core::config` (rules) + `quorum-cli::commands::review`
(enforcement). `base_url` comes only from the repository's own
`.quorum/config.toml`. There is no CLI flag and no env override, so a
cloned repository chooses where the session and the bundle are sent.

- **Scheme.** `validate_base_url` requires `https`. Plain `http` is
  accepted only on an explicit loopback host: `localhost`, `127.0.0.1`,
  `::1`. That list is literal: no `127.0.0.0/8`, no `*.localhost`, no DNS
  lookup. Anything else, including an unparseable URL, is
  `ConfigError::InsecureBaseUrl`. It is checked in `config::read`, so
  every command that loads the config exits 2 the same way, before any
  network. `config::write` checks too, so `quorum link` cannot write a
  config that later commands reject. Under `--hook-mode=*` this is a
  normal exit 2, which the hook template fails open on by default: the
  push proceeds and nothing is sent.
- **Keyring cookie.** Already safe: entries are host-scoped
  (`lippa-session@<host>`), so an unknown host loads `None`. The pin
  below does not apply to it.
- **Env-var cookie.** `QUORUM_LIPPA_SESSION` is not host-scoped, so it
  is used only together with the pin `QUORUM_LIPPA_BASE_URL`
  (`SESSION_PIN_ENV`). The pin never replaces `base_url`; it only
  vouches for it. The scheme rule alone does not protect the session,
  because a certificate for a domain the repository's author controls
  is free. `check_session_pin` compares hosts as the keyring does
  (`host_str`, case-insensitive, port ignored):
  - **pin set, hosts match:** the cookie is used, with no note.
  - **anything else** (the pin is unset, empty, not a URL with a host, or
    names a different host): refused with `CliError::Config` (exit 2).
    The message names `QUORUM_LIPPA_BASE_URL` and the host(s), and the
    server is never contacted. No mode warns and proceeds: under
    `--hook-mode=*`, the mode a hostile clone's config fires in on
    `git push`, nobody reads stderr. The refusal does not fall back to
    the keyring either, since setting the env var chose that session.
    Under a hook, exit 2 follows the template's policy: fail-open by
    default, so the push proceeds unreviewed and the error line still
    prints.

### 4.2 Two-factor login

**Crates:** `quorum-lippa-client::sign_in` (protocol) and
`quorum-cli::commands::second_factor` (prompting). The Lippa contract is
`routes/api_v1/mfa.py:217-261` and `services/mfa_service.py:293-335` on
Lippa `origin/main` `514b4f5`. Two-factor is per-user opt-in there.

- **Step 1.** A correct password for an enrolled account gets
  `403 {"detail":{"error":"mfa_required"}}` and a *pending* session
  cookie. That cookie has no `user_id`, so it reads as signed out, and
  Lippa expires it after 10 minutes. The client returns
  `AuthError::MfaRequired`.
- **Step 2.** `POST /api/v1/auth/mfa/verify` with JSON `{"code"}` or
  `{"recovery_code"}`, carrying the pending cookie. Answers:
  - `200 {"ok","method","recovery_codes_remaining"}` with the real
    `Set-Cookie: session=`;
  - `401 invalid_code`, which counts toward the same lockout as a wrong
    password;
  - `401 mfa_session_expired`;
  - `429`.
- **One client, one jar.** `SignIn` owns one reqwest client (fresh
  `cookie::Jar`, 30 s timeout, no redirects) for both steps. Step 1 is
  `login_on(&client, req)`, the same code `login_with_cookie` runs, so
  its refusals are the ones in §3. The jar is the only carrier of the
  pending cookie. No request sets `Cookie:` by hand, because the edge
  rejects that (§4, "Use").
- **The pending cookie is never stored.** It is never returned,
  printed, or written to the keyring, a file or an env var, and the jar
  is dropped with the `SignIn`. The session is taken from the verify
  response's `Set-Cookie` through `extract_session_cookie`. A 200
  without one is `NoSessionCookie`, never a fallback to the jar.
- **Prompt.** `Authentication code (or a recovery code): `, hidden,
  through `rpassword`. Six digits once whitespace is removed are sent as
  `code`; anything else is sent trimmed as `recovery_code`. An empty
  answer sends nothing and stops.
- **Three attempts.** At most `MAX_CODE_ATTEMPTS` = 3 codes in total.
  The first `invalid_code` says that wrong codes count toward the same
  lockout as wrong passwords. After the third the login stops.
- **Expiry, rate limit, other refusals.** On `mfa_session_expired` the
  CLI names the 10-minute window and says to run `quorum auth login`
  again; the password is never re-sent on its own. On `429` it reports
  and exits; `Retry-After` is not read, so nothing sleeps. Any other
  status stops with that status. Every two-factor failure exits 3.
- **Success.** The verified session is stored exactly as a
  password-only login stores its session (keyring, or the
  `--no-keyring` file). With 3 or fewer `recovery_codes_remaining` a
  note is printed.
- **No terminal, or `--non-interactive`.** The code is asked for only
  when stdin is a terminal. The dispatcher already refuses a non-TTY
  interactive login before any request. Otherwise both cases print
  "This account has two-factor authentication enabled. Use
  QUORUM_LIPPA_SESSION together with QUORUM_LIPPA_BASE_URL.": the
  session-plus-pin pair (§4.1) is the CI path for such an account.

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
  do not appear anywhere in the assembled bundle: not in the changed-file
  section (`deny_listed_files_excluded_with_marker`) and not in the diff
  body (§5.1, `sec1_staged_env_contributes_nothing_to_the_diff_body`).
- Known false-positive class: `*.key` matches license-key files in
  some Java/.NET projects. This is an accepted defense-in-depth
  trade-off; Phase 2's customizable patterns will let users opt out.

### 5.1 Deny-listed files are filtered per diff section (SEC-1)

**Crate:** `quorum-core::bundle` (`diff_section`).

The diff body is filtered as **whole per-file sections**, never line by
line. The unified diff is split on lines that begin with `diff --git `;
only a libgit2 file header can begin that way, because every content line
carries a one-character origin prefix (` `, `+`, `-`, …).

- A section's paths come from **its own header**: the `diff --git` line
  (git's C-quoting decoded, so `"a/caf\303\251/.env"` reads as
  `café/.env`) plus any `rename from|to` / `copy from|to` lines before the
  first `@@`. Never from a substring search of the body.
- **Both sides count.** A section is dropped if its old *or* new path is
  deny-listed, so a deletion of `.env` or a rename away from it sends
  nothing. An old path absent from the file list is still added to the
  exclusion summary and to `BundleResult::exclusions`.
- A dropped section is replaced **in place** by
  `[excluded by deny-list: <path>, pattern=<p>]`, in addition to the
  summary at the top of `## Diff`.
- **Fail closed:** a `diff --git` line from which no path can be read is
  replaced by `[excluded: diff section whose header names no parseable path]`.
- Kept sections pass through **byte-for-byte**. A content line that merely
  mentions `a/.env` is never touched.

Pinned by the `sec1_*` diff tests in `tests/bundle_assembly.rs`, which
stage real changes in a real repository; synthetic `unified:
String::new()` fixtures cannot exercise this path.

## 6. Dismissals store (Phase 1B)

**Crate:** `quorum-core::memory`.

- **Location:** `<repo_root>/.quorum/dismissals.sqlite`. Auto-created
  on first `quorum review`; auto-gitignored via `.gitignore` write
  on first open. Stderr warning if any store file (`.sqlite`, `-wal`,
  `-shm`) is tracked by git, since its free-text `note` column must
  not be shared. Tracking is tested by index membership, never by
  status flags: a committed, unmodified file has the empty status set.
  The warning says a `.gitignore` entry does not untrack the file and
  gives the `git rm --cached` command for the tracked files. When a
  store file is tracked, the `wrote .gitignore` note says it is not the fix.
  `<repo_root>` is the repository's working tree for every command:
  `quorum review` and `quorum convention` both use
  `Repository::discover` from the current directory, then `workdir()`,
  so a subdirectory reaches the same store. `quorum convention` honours
  `--quorum-dir` first, and outside a repository falls back to the
  current directory.
- **Pragmas applied at open:** `busy_timeout=5000` first (see
  DATABASE.md "Concurrency and migration"); `journal_mode=WAL` with fallback to
  `journal_mode=DELETE` + stderr warning on filesystems without
  shared-memory support (NFS / SMB / sandboxed); `synchronous=FULL`,
  so a commit survives power loss (DATABASE.md "Durability");
  `foreign_keys=ON`.
- **Schema v1:** `schema_version` + `dismissals` tables. CHECK
  constraints enforce `recurrence_count >= 1`, `reason IN
  {false_positive, intentional, out_of_scope, wont_fix, other}`,
  `reason != 'other' OR note IS NOT NULL`, `promotion_state IN
  {candidate, local_only, promoted_convention}`. Phase 1B writes only
  `'candidate'`; Phase 1C adds the state-transition logic.
- **`MemoryStore` trait is sync.** Phase 1B has one impl
  (`LocalSqliteMemoryStore`); Phase 2 layers an async outbox on top
  via the existing trait surface — the trait stays sync, the worker
  is what's async.
- **Two operations, not one (P01).** `dismiss()` is INSERT-only;
  duplicates return `MemoryError::AlreadyDismissed`. `record_seen()`
  is the filter-side bulk update — bumps `recurrence_count` +
  `last_seen_at` exactly once per `(hash, review_session_id)`.
  Conflating the two produced v0.1's upsert-vs-UNIQUE bug.
- **Default expiry:** `expires_in = Some(365 days)` on dismiss unless
  `quorum review --no-expire` is passed (writes `expires_at = NULL`,
  permanent). `load_active_dismissals` predicate is
  `WHERE expires_at IS NULL OR expires_at > now()` — permanent rows
  always returned.
- **Body truncation (P29).** `dismiss` truncates `finding.body` to
  2KB at a UTF-8 codepoint boundary before storing as `body_snapshot`.
  Caller passes the full `&Finding`; truncation is the storage
  layer's responsibility.
- **Note validation.** Free-text notes ≤ 2KB UTF-8 bytes;
  embedded `\n`/`\r` rejected (single-line modal); control chars
  other than `\t` stripped; empty note rejected when
  `reason == Other`. Trait + TUI layers both enforce.
- **Finding identity hash (3-input, post-D2/D3 adjudication).**
  SHA-256 over `title || 0x1F || source.type || 0x1F || sorted_models`
  with model elements joined by `0x1F`. `body_opener` and P39 (`file`
  swap) were dropped: Lippa's wire format exposes no rich cluster
  body and no per-cluster `file` reference, and `cluster_id` is
  session-random (preflight measured 0% stability across 5 reruns
  of the same diff). The residual cross-finding collision risk is
  guarded by an integration test asserting ≤ 2% collision rate on
  the v1.0 fixture set (`dismissals_filter::cross_finding_collision_rate`,
  measured at 0/190 = 0.0%).
- **Audit trail leak surface.** Archive's `suppressed_findings[]`
  carries only `finding_identity_hash`, `title_snapshot`,
  `source_type_snapshot`, `reason`, `dismissed_at`. Free-text
  `note` and `body_snapshot` stay in SQLite, never on disk
  outside the `.gitignore`-d store.

### 6.1 Conventions-promotion state machine (Phase 1C)

**Crate:** `quorum-core::memory` + `quorum-core::conventions`. Extends §6 without breaking the Phase 1B `MemoryStore` contract.

- **Schema v2.** Three new tables: `state_transitions` (append-only audit log of T1/T2/T3 transitions, FK to `dismissals` with `ON DELETE CASCADE`); `conventions` (one row per `promoted_convention` state, FK to `dismissals`); `schema_meta` (key/value, holds `forward_compat_min_version`). `finding_identity_hash` columns in both new tables are TEXT (matching Phase 1B v1's hex-encoded `dismissals.finding_identity_hash` for FK affinity). No backfill of `state_transitions` for pre-v2 dismissal rows.
- **Forward-compat gate at every DB open.** Binary checks `schema_version` against installed max (currently 2) and `schema_meta.forward_compat_min_version` against installed max. Either exceeds → exit 2 with upgrade message before any read or write. v0.2 binaries opening a v3+ DB hit this gate cleanly. `schema_version` is read as `MAX(version)`, and a table holding more than one row is refused outright (`MemoryError::SchemaVersionRows`, naming the file) before the migration writes anything; DATABASE.md "Concurrency and migration".
- **State machine: T1 / T2 / T3 / T4 / T5.** T1 (`candidate → local_only`, `auto_recurrence`) fires inside `record_seen()` during the post-Lippa dismissal-suppression filter pass — Phase 1B's existing call site, no relocation. T2 (`local_only → promoted_convention`, `explicit_promote`) and T3 (`promoted_convention → local_only`, `explicit_demote`) fire from `convention promote`/`demote`. T4 (candidate deletion via `convention prune`) and T5 (existing Phase 1B undismiss path) are audit-trail-silent: ON DELETE CASCADE evaporates would-be audit rows; `'deleted'` and `'explicit_undismiss'` are NOT in the `state_transitions` CHECK enum.
- **T1 concurrency: `rows_affected > 0` is primary.** `UPDATE dismissals SET promotion_state='local_only' WHERE ... AND promotion_state='candidate'` is the contention surface; the application code reads `rows_affected` to decide whether to insert the audit row. A no-op UPDATE produces no audit row. The `UNIQUE (finding_identity_hash, from_state, to_state, ts)` on `state_transitions` is secondary defense for the same-millisecond corner case.
- **`TransitionEvent` returned-value channel.** `record_seen` returns `Vec<TransitionEvent>` (owned, `Clone`); callers consume the vec to emit user-visible notes (e.g. CLI's `quorum review` stderr `quorum: dismissal <short-hash> auto-promoted to local convention (recurrence=N)`). No callback into `quorum-cli` from `quorum-core` — preserves the C3 boundary lock.
- **File-then-SQLite atomicity (T2 / T3).** The promote/demote path is: (0) take the store's write lock, `MemoryStore::begin_convention_write` (`BEGIN IMMEDIATE`); (1) read and parse current conventions.md, refusing a broken structure; (2) compute new block set; (3) write new content to `.quorum/conventions.md.tmp.<pid>.<nanos>`; (4) `fs::rename` temp → real (atomic on same volume on both Unix and Windows; made durable on Unix by a parent-directory fsync, see DATABASE.md "Durability"); (5) through the lock's guard, UPDATE state + INSERT/DELETE `conventions` row + INSERT state_transition; (6) COMMIT, which releases the lock. Crash between (4) and (6) leaves the file ahead of SQLite; the guard's drop rolls back. Recovery is idempotent re-promote: the WHERE clause on the state UPDATE filters by current state, and the writer's add-block helper detects "block already present for this hash" and upserts (no duplicate blocks emitted). `list --orphans` surfaces the pre-recovery drift if the user wants to inspect first.
- **Promote and demote are serialized through the store.** The lock in step (0) is held from before conventions.md is read until the commit, so a concurrent promote or demote, from another process or the TUI, waits on SQLite's busy timeout. It cannot render from the same read and overwrite this one's block. There is no file lock: the database is the single serialization point (DATABASE.md "Concurrency and migration"). CLI promote takes the lock after the body is resolved, because `--from-editor` may wait on `$EDITOR` for minutes.
- **Writer round-trip property.** `quorum-core::conventions::render_conventions_md` consumes `ParsedConventionsMd` (preserving `above_fence` + `below_fence` byte slices) + a `BlockToWrite` set, producing output bytes byte-equal to input on well-formed unchanged files. Line endings (LF / CRLF) detected from input bytes and preserved on output. First-line marker `<!-- quorum-managed-conventions-md v=1 -->` written only on freshly-created files (no marker emitted into existing files lacking it). A new fence on a file whose last line has no line break starts on a line of its own.
- **Blocks a write does not own are kept verbatim.** All four promote/demote call sites (CLI and TUI) build their block set with one function, `conventions::build_block_set`. Every block whose id is not the write's own goes through as `BlockToWrite::Verbatim`: its original bytes (`ParsedBlock::raw`), in its original order, whatever its layout. The write owns the blocks carrying its id. A promote replaces the first of them in place or appends; a demote removes them; extra copies of the owned id are dropped. Only the owned block is rendered canonically (`BlockToWrite::Canonical`). The managed section's own scaffolding, the two comment lines and the blank lines between blocks, is regenerated on every write.
- **A broken structure refuses the write; there is deliberately no bypass.** Every parser diagnostic is structural: a block marker the parser skipped (bad id, bad version, no close), a section with no close marker (`UnclosedManagedSection`), a close marker with no open-marker line (`UnopenedManagedSection`), or a second section (`DuplicateManagedSection`). Promote and demote run `conventions::check_writable` before rendering. On any diagnostic they refuse, naming the file, each problem's line and its repair, and write nothing to the file or SQLite. There is no `--force`: the write stays refused until the user repairs the file. The TUI returns the same refusal for its status bar; `list --orphans` never writes and only warns. The section markers count only on a line of their own (column 0, then a line break or end of file), so a file may quote them in prose; a column-0 marker inside a fenced code example still counts, as a second section.
- **Titles and bodies are neutralized at render.** `emit_block` puts the title on one line (each line break becomes a space). In the title and the body it then escapes `<!-- quorum:` / `<!-- /quorum:` as `<\!-- quorum:` / `<\!-- /quorum:`, which Markdown renders unchanged. A model-written title or a quoted body therefore cannot open or close a section or block. Only the rendering changes: `title_snapshot` keeps the title as the model wrote it, and the identity hash is untouched.
- **§6.2 promote-but-uncommitted bridge.** A `promoted_convention` row whose `.quorum/conventions.md` is uncommitted / dirty / missing renders inside the 20 KB memory section as a local-convention entry, NOT in the 10 KB conventions section. The fork is per-row at render time; SQLite state is unchanged. The committed-and-clean check is called once per bundle assembly (not per row) and reuses the existing Phase 1A `ConventionsState` value already loaded by `quorum review`. Zero new libgit2 calls.
- **TUI write path** (Stage 5) runs a thin `tui_promote` / `tui_demote` orchestrator with the same lock, refusal, block-set builder and file-first-then-SQLite ordering as the CLI, committing through the lock's guard. A refusal comes back as the status-bar error. The orphan warnings of the AC 168 pre-flight are not emitted from the TUI side (stderr doesn't render cleanly in the alt-screen surface); deferred to a future `pre_flight_check()` helper.
- **Test seam (release-safe).** `QUORUM_TEST_CRASH_AFTER_RENAME` env-var probe sits between `fs::rename` and the SQLite statements in the promote path; its panic drops the lock's guard, which rolls back. Zero release cost (one `AtomicBool` load + one env-var probe per promote invocation). `#[doc(hidden)]`-equivalent surface; not part of the public contract.
- **`[memory]` config keys.** `candidate_threshold` (int, 2..100, default 3) controls T1 firing point; `local_convention_bundle_cap` (int, 100..2048, default 500) caps per-entry bytes in the memory section render; `candidate_expire_days` (int, 0..3650, default 90) controls `prune` window with 0 meaning disabled.

## 7. Hook installer (Phase 1B)

**Crate:** `quorum-cli::hooks` + `quorum-cli::commands::hook_mode`.

- **Path resolution** uses libgit2 explicitly: `Repository::discover`
  then `repo.path().join("hooks").join(<name>)`. `repo.path()` is
  the literal `.git/` directory — correct under worktrees and
  custom `--git-dir`. We never use `path().parent()` inference.
- **Idempotency marker** is `# quorum-managed-hook`, scanned in
  the first 5 lines of an existing hook. Not found → exit 2; the user
  must remove the foreign hook manually. The marker line carries a
  version: `v3` since fail-closed covers a missing binary (below), `v2`
  in 0.4.1, `v1` before.
- **Install and uninstall never destroy an edited hook.** A managed
  hook is overwritten by `install` or removed by `uninstall` only when
  its content is exactly a template Quorum shipped for that hook, LF or
  CRLF: the current v3, or v2 or v1, kept verbatim in `hooks/legacy.rs`
  (never edit those bytes). That makes `quorum install --hook=<type>`
  both the idempotent re-install and the v1 / v2 → v3 upgrade. A marker with any other
  content means local edits: both commands exit 2 with a message saying
  so and leave the file untouched. There is no `--force`, the same rule
  as the conventions write paths (§6.1). The gate is one function,
  `hooks::refuse_unless_unedited`. Quorum never rewrites
  `.git/hooks` on its own; installed hooks change only when the user
  runs `quorum install`.
- **Unix mode** is `0o755` via `std::os::unix::fs::PermissionsExt`.
  On Windows the inheriting ACL of `.git/hooks/` governs execution;
  Git for Windows runs hooks via `sh.exe` regardless of NTFS
  executable bit.
- **Templates** are POSIX `#!/bin/sh`. Both consume `QUORUM_SKIP=1`
  (echo "skipped" + exit 0) and a missing binary (`command -v quorum`
  fails) before invoking `quorum review --hook-mode=<type>`. Under
  `fail-closed` a missing binary blocks, with one stderr line, `binary
  not on PATH; commit blocked (fail-closed)` plus the bypass. Under
  `fail-open` and `warn` it prints "binary not on PATH; skipping" and
  exits 0. Both honor
  `QUORUM_HOOK_POLICY ∈ {fail-open, fail-closed, warn}`:
  - `fail-open` (default): only exit 1 (high-severity findings)
    blocks.
  - `fail-closed`: **any non-zero exit** blocks: tooling (2), auth
    (3), and a crash, i.e. a panic (101) or a signal death (128+N).
    The v1 template blocked only on 2 and 3, so a crash went through.
    This also blocks network errors, which the CLI reports as exit 2;
    the Phase 1B spec's §4.11 table saying fail-closed passes them is
    superseded.
  - `warn`: nothing blocks regardless of exit code.
  Bypass for one operation: `env QUORUM_SKIP=1 git commit ...`
  or `env QUORUM_SKIP=1 git push ...`.
- **Hook policy is env-var-only.** Templates read
  `QUORUM_HOOK_POLICY` directly; the Rust binary emits the standard
  0/1/2/3 exit codes (Phase 1A stable taxonomy) and the shell
  template performs the policy translation. Config-file alternative
  is deferred to Phase 1B v2.
- **Pre-push stdin parsing (per D4/D5 preflight adjudication).**
  Each stdin line is `<local-ref> <local-sha> <remote-ref> <remote-sha>`
  whitespace-separated. Classification:
  - `local_ref == "(delete)"` (literal) → SkipDeletion with stderr
    note. The local-ref field is NOT a real ref name in this case;
    parsers must rely on the literal, not on zero-sha.
  - `local_ref` starts with `refs/tags/` → SkipTag with stderr note
    `quorum: tag push detected; skipping review (code review not
    applicable to refs/tags/*)`. Tag pushes carry no commit-range
    semantics for review.
  - `remote_sha` all zeros (40 `0`s) → NewBranch. Base resolved via
    `merge_base(local_sha, refs/remotes/origin/HEAD)`. If unresolvable
    (no upstream HEAD), review the tip commit only via
    `<head>^..<head>` with stderr note. If the lookup itself errors
    (`origin/HEAD` exists but the merge base can't be computed), the
    tuple is not reviewed: a stderr warning `new-branch base resolution
    failed for <ref>: …; not reviewed (exit 2)`, and it counts as
    `ToolingError` in the aggregation below. It used to be skipped
    without counting.
  - Otherwise → standard/force-push: `DiffSource::CommitRange {
    base: remote_sha, head: local_sha }`. Git's range semantics
    handle force-push correctly; no special marker in the stdin
    shape.
- **Per-tuple archive naming.** Pre-push writes
  `.quorum/reviews/<push-start-ISO>.tuple-<N>.json` per non-skipped
  tuple (1-indexed `N`, shared `push-start-ISO` across one push).
  All other invocation modes use the Phase 1A `<ISO>.json` shape.
- **Pre-push aggregation: a blocking verdict always wins.** Every
  non-skipped tuple is reviewed. A tuple whose review errors prints one
  stderr line naming its local ref (`quorum: pre-push review of <ref>
  failed (exit N): …`), counts as that error's exit (`AuthRequired` for
  an auth error, `ToolingError` otherwise), and the loop goes on. The
  push's exit (`hook_mode::aggregate_push_exits`) is `HighSeverity` if
  any tuple returned it; otherwise the highest of the rest, in the order
  `AuthRequired` > `ToolingError` > `Ok`. This deliberately departs
  from spec §4.5.5's literal "max severity". The numeric maximum
  (`Exit::max`, 0 < 1 < 2 < 3) ranks a transient 502 above
  high-severity findings, and the fail-open template blocks only on 1,
  so the push went through. The hard constraint is that a blocking
  verdict blocks. `Exit::max` itself is unchanged and ranks only the
  non-blocking rest.
- **A failed archive write never changes the exit code.** `quorum
  review` settles its exit (1 on high-severity findings, else 0) before
  writing `.quorum/reviews/<name>.json`. If the write fails (full disk,
  read-only mount, `reviews` not a directory) it prints `warning: could
  not write the review archive <path>: <error>` and the exit stays the
  review's, in every mode. `--json` still prints the buffer to stdout.
  As exit 2, the failure would let high-severity findings through the
  fail-open template.
- **No-auth in hook mode.** Under `--hook-mode=*`, the absence of
  any auth (no keyring entry AND no `QUORUM_LIPPA_SESSION`) is NOT
  a hard error — emits stderr `quorum: not authenticated — hook
  reviews are being skipped; run quorum auth login` and returns
  exit 0 so the shell template's fail-open path proceeds.

## 8. Diff source & bundle assembly extension (Phase 1B)

**Crate:** `quorum-core::git` + `quorum-core::bundle`.

- **`DiffSource` enum:** `StagedIndex` (Phase 1A behavior — HEAD vs
  index) or `CommitRange { base, head }` (Phase 1B). The bundle
  layer (`quorum-core::bundle::assemble`) is agnostic — receives a
  `StagedDiff` value regardless of source, budgets and deny-list
  identically.
- **`diff_for_source(repo, &DiffSource)`** dispatches:
  - `StagedIndex` → `staged_diff()` (Phase 1A, unchanged).
  - `CommitRange { base, head }` → `commit_range_diff()`. Both refs
    are resolved via `revparse_single` so callers can pass SHAs,
    branch names, tags, or arbitrary rev expressions (`HEAD~3`,
    `origin/main`, …). Diff is computed via `diff_tree_to_tree`;
    file blobs come from the `head` tree.
- **Symmetric-difference (`base...head`) is rejected** by the CLI
  `--range` parser. The bundle pipeline diffs `head` content against
  `base` tree; the spread-three-dot form is not a single base
  reference and produces ambiguous review semantics. The two-dot
  form is the supported shape.
- **CLI surface:** `quorum review --range <ref-expr>` for manual
  invocation; `quorum review --hook-mode=pre-push` dispatches per
  stdin tuple internally. `quorum review` without `--range` and
  without `--hook-mode=pre-push` uses `DiffSource::StagedIndex`
  (Phase 1A regression baseline preserved).

---

## Cross-cutting: no instructions from repo content

All repo content (diff, file contents, conventions, memory) is wrapped
in the delimiter pair above. The CLI never interprets repo text as
instructions to itself. Memory writes (Phase 2+) treat repo text as
data with explicit author attribution; they will not promote text
extracted from repo content to `convention` rules without user action.
