# Quorum — Milestone History

Chronological log of closed milestones. Most-recent first.

---

## Phase 1B Stages 1–4 + 5a — Dismissals, TUI, hooks, CI auth, distribution scaffolding ✦ 2026-05-11 → 2026-05-12

**Spec:** `specs/Quorum-Phase1B-Spec-v1_0.md`.
**Preflight:** `specs/Quorum-Phase1B-Preflight-notes.md` (8 documented divergences D1–D8; adjudication appended after Rolf review).
**Status:** Stage 5a complete; Stage 5b (live `cargo publish` + `v0.2.0` tag push + release artifacts) Rolf-gated.

### Commit graph

```
f5b92ae feat(render): markdown header dismissed-count suffix (AC 53)
725d323 chore(release): cargo-dist init + workspace config + release.yml
d00affb chore(cli): build.rs GIT_SHORT_SHA + version string
b3b541f feat: Phase 1B Stage 4 — non-interactive auth + QUORUM_LIPPA_SESSION + security README
0770936 feat: Phase 1B Stage 3 — hook installer + split pre-commit/pre-push templates
fc6cb31 feat: Phase 1B Stage 2 — TUI (ratatui + crossterm) with dismiss/undo + restoration
28da5ec feat: Phase 1B Stage 1 — dismissals foundation + DiffSource + archive v2
83ea3c1 chore: license Apache-2.0; publish metadata gaps
f4d3ef6 fix(client): use reqwest cookie_store for session lifecycle
a0b84e5 recon: preflight notes — divergence-gate adjudication D1-D8 + memory/propose schema
```

### What shipped

