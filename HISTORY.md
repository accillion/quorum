# Quorum — Milestone History

Chronological log of closed milestones. Most-recent first.

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
