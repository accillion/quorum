# Quorum — Backlog

Tickets surfaced mid-session that the active milestone doesn't cover.
Group by next-target version. Most-recent first.

---

## Surfaced by the 0.4.0 release and its prep (21 Nov 2026)

- **For Rolf: delete the `CARGO_REGISTRY_TOKEN` repository secret.**
  Nothing uses it now. The three `CARGO_REGISTRY_TOKEN` env lines in
  `publish-crates.yml` read the auth step's output
  (`steps.auth.outputs.token`), and Trusted Publishing published all of
  0.4.0 without it (run 44931991241). The secret still exists
  (`gh secret list`: last updated 2026-08-09) and holds the
  `quorum-ci-0.3.x` token, which expired 2026-11-07. Delete it in
  Settings → Secrets and variables → Actions, or with
  `gh secret delete CARGO_REGISTRY_TOKEN`.
- **`Cargo.lock` is gitignored** (`.gitignore:4`). Release builds and
  `cargo publish` therefore resolve every dependency fresh at tag time,
  and the lockfile packaged into `quorum-cli` (the one
  `cargo install --locked` uses) is whatever CI resolved then. A
  `cargo update -w` at release prep has nothing to commit. Committing the
  lockfile is the usual choice for a workspace that ships a binary, and
  it would make release builds reproducible. It is a decision for Rolf,
  not a release-prep change.
- **`tui/panels.rs::session_short` byte-slices a server string.**
  `&id[id.len() - 8..]` panics the TUI if a Consensus session id has a
  multi-byte character in its last 8 bytes. Lippa's ids are ASCII today,
  so the risk is low. The fix takes the last 8 `char`s, not a head cut, so
  it was not the AC 211a one-line swap and stayed out of that commit.
- **No public library enum is `#[non_exhaustive]`.** Every new error
  variant is a breaking change for `quorum-core` and
  `quorum-lippa-client` users. In 0.4.0, five enums gained nine variants.
  Marking the error enums `#[non_exhaustive]` (itself a one-time break)
  would let later releases add variants in a patch. It is an API decision.