- **Dismissals foundation (`quorum-core::memory`).** `MemoryStore` trait sync, INSERT-only `dismiss`, idempotent `record_seen` per `(hash, session_id)`, idempotent `delete`. `LocalSqliteMemoryStore` opens `.quorum/dismissals.sqlite` with WAL+fallback, runs the v1 migration, applies pragmas (`foreign_keys=ON`, `busy_timeout=5000`), auto-writes `.gitignore`, warns on tracked DB. Three-input `finding_identity_hash` (title + source.type + sorted models) per D2/D3 adjudication; `body_opener` dropped (Lippa's wire format exposes no rich cluster body), P39 dropped (no `file` field).
- **DiffSource enum (`quorum-core::git`).** `StagedIndex` (Phase 1A behavior) | `CommitRange { base, head }`. Two diff helpers; bundle assembly is source-agnostic.
- **Archive v2.** `schema_version: 2`, `dismissals_applied: u32`, `suppressed_findings[]` (hash + title + source_type + reason + dismissed_at; never note text). Filenames stay `<ISO>.json` for direct invocation; pre-push uses `<push-start-ISO>.tuple-<N>.json` per spec §4.10.2 P28.
- **TUI (`quorum-cli::tui`).** ratatui+crossterm; pure `AppState` + `Command` event-handler; list + body + status bar layout; reason-picker + free-text-note modals with 2KB cap, control-strip-with-tab-collapse, embedded-newline rejection; unbounded undo stack; `TuiSession` RAII guard + panic-hook chain restores terminal on every exit path.
- **Hook installer (`quorum-cli::hooks`).** `quorum install --hook=<kind>` / `quorum uninstall --hook=<kind>` writing v1.0 §4.5.2/§4.5.3 shell templates. Idempotency marker scan (first 5 lines); `repo.path().join("hooks")` discipline; `0o755` on Unix. Pre-push stdin parser per D4/D5 adjudication: `(delete)` literal triggers deletion-skip (not zero-sha), `refs/tags/*` triggers tag-skip; new-branch resolves base via merge-base against `origin/HEAD` with tip fallback; standard/force-push runs `CommitRange`; per-tuple archives at `<push-start-ISO>.tuple-<N>.json`.
- **Non-interactive auth (`commands::auth`, `commands::review`).** `quorum auth login --non-interactive` reads `QUORUM_LIPPA_EMAIL` + `QUORUM_LIPPA_PASSWORD`, both wrapped in `Secret` at env-read site; password dropped immediately after `login_with_cookie`. `quorum auth status --show-session [-y]` prints the cookie on its own stdout line (no labels, pipe-friendly) after y/N confirm. `QUORUM_LIPPA_SESSION` consumed directly by `quorum review`; precedence note fires only in interactive review, suppressed under `--hook-mode=*` per P31.
- **Lippa client cookie path (`f4d3ef6`).** Discovery during Stage 1 work that Lippa's edge rejects manually-attached `Cookie:` headers (D7). Switched `LippaClient::new` to `reqwest::cookie::Jar` + `cookie_provider(jar)`; `auth_apply` is a near-noop for Cookie mode. Patch-level bump to lippa-client 0.1.1 inline (folded into 0.2.0 at the Stage 1.3 metadata commit).
- **Distribution scaffolding (`d00affb`, `725d323`).** `build.rs` emits `cargo:rustc-env=GIT_SHORT_SHA=<sha>` from `git rev-parse --short HEAD` with `unknown` fallback (crates.io tarball builds); clap version is `concat!(CARGO_PKG_VERSION, " (", GIT_SHORT_SHA, ")")`. `dist-workspace.toml` pins `cargo-dist = "0.31.0"`, installers = shell/powershell/homebrew/msi, 5 targets per §4.8.3. `.github/workflows/release.yml` rendered; aarch64 cross-toolchain handled via cargo-dist's `matrix.packages_install` (release.yml:141).
- **License + publish metadata (`83ea3c1`).** MIT → Apache-2.0 across workspace (LICENSE + Cargo.tomls); per-crate `readme` / `keywords` / `categories`; `version = "0.2.0"` declared on `quorum-cli`'s path-deps so the publish dry-run resolves.

### Live verification

- **D7 cookie fix** confirmed end-to-end against `app.lippa.ai` via `whoami_probe` and `session_env_probe` scratch binaries: `Secret`-wrapped cookie carried through the jar → `/api/v1/me` returns `{email: "rolf@lippa.com"}`. The AC-91 SESSION-env-var chain is fully live-verified.
- **D8 (Lippa edge rejects `project_id` with 403)** remains blocking for any live AC that requires project context (Stage 1 happy-path archive, Stage 3 pre-commit blocking semantics, Stage 4 keyring-end-to-end). MOCK coverage in the integration suites is comprehensive; live verification of project-bound ACs deferred to whenever D8 is resolved Lippa-side. Same shape as Phase 1A's ACs 17/22/25/30 deferral.

### Process learnings

- **Windows UAC heuristic on test-binary names.** Windows' loader refuses to launch any binary whose filename contains `install`, `setup`, `update`, or `patch` substrings without an admin elevation prompt — including unit-test binaries. The hook integration suite was renamed `hooks_install.rs` → `hooks_lifecycle.rs` during Stage 3 for this reason. Project convention going forward: avoid those substrings in `tests/*.rs` filenames; prefer `_lifecycle.rs`, `_pipeline.rs`, `_setup_flow.rs` (the latter trips the heuristic — `setup` is on the avoid-list), or similar. If a *production* binary name ever lands one of those substrings — e.g. a cargo-dist artifact named `quorum-install-windows.exe` — that becomes a real distribution concern, not just a test-naming issue; surface immediately.
- **MSRV bump to 1.81.** Workspace `rust-version` moved 1.74 → 1.81 in Stage 2 because `std::panic::PanicHookInfo` (used by the TUI restoration panic hook) stabilized in 1.81; the predecessor `PanicInfo` is deprecated as of 1.82. Current stable Rust is 1.95.0; nothing in CLAUDE.md or the v1.0 spec locked MSRV, and the supported-toolchain narrowing is cosmetic against the current stable floor.
- **`cargo publish --dry-run` semantics for path-deps.** Even with `version = "X"` declared on path-deps in `quorum-cli/Cargo.toml`, the dry-run fails at "no matching package on crates.io" until the dep crates have actually been published. The dependency-order publish sequence (`quorum-core` → `quorum-lippa-client` → `quorum-cli`) is mandatory; Stage 5a's rehearsal confirms the first two go through cleanly and documents the expected failure shape for the third.
- **Test isolation against the OS keychain.** Phase 1B's auth tests are hermetic against the host's real keychain by redirecting `APPDATA` (Windows) / `XDG_CONFIG_HOME` (Linux) to a tempdir and using `--no-keyring`. Worth adopting as the default test pattern for any future code that touches `keyring::Storage`.

### Stats

- **194 tests passing** across the workspace (Phase 1A close was 60; Phase 1B Stages 1–4 added 134; Stage 5a/6.1 added 3 render unit tests for AC 53 → **197 tests** at this entry's close).
- Workspace `cargo build --release`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check --all` all green throughout.
- 5 production crates / modules added or substantially modified: `quorum-core::memory`, `quorum-core::git::DiffSource`, `quorum-core::archive` v2, `quorum-cli::tui`, `quorum-cli::hooks`. The `quorum-lippa-client` crate gained no new public surface (the D7 fix was a transport-layer change).

### Pending for Stage 5b (Rolf-gated)

- `quorum-core` then `quorum-lippa-client` then `quorum-cli` actual `cargo publish` in dep order (the `dry-run` rehearsal confirms the metadata; Stage 5b adds `publish-jobs = ["./publish-crates"]` to `dist-workspace.toml`).
- `v0.2.0` tag push triggers the GitHub Actions workflow; produces SHA256SUMS + sigstore/OIDC attestation per AC 132.
- Post-release live verification of ACs 93 (`cargo install` round-trip from crates.io) + 94 (release artifacts present for all five targets) + 132 (attestation present).

---

## Phase 1A — Walking-skeleton CLI ✦ 2026-05-10 → 2026-05-11

**Spec:** `specs/Quorum-Phase1A-Spec-v1_0.md` (committed locally, awaiting v1.1 backfill).
**Preflight:** `specs/Quorum-Phase1A-Preflight-notes.md`.
**Status:** Implementation complete; live verification partial (smoke
tests of ACs 1, 8, 11, 13, 14 against the release binary). Full
happy-path live verification (ACs 17, 22, 25, 30) blocked on a
project-specific Lippa-side issue noted in preflight: sessions
submitted with this account's `project_id` fail in ~5s due to a
server-side seeding-context exception, while sessions without a
project_id converge cleanly.

**Shipped:**
- Cargo workspace with three crates: `quorum-cli`, `quorum-core`,
  `quorum-lippa-client`.
- `quorum-core`: `Review`/`Finding`/`Severity`/`FindingSource`,
  `review_from_json` (the only legal Lippa→Review mapping site),
  bundle assembly with HEAD-vs-index diff, per-section budgets,
  hardcoded deny-list, deterministic JSON archive.
- `quorum-lippa-client`: `Secret` newtype, `AuthMethod` (cookie wired,
  bearer typed-error stub for M-ExternalAuth swap), cookie login,
  per-host OS keychain + `--no-keyring` file fallback, `LippaClient`
  with create-session (multipart), polling (1s→8s ±25% jitter, 5min
  timeout, `Retry-After` honored), fetch-detail, server-side
  best-effort logout, whoami ping, in-memory cookie auto-renewal with
  one persistent write at clean exit.
- `quorum-cli`: clap-based `auth login/logout/status`, `link
  --project/--show/--no-remote-url`, `review`/`review --json`. Exit-code
  taxonomy with create-401 vs poll-401 distinction. TTY detection on
  `auth login`.
- Five integration suites + fixture under `crates/quorum-cli/tests/`:
  `auth_flow.rs`, `bundle_assembly.rs`, `end_to_end.rs`, `render.rs`,
  `secret_redaction.rs`, `wire_mapping.rs`. Wire-format fixture
  `lippa_v1_session_detail.json` is the verbatim converged-session
  payload captured at preflight time (criterion 36).
- `SERVICES.md` (auth storage, bundle assembly, Lippa-client surface,
  cookie lifecycle, deny-list).
- 60 tests passing across all suites. `cargo clippy --workspace
  --all-targets -- -D warnings` clean. `cargo fmt --check --all` clean.
- Crate boundary verified by `cargo tree`: `quorum-core` has no
  `quorum-lippa-client` dep; `quorum-lippa-client` has no `quorum-core`
  dep; `quorum-core` has no `tokio` dep (criterion 36).

**Deferred (Phase 1B):**
- Interactive dismiss layer.
- Local dismissals.sqlite store.
- Git hook installer (`quorum install --hook=pre-push`).
- CI / non-interactive auth (env-var creds + `--non-interactive`).
- Binary distribution (cargo install / brew / winget).

**Deferred (Phase 2+):**
- Memory write-back to Lippa.
- Privacy-mode redaction (regex/AST first, local LLM second).
- Conventions write-back via dismissal state machine.

**Spec divergences applied inline** (full detail in preflight notes;
v1.1 spec backfill is Rolf's call post-session):
- D1: Lippa always-multi-model. `model_roles` is display labels.
- D2: Detail response is debate-shaped; `Finding` synthesizes severity.
- D3: `create_session` uses multipart, not JSON.

**Concept-spec re-validation:** Lippa's always-multi-model design with
agreement-cluster output matches the original Quorum concept spec
("Consensus across frontier models") more directly than the Phase 1A
spec did. v1.1 roadmap should drop Phase 1C as a separate milestone —
Phase 1A already delivers multi-model.

**Commits:**
- `86c8d3f` — recon: preflight notes (initial)
- `68ba169` — recon: concept-spec re-validation + determinism note
- `3b67ad5` — feat: workspace + crate skeletons + unit tests
- `eaeae74` — test: five integration suites + wire fixture
- (this commit) — doc: CLAUDE.md / SERVICES.md / HISTORY.md /
  README.md updates closing Phase 1A

---

## Phase 0 — Reconnaissance ✦ 2026-05-10

**Deliverable:** `specs/Quorum-Recon-v0_findings.md`.
**Branch:** `GO-PAT-PARTIAL` — Quorum ships against cookie auth in
Phase 1A with the Bearer-token swap deferred to Lippa's M-ExternalAuth
milestone.
**Commit:** `0f33969`.
