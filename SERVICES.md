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

## 6. Dismissals store (Phase 1B)

**Crate:** `quorum-core::memory`.

- **Location:** `<repo_root>/.quorum/dismissals.sqlite`. Auto-created
  on first `quorum review`; auto-gitignored via `.gitignore` write
  on first open. Stderr warning if the DB is currently tracked by
  git (its free-text `note` column must not be shared).
- **Pragmas applied at open:** `journal_mode=WAL` with fallback to
  `journal_mode=DELETE` + stderr warning on filesystems without
  shared-memory support (NFS / SMB / sandboxed); `synchronous=NORMAL`;
  `foreign_keys=ON`; `busy_timeout=5000`.
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
- **Forward-compat gate at every DB open.** Binary checks `schema_version` against installed max (currently 2) and `schema_meta.forward_compat_min_version` against installed max. Either exceeds → exit 2 with upgrade message before any read or write. v0.2 binaries opening a v3+ DB hit this gate cleanly.
- **State machine: T1 / T2 / T3 / T4 / T5.** T1 (`candidate → local_only`, `auto_recurrence`) fires inside `record_seen()` during the post-Lippa dismissal-suppression filter pass — Phase 1B's existing call site, no relocation. T2 (`local_only → promoted_convention`, `explicit_promote`) and T3 (`promoted_convention → local_only`, `explicit_demote`) fire from `convention promote`/`demote`. T4 (candidate deletion via `convention prune`) and T5 (existing Phase 1B undismiss path) are audit-trail-silent: ON DELETE CASCADE evaporates would-be audit rows; `'deleted'` and `'explicit_undismiss'` are NOT in the `state_transitions` CHECK enum.
- **T1 concurrency: `rows_affected > 0` is primary.** `UPDATE dismissals SET promotion_state='local_only' WHERE ... AND promotion_state='candidate'` is the contention surface; the application code reads `rows_affected` to decide whether to insert the audit row. A no-op UPDATE produces no audit row. The `UNIQUE (finding_identity_hash, from_state, to_state, ts)` on `state_transitions` is secondary defense for the same-millisecond corner case.
- **`TransitionEvent` returned-value channel.** `record_seen` returns `Vec<TransitionEvent>` (owned, `Clone`); callers consume the vec to emit user-visible notes (e.g. CLI's `quorum review` stderr `quorum: dismissal <short-hash> auto-promoted to local convention (recurrence=N)`). No callback into `quorum-cli` from `quorum-core` — preserves the C3 boundary lock.
- **File-then-SQLite atomicity (T2 / T3).** The promote/demote path is: (1) parse current conventions.md; (2) compute new block set; (3) write new content to `.quorum/conventions.md.tmp.<pid>.<nanos>`; (4) `fs::rename` temp → real (atomic on same volume on both Unix and Windows); (5) BEGIN SQLite transaction; (6) UPDATE state + INSERT/DELETE `conventions` row + INSERT state_transition; (7) COMMIT. Crash between (4) and (7) leaves the file ahead of SQLite. Recovery is idempotent re-promote: the WHERE clause on the state UPDATE filters by current state, and the writer's add-block helper detects "block already present for this hash" and upserts (no duplicate blocks emitted). `list --orphans` surfaces the pre-recovery drift if the user wants to inspect first.
- **Writer round-trip property.** `quorum-core::conventions::render_conventions_md` consumes `ParsedConventionsMd` (preserving `above_fence` + `below_fence` byte slices) + a new `BlockToWrite` set, producing output bytes byte-equal to input on well-formed unchanged files. Line endings (LF / CRLF) detected from input bytes and preserved on output. First-line marker `<!-- quorum-managed-conventions-md v=1 -->` written only on freshly-created files (no marker emitted into existing files lacking it).
- **§6.2 promote-but-uncommitted bridge.** A `promoted_convention` row whose `.quorum/conventions.md` is uncommitted / dirty / missing renders inside the 20 KB memory section as a local-convention entry, NOT in the 10 KB conventions section. The fork is per-row at render time; SQLite state is unchanged. The committed-and-clean check is called once per bundle assembly (not per row) and reuses the existing Phase 1A `ConventionsState` value already loaded by `quorum review`. Zero new libgit2 calls.
- **TUI write path** (Stage 5) calls `MemoryStore::commit_promote` / `commit_demote` directly with a thin `tui_promote` / `tui_demote` orchestrator that performs the same file-first-then-SQLite ordering. Pre-flight diagnostics (AC 168) are not emitted from the TUI side (stderr doesn't render cleanly in the alt-screen surface); deferred to a future `pre_flight_check()` helper.
- **Test seam (release-safe).** `QUORUM_TEST_CRASH_AFTER_RENAME` env-var probe sits between `fs::rename` and `BEGIN TX` in the promote path. Zero release cost (one `AtomicBool` load + one env-var probe per promote invocation). `#[doc(hidden)]`-equivalent surface; not part of the public contract.
- **`[memory]` config keys.** `candidate_threshold` (int, 2..100, default 3) controls T1 firing point; `local_convention_bundle_cap` (int, 100..2048, default 500) caps per-entry bytes in the memory section render; `candidate_expire_days` (int, 0..3650, default 90) controls `prune` window with 0 meaning disabled.

## 7. Hook installer (Phase 1B)

**Crate:** `quorum-cli::hooks` + `quorum-cli::commands::hook_mode`.

- **Path resolution** uses libgit2 explicitly: `Repository::discover`
  then `repo.path().join("hooks").join(<name>)`. `repo.path()` is
  the literal `.git/` directory — correct under worktrees and
  custom `--git-dir`. We never use `path().parent()` inference.
- **Idempotency marker** is `# quorum-managed-hook`, scanned in
  the first 5 lines of an existing hook. Found → overwrite
  (idempotent re-install). Not found → exit 2; the user must
  remove the foreign hook manually.
- **Unix mode** is `0o755` via `std::os::unix::fs::PermissionsExt`.
  On Windows the inheriting ACL of `.git/hooks/` governs execution;
  Git for Windows runs hooks via `sh.exe` regardless of NTFS
  executable bit.
- **Templates** are POSIX `#!/bin/sh`. Both consume `QUORUM_SKIP=1`
  (echo "skipped" + exit 0) and `command -v quorum` (echo "binary
  not on PATH; skipping" + exit 0, fail-open) before invoking
  `quorum review --hook-mode=<type>`. Both honor
  `QUORUM_HOOK_POLICY ∈ {fail-open, fail-closed, warn}`:
  - `fail-open` (default): only exit 1 (high-severity findings)
    blocks.
  - `fail-closed`: exit 2 (tooling) and exit 3 (auth) also block.
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
    `<head>^..<head>` with stderr note.
  - Otherwise → standard/force-push: `DiffSource::CommitRange {
    base: remote_sha, head: local_sha }`. Git's range semantics
    handle force-push correctly; no special marker in the stdin
    shape.
- **Per-tuple archive naming.** Pre-push writes
  `.quorum/reviews/<push-start-ISO>.tuple-<N>.json` per non-skipped
  tuple (1-indexed `N`, shared `push-start-ISO` across one push).
  All other invocation modes use the Phase 1A `<ISO>.json` shape.
  Push's overall exit code is `Exit::max` across all per-tuple
  exits (spec §4.5.5: max severity across tuples).
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