- **CI still runs on `ubuntu-22.04`, which will retire the way `macos-14`
  did.** The first v0.4.0 tag run (44924050931) lost its
  aarch64-apple-darwin job to that retirement. Still on 22.04:
  - `release.yml`'s `plan`, `build-global-artifacts`, `host` and
    `announce` (`runs-on: "ubuntu-22.04"`);
  - the two Linux build targets that `dist plan` assigns by default,
    `x86_64-unknown-linux-gnu` on `ubuntu-22.04` and
    `aarch64-unknown-linux-gnu` on `ubuntu-22.04-arm`;
  - `publish-crates.yml`'s `publish-crates` job.

  Move them to `ubuntu-24.04` in a later PR, not the macOS runner fix (PR
  #9). The build targets move through `[dist.github-custom-runners]`. The
  fixed jobs are edited by hand in `release.yml`, which
  `allow-dirty = ["ci"]` permits. Building the Linux binaries on 24.04
  raises their glibc floor from 2.35 to 2.39. That is a support decision
  to make in that PR.

---

## Surfaced by the MFA login fix (21 Nov 2026)

- **Staging names its session cookie `session_stg`.** Lippa's
  `SESSION_COOKIE_NAME` is per environment (`config.py:478-485`): the
  default is `session`, and staging uses `session_stg`. Quorum hard-codes
  `session=` when sending (`AuthMethod::apply`, the `LippaClient::new`
  jar seed) and when capturing (`extract_session_cookie`). A
  *successful* staging login therefore fails with "server returned 200
  but no session cookie". Fixing it means storing the cookie name with
  its value, which changes the keyring and `--no-keyring` file format,
  so it is its own change.
- **`login_with_cookie` has no request timeout.** SERVICES.md §3 says
  "30 s request timeout", which holds for `LippaClient` and the
  two-factor `SignIn` but not for the one-shot login client. A
  non-interactive login against a server that accepts the connection
  and never answers hangs. Pre-existing; the 403-body read added by the
  MFA fix has its own 10 s bound.
- **Upstream ask: accept Lippa's `lpa_` personal access tokens on the
  routes Quorum calls.** This supersedes the parked M-ExternalAuth v0.2
  draft, none of which shipped: `optional_bearer_auth`, `AuthPrincipal`
  and `EXTERNAL_AUTH_BEARER_ENABLED` have no hits on Lippa `origin/main`
  `514b4f5`. Lippa already ships hashed, scoped, revocable PATs
  (`models/personal_access_token.py`). They are minted from a session
  (`routes/api_v1/pat.py:142-147`), so the second factor is already
  satisfied, and verified by `require_pat_auth`
  (`dependencies.py:372`). Its only consumer is the public MCP route
  (`routes/mcp_public.py:233`), and `require_auth`, used by
  `/api/v1/consensus/*` and `/api/v1/memory/propose`, reads the session
  cookie only (`dependencies.py:135-164`). Accepting a PAT there would
  give CI a revocable token, end cookie rotation, and make the
  interactive two-factor step optional. The Quorum side is the existing
  `AuthMethod::Bearer` seam. It is a Lippa-side change, filed as a Lippa
  spec, never made from this repo.

---

## Surfaced by DL Part 2 (21 Nov 2026)

- **`quorum link` writes `.quorum/config.toml` relative to the cwd.**
  `main.rs` passes `current_dir()` as the repo root, so `quorum link`
  run from a subdirectory writes `<subdir>/.quorum/config.toml`, which
  `quorum review` (rooted at the workdir) never reads. This is the same
  class as the `quorum convention` bug DL 2.5 fixed, but outside that
  fix's scope.
- **Prose inside the managed section, outside every block, is dropped.**
  A rewrite regenerates the section's scaffolding from the blocks
  alone, so text a user types between blocks, or between the open
  marker and the first block, disappears on the next promote or demote,
  with no diagnostic. The section is marked DO NOT EDIT, so this is
  documented behaviour. It is still user text, though, and a parser
  diagnostic, which the write refusal would pick up, would close it.
- **CLI promote checks the row's state before taking the store lock.**
  The lock cannot be held across `--from-editor`'s `$EDITOR`, so the
  state is read before the editor opens. If a promote of the same hash
  finishes in that window, this one still writes its block, replacing
  the other's body, and then ends on StateDrifted: no block is lost, but
  the file's body and `conventions.convention_text` disagree.
  Re-reading the state under the lock would close it. No test can drive
  the window without a new test seam, because a store open already
  waits out the lock.
- **A `--from-editor` body is discarded when the write is then refused.**
  The editor runs before conventions.md is read, so a structural refusal
  (DL 2.2) loses what the user typed. Checking the file once before the
  editor opens would avoid it; the check under the lock stays
  authoritative.

---

## Surfaced by DL Part 1 (21 Nov 2026)

- **Concurrent first opens duplicate the `.gitignore` line.**
  `gitignore::ensure_ignored` checks ignore status, then appends, and
  nothing serialises the two across processes. Four threads doing a
  first open of one fresh repo left more than one
  `.quorum/dismissals.sqlite*` line in 18 of 20 trials. Cosmetic, but
  it edits a file the user commits. One fix is to append while holding
  the migration's IMMEDIATE write lock.

---

## Surfaced by SEC-1 (20 Nov 2026)

- **Deny-list pattern coverage gaps.** Found alongside the three SEC-1
  egress fixes and deliberately left out of that PR. `deny_list.rs` does
  not cover `credentials.json`, `.git-credentials`, `*.tfvars`,
  `terraform.tfstate`, `.kube/config` or `*.jks`. Matching is
  case-sensitive, so on case-insensitive filesystems (macOS default APFS,
  Windows) `.ENV` or `ID_RSA` names the same kind of file and passes. The
  directory-prefix rule (`pat.ends_with('/')`) tests `path.starts_with`,
  so `.quorum/` matches only as the *first* path segment:
  `packages/app/.quorum/config.toml` is not denied. Each widening also
  needs README's NEVER-sent list updated.
- **A rename away from a denied name still sends the blob in the
  changed-file section.** SEC-1 drops the *diff* section when either side
  of a rename is denied. The changed-file section checks only
  `StagedFile::path`, the new name, so `git mv .env notes.txt` sends the
  former `.env` content as `notes.txt`. Arguably correct, since the file
  is no longer named like a secret and is about to be committed, but it is
  a deny-list semantics decision. Carrying the old path needs a new
  `StagedFile` field from `git.rs`.
- **No CI job runs `cargo test`.** `.github/workflows/` holds only
  `release.yml` and `publish-crates.yml`. The SEC-1 leaf-symlink tests
  skip loudly on Windows without Developer Mode and are authoritative
  only on Unix, so today they run only when someone runs the suite on
  Linux or macOS. An ubuntu job running `cargo test --workspace` (with the
  libdbus apt step, see the v0.4 CI item below) would make them, and the
  rest of the suite, a gate.
- **`auth login|logout|status --url` accept plain http.** These take a
  *user*-supplied URL rather than a repository-chosen one, so they are
  outside SEC-1. But `auth login --url http://host` sends the password in
  cleartext. Consider applying `validate_base_url` there too.

---

## Closed in v0.3.3 (originally planned for v0.3.1)

- ~~**Stale `--help` text in `convention` subcommand group.**~~ Closed by WI-5 in the v0.3.1 fixes payload (landed at `aed4aa1`); shipped to users in v0.3.3.
- ~~**Cross-process `OsKeyring` round-trip test.**~~ Closed by WI-1 in the v0.3.1 fixes payload; negative-control verified failing on unfixed `aed4aa1` (would have caught BUG 1 pre-v0.3.0).

---

## Next-target: v0.4 (or later)

- ~~**Trusted Publishing is unproven live (20 Nov 2026).**~~ **CLOSED by the v0.4.0 release.** In run [`44931991241`](https://github.com/accillion/quorum/actions/runs/44931991241), `custom-publish-crates / publish-crates` exchanged its OIDC token through `rust-lang/crates-io-auth-action@v1`, then published `quorum-core`, `quorum-lippa-client` and `quorum-cli` 0.4.0. Every step succeeded, including the token-revoking post step. The now-unused `CARGO_REGISTRY_TOKEN` secret is its own item at the top of this file. Rollback, if OIDC ever breaks: mint a fresh crates.io token, set it as the secret, and point the three `env: CARGO_REGISTRY_TOKEN:` lines at `${{ secrets.CARGO_REGISTRY_TOKEN }}`.

- **CI: explicit system-dependency declaration.** Both `release.yml` and `publish-crates.yml` currently rely on a manual `Install libdbus on Linux` apt step because cargo-dist 0.31 ignores `[dist.dependencies.apt]` in `dist-workspace.toml`. The duplication caused the v0.3.1 → v0.3.2 → v0.3.3 cascade (v0.3.1 added the manual step in only `release.yml`; v0.3.2 needed `allow-dirty = ["ci"]` to silence cargo-dist's self-check on that file; v0.3.3 added the same manual step to `publish-crates.yml` after `cargo publish --verify` re-compiled `libdbus-sys`). A future cargo-dist upgrade (or a `dependencies` config syntax change) should reduce this to a single declarative config entry. Until then, any new workflow file that runs `cargo build` / `cargo publish --verify` on Linux needs its own libdbus apt step. Tracking item — revisit at next cargo-dist bump.

- **Shared file+SQL orchestrator helper** (Phase 1C Stage 5; narrowed by DL Part 2). The block-set decision is shared since DL 2.1: all four promote/demote call sites use `conventions::build_block_set`, so there is one policy for blocks a write does not own. The rest of the sequence is still duplicated between `commands/convention.rs` (CLI orchestrator) and `tui/mod.rs::tui_promote/tui_demote` (TUI orchestrator): take the store lock, read, `check_writable`, parse, render, `atomic_write`, the AC 175 seam, commit through the guard. Moving it into a `quorum-core::conventions` helper is still open. It was deliberately left out of DL Part 2.

- **TUI pre-flight orphan warnings** (Phase 1C Stage 5; narrowed by DL Part 2). Structural diagnostics are no longer silent in the TUI: since DL 2.2 they refuse the write, and the refusal shows as the status-bar error. Still open: the orphan warnings from the AC 168 pre-flight (file block with no row, row with no block), which the CLI prints to stderr and the TUI drops. A library helper `pre_flight_check() -> Vec<String>` callable from both would close it; render them as transient status-bar messages on the TUI, and suppress them on `--hook-mode=*`, consistent with the Q14 lean.

- **Q15 inside-vs-outside-fence detection** (Phase 1C Stage 4 — deferred). Stage 4 simplified to warn-on-any-pre-flight-discrepancy. If user feedback ever surfaces the distinction mattering, add body-text comparison against `conventions.convention_text` + `--yes` gating for inside-fence modifications, per the v1.1 spec §10 Q15 lean.

- **Auto-promote stderr end-to-end integration test** (Phase 1C Stage 1 — deferred). The `quorum: dismissal <short-hash> auto-promoted ...` message + `--hook-mode=*` suppression has constituent-part test coverage (TransitionEvent shape via AC-135 test; formatter exercised at call site) but no dedicated end-to-end integration test running `quorum review` against a synthetic Lippa fixture. Stage 3's stderr work covered parser diagnostics on `list --orphans`, not this surface; the gap persists.

- **`review.rs` MemoryStore double-open consolidation** (Phase 1C Stage 2 — deferred). Bundle assembly and the dismissals-filter site each open the SQLite store independently per `quorum review` invocation. Consolidate into one handle threaded through the pipeline. Stage 4 design-judgment, not Phase 1C blocker. Since DL Part 1 the tracked-DB warning actually fires, once per open, so a review in a repo with the DB committed prints it twice.

- **Transition-log in-memory cache for TUI body pane** (Phase 1C Stage 5 — perf optimization). `Command::LoadTransitions` fires per `j`/`k`/`g`/`G` cursor move in the dismissal-history view. Each query is an indexed `state_transitions` read by hash — fine for typical histories, but a small per-hash cache keyed in the TUI app state would avoid repeated reads when scrolling. Add only if perf data demands it.

---

## Phase 1D / v0.4 candidates from production dogfood (15 May 2026)

### Quorum-side (v0.4)

- ~~**Bundle budget rethink for real repos.**~~ **CLOSED by v0.4 Stage 1.** Resolved as "smarter prioritization" rather than a raised cap: changed-file inclusion is now priority-scored (class, changed-before-context, hunk count, size ascending) instead of largest-first, and `[bundle] total_budget_kb` makes the cap tunable 100..=1024 for users who do want to raise it. A 25-commit dogfood range on this repo now evicts `HISTORY.md` (52 KB) last instead of admitting it first. See SERVICES.md §2.1.
- **Archive schema: model name normalization.** `model_names` uses vendor names (claude, google, openai); each finding's `supported_by` uses model names (gpt-4o, gemini-pro, claude-sonnet). A consumer of the JSON can't reliably join the two.
- **Archive schema: `final_agreement_score` semantics.** Score reported as 1.0 contradicts per-finding `supported_by` counts of 2-of-3. Either the score is effectively always 1.0, or it measures something other than what a reader assumes. Reconcile or document.
- **Archive schema: `dismissals_applied` accuracy.** Currently counts only dismissals that fired this run, not dismissals loaded from the store. Re-run archives report 0 even when a dismissal sits in the DB. Misleading observability signal.
- **`convention list` side effect.** Writes a `.gitignore` entry (`.quorum/dismissals.sqlite*`) on first run — surprising for a read command. Move to an explicit step in `link` / `install`, or announce.
- **Linux CI keyring round-trip test enablement.** The cross-process OsKeyring test added in v0.3.1 (`7cb7d46`) is `#[cfg_attr(target_os = "linux", ignore)]`. Enable on runners with `gnome-keyring` / `kwallet` available, or wire into a dedicated keyring-CI job.

### Upstream / Lippa-coordination (tracked here for visibility; requires Lippa team engagement)

- **BLOCKER for the memory wedge: Lippa consensus non-determinism on identical input.** Dogfood showed four runs on the same staged diff producing *disjoint* finding sets. Quorum's identity hash `title + source_type + sorted_models` can never match if titles vary. Without resolution, Phase 1C's whole machinery is inert by construction. Coordinate with Lippa team.
- **Finding schema redesign: file path + line range + description + suggested fix per finding** in the archive schema. Currently findings carry only a title and metadata; the body is a restatement of confidence/supported-by. Prerequisite to a stable identity hash. Coordinate with Lippa team.
- **Filter affirmation findings; assign severity meaningfully.** Several dogfood "findings" were statements that something is fine ("Sentry logging sufficiently addresses observability needs"). All 8 findings across 4 runs returned `severity: medium`. Lippa-side schema and prompt work.
- **Task-spec-aware review.** Dogfood open question: extend the consensus prompt to take a task description / ticket / requirements alongside the diff. Strategic; joint Quorum/Lippa scoping.

---

## Deferred from v0.4 Stage 1 (15 Sep 2026)

- ~~**`truncate_with_marker` can panic on multi-byte UTF-8.**~~ **CLOSED
  by AC 211a in the 0.4.0 release prep (`6842955`).** The cut now goes
  through `truncate_at_codepoint_boundary`. ASCII output is
  byte-identical, so no pinned marker count moved. The same swap fixed
  the 16-byte HTML sniff in `exit.rs::classify_http_status`.
- **`.yml` / `.yaml` / `.sql` / `.sh` carry no class of their own.** They
  fall through `class_rank` to `Source` (rank 0), so a changed CI workflow
  outranks changed tests and docs. That is faithful to the spec's §4.2
  table, and defensible — they were changed — but the dogfood probe makes
  it visible (`.github/workflows/release.yml` sorted above every test
  file). Worth a spec decision in a later stage, not a CC call.
- **One-hop resolution does not cover Python or Go.** The spec's specifier
  list (`./`, `../`, `crate::`, `super::`) is JS/TS and Rust only, so
  `.py` and `.go` changed files contribute config-allowlist candidates but
  no import neighbours. Deliberate scope, recorded so it is not mistaken
  for a bug.
- **`include` globs are independent of `related_files`.** AC 190 requires
  `related_files = false` to restore the v0.3.3 candidate set exactly;
  that holds at the default `include = []`. A user who sets both
  `related_files = false` and a non-empty `include` still gets the
  included files, on the reading that an explicit glob is a direct
  instruction rather than part of the automatic discovery AC 190 disables.
  Flagged for confirmation.
