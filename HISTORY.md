# Quorum — Milestone History

Chronological log of closed milestones. Most-recent first.

---

## Phase 0.2.1 — Release engineering: sigstore attestation + workflow-driven publish ✦ 2026-05-12

**Spec:** `BACKLOG.md` 0.2.1 entry (now closed and removed).
**Status:** **Closed at `v0.2.1`**. AC 132 flips PARTIAL → FULL via live sigstore attestation. AC 93 / AC 94 re-verified on the new version. All three crates published to crates.io; GitHub Release shipped with the same 19-asset shape as v0.2.0 plus per-target sigstore bundles.

**Public release:**
- crates.io: [`quorum-core`](https://crates.io/crates/quorum-core/0.2.1), [`quorum-lippa-client`](https://crates.io/crates/quorum-lippa-client/0.2.1), [`quorum-cli`](https://crates.io/crates/quorum-cli/0.2.1).
- GitHub Release: [`v0.2.1`](https://github.com/accillion/quorum/releases/tag/v0.2.1).
- CI run (release workflow, attempt 2): [`25744528118`](https://github.com/accillion/quorum/actions/runs/25744528118) — attestation + Release flow green; `custom-publish-crates` job failed, recovered out-of-band.

### Commit graph

```
<close>  doc: Phase 0.2.1 close — sigstore attestation LIVE + AC 132 FULL
5a658ef  fix(ci): drop nonexistent cargo publish --wait-for-publish flag
c052593  doc: track 0.2.1 scope in flight
f56ca69  doc: README polish for 0.2.1
d1229d8  chore(cli): bump versions to 0.2.1
286b1be  chore(release): enable cargo-dist sigstore attestation + publish-jobs
```

Tag `v0.2.1` points at `c052593`; not rewritten across the session.

### What shipped

- **cargo-dist sigstore attestation (`github-attestations = true`).** `dist-workspace.toml` opts in; cargo-dist 0.31 automatically emits per-matrix-target `permissions: { attestations: write, contents: read, id-token: write }` and an `Attest` step using `actions/attest-build-provenance@v3` with `subject-path: "target/distrib/*${{ join(matrix.targets, ', ') }}*"`. Sigstore bundles attached to each platform tarball verify via `gh attestation verify <file> --owner accillion` against Fulcio + RFC3161 timestamp authority.
- **Workflow-driven crates.io publish (`publish-jobs = ["./publish-crates"]`).** cargo-dist 0.31 emits the `custom-publish-crates` call site in `release.yml`; the called reusable workflow (`.github/workflows/publish-crates.yml`) is user-authored. Quorum's authoring publishes `quorum-core` → `quorum-lippa-client` → `quorum-cli` sequentially via `cargo publish -p <crate>`. `CARGO_REGISTRY_TOKEN` is plumbed via `env:` from `secrets: inherit`. Default cargo blocks until the new version is index-resolvable — no opt-in flag needed.
- **Version bump to 0.2.1.** Workspace `[workspace.package].version` + `quorum-cli`'s path-dep versions for `quorum-core` and `quorum-lippa-client`. `cargo publish --dry-run` clean for `quorum-core` + `quorum-lippa-client`; `quorum-cli` dry-run fails resolving `quorum-core ^0.2.1` against crates.io — expected for the chain-published workspace.
- **README polish (5 items).** crates.io version badge after H1; "Documentation" subsection linking docs.rs/{quorum-core,quorum-lippa-client,quorum-cli}; `.quorum/dismissals.sqlite` + WAL/SHM sidecars documented under "Files Quorum writes locally"; cargo-dist installer one-liners moved from `/releases/download/v0.2.0/` to `/releases/latest/download/` (no more per-release README bumps); `auth login Phase 1A` framing replaced with `By default ... see "Non-interactive auth" below`; changelog pointer to GitHub Releases.

### Live verification

- **AC 132 LIVE ✓** — `gh attestation verify quorum-cli-x86_64-unknown-linux-gnu.tar.xz --owner accillion` exits 0. `--format json` dump confirms: Fulcio cert issuer (`CN=Fulcio Intermediate l2, O=GitHub, Inc.`), OIDC issuer `token.actions.githubusercontent.com`, build signer `release.yml@refs/tags/v0.2.1`, source SHA `c0525932261c59bcf300c703cc42b9888d653e8d`, RFC3161 timestamp `2026-05-12T15:53:31Z` from `timestamp.githubapp.com`, SLSA provenance v1, `sourceRepositoryVisibilityAtSigning=public`. AC 132 PARTIAL → **FULL**.
- **AC 93 LIVE re-verified ✓** — `cargo install --root /tmp/q021-install --force quorum-cli` resolves `quorum-cli v0.2.1` from crates.io and produces a binary reporting `quorum 0.2.1 (unknown)` (`build.rs` GIT_SHORT_SHA fallback unchanged from v0.2.0).
- **AC 94 LIVE re-verified ✓** — downloaded `quorum-cli-x86_64-unknown-linux-gnu.tar.xz` from the v0.2.1 Release; computed SHA256 `92261a4b5d21954d9ebfcb4e9215eb07643c58da9805b2c49cc9a50507803fa9` matches the entry in `sha256.sum`.

### Recovery narrative

Two CI incidents this session, both root-caused and resolved without rewriting the v0.2.1 tag:

1. **Attempt 1 — repo-attestations 403 on every matrix target.** `actions/attest-build-provenance@v3` 403s on private repos when the org is on a billing plan below Team/Enterprise. Flipped `accillion/quorum` from private to public (`gh repo edit --visibility public`); attestations are free on public repos. Pre-flip precaution: `gh api .../actions/secrets` + `dependabot/secrets` + `environments` all returned `total_count: 0` (no secrets that would now be exposed) and the repo tree was confirmed not to carry sensitive material. The flip is permanent for this project — see learning #4 below.
2. **Attempt 2 — `cargo publish --wait-for-publish` parse error on the publish-crates job.** I had presented `--wait-for-publish` as a real cargo flag during preflight; it isn't. cargo 1.95.0 (and every preceding stable) errors with `unexpected argument`. Default `cargo publish` already blocks until the new version is index-resolvable, so the flag is unneeded. Fixed on `main` in `5a658ef` so the workflow ships clean from 0.2.2 onward; v0.2.1's three publishes were issued out-of-band by `cargo publish -p quorum-{core,lippa-client,cli}` from the local 0.2.1 tree (diff vs the v0.2.1 tag was only `.github/workflows/publish-crates.yml`, which cargo doesn't package — so the published tarballs are functionally identical to what the in-CI workflow would have produced).

The GitHub Release `v0.2.1` was created by attempt 2's `host` job before the publish-crates failure, so all 19 artifacts (5 platform tarballs × 2 + aggregate `sha256.sum` + MSI + `.sha256` + 2 installers + Homebrew formula + dist-manifest + source + `.sha256`) plus the cargo-dist-emitted sigstore bundles are present on the Release as-shipped.

### Process learnings

- **Attest `subject-path` is coupled to one-target-per-matrix-entry.** cargo-dist 0.31 emits `subject-path: "target/distrib/*${{ join(matrix.targets, ', ') }}*"`. This works because every matrix entry in Quorum's `dist-workspace.toml` has exactly one target. If a multi-target entry is ever added (e.g. universal macOS, or cross-compile-grouped Linux variants), the glob will silently fail to match the additional targets and those artifacts will ship without attestation — without erroring. Anyone restructuring the matrix should review the attest subject-path at the same time.
- **Reusable-workflow refs are pinned to the caller's commit, not to the default branch.** `uses: ./.github/workflows/publish-crates.yml` resolves from the same ref as the calling workflow (so for tag-triggered runs, from the tagged commit). A bug at the tagged commit cannot be fixed by landing a patch on `main` and `gh run rerun`-ing — the rerun reads the called workflow from the same commit. Recovery requires either tag rewrite (avoid when crates.io is involved — yank-not-delete) or out-of-band execution outside the workflow. This is a structural property of GitHub Actions reusable workflows; document explicitly so future ship-checklists don't bet on a `main`-side hotfix.
- **`cargo publish --wait-for-publish` is not a cargo flag.** It exists in some adjacent tooling and proposals but not as `cargo publish` CLI surface. Modern cargo (1.66+) already blocks on the post-publish index wait by default. Preflight that surfaces "should we wait?" questions should grep `cargo publish --help` against the version that will run in CI before recommending a flag — the CC-Recon-0.2.1-3 question presented `--wait-for-publish` as a real option and the CI failure flowed from accepting that answer uncritically.
- **GitHub repo-attestations are gated on private repos.** `actions/attest-build-provenance@v3` calls `POST /repos/{owner}/{repo}/attestations`, which 403s on private repos unless the org is on Enterprise Cloud / Team. Public repos get it for free. If sigstore attestation is required and the org is on Pro/Free, the repo has to be public; plan this constraint into milestone scoping rather than discovering it at first tag push.
- **Workflow-driven publish half is unproven live.** v0.2.1's attestation half validated end-to-end via the workflow (attempt 2's `build-local-artifacts` + `Attest` steps all green, sigstore bundles attached to the Release). The crates.io publish half went out-of-band — the workflow's `custom-publish-crates` failed on the `--wait-for-publish` parse error before any `cargo publish` call landed, so the fix at `5a658ef` won't be exercised until the next tag push. The next release is the first true end-to-end validation of the publish-crates workflow. Does not affect AC 132 FULL status, but future readers should not assume the full pipeline shipped clean from v0.2.1.
- **Repo visibility is now permanently public.** The mid-0.2.1 private → public flip was justified by the attestations-on-private-repo gate, but it's irreversible in practice for this project: the v0.2.0 and v0.2.1 binary distributions now assume public Releases (cargo-dist installers, attestation verifications, the README's `releases/latest` URLs). Any future private-only material cannot land in this repo. The pre-flip secret scan (`actions/secrets` / `dependabot/secrets` / `environments` all `total_count: 0`; tree audit) was the right precaution; subsequent work that wants to touch genuinely sensitive material needs a separate private repo or an Lippa-side equivalent.

### Stats

- **197 tests passing** at close — unchanged from Phase 1B close. 0.2.1 added zero new tests; this is release engineering only.
- `cargo build --release` / `cargo test --workspace` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo fmt --check --all` all green throughout.
- 5 commits land on `main` for the milestone (4 in Phase 0.2.1-A + 1 in 0.2.1-B + 1 close commit = 6 total; `c052593` was the tag base).

---

## Phase 1B — Dismissals, TUI, hooks, CI auth, v0.2.0 release ✦ 2026-05-11 → 2026-05-12

**Spec:** `specs/Quorum-Phase1B-Spec-v1_0.md`.
**Preflight:** `specs/Quorum-Phase1B-Preflight-notes.md` (8 documented divergences D1–D8; adjudication appended after Rolf review).
**Status:** **Closed at `v0.2.0`** — all three crates published to crates.io; GitHub Release shipped with 19 artifacts across 5 platforms + 4 installers. AC 132 PARTIAL (SHA256SUMS verified; sigstore deferred to 0.2.1 per `BACKLOG.md`).

**Public release:**
- crates.io: [`quorum-core`](https://crates.io/crates/quorum-core/0.2.0), [`quorum-lippa-client`](https://crates.io/crates/quorum-lippa-client/0.2.0), [`quorum-cli`](https://crates.io/crates/quorum-cli/0.2.0).
- GitHub Release: [`v0.2.0`](https://github.com/accillion/quorum/releases/tag/v0.2.0).
- CI run (release workflow): [`25732283892`](https://github.com/accillion/quorum/actions/runs/25732283892) — 9/9 jobs green.

### Commit graph

```
a92037f doc: track AC 132 PARTIAL + cargo-dist defaults learning
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
- **cargo-dist 0.31 defaults vs spec promises.** v1.0 §4.8.3 stated that tag-push triggers crates.io publish + sigstore attestation via GitHub OIDC. The Stage 5b.1 workflow inspection found cargo-dist 0.31's `dist init -y` defaults ship binaries + SHA256SUMS only; `publish-jobs` and `github-attestations` must be opted into via `dist-workspace.toml`. v0.2.0 ships with **AC 132 PARTIAL** (SHA256SUMS present, sigstore deferred); the follow-up is tracked in `BACKLOG.md` under 0.2.1 release engineering. Lesson: verify generated workflow against spec promises at preflight, not at ship time.
- **GitHub Actions registers workflows from the default branch, not from tags.** Stage 5b.2 pushed `v0.2.0` and got a silent no-op: zero workflow runs queued. Diagnosis: remote `main` was at `7189e7c` (pre-Phase-1A), so the workflow file existed at the tagged commit but was not in GitHub's "registered workflows" index, which sources from `main`. Fix: `git push origin main` to register the workflow file, then delete + recreate the tag at the same SHA. The retag at the same SHA was cosmetically ugly but mechanically clean — crates.io v0.2.0 was already final, so the second tag push only kicked the GitHub-Release workflow into life. Lesson: preflight of a tag-triggered workflow must include `git ls-tree origin/<default-branch> -- .github/workflows/` to confirm server-side registration; local file inspection alone is insufficient.
- **crates.io index propagation is fast on this account/connection.** Each `cargo publish` returned `Published quorum-X v0.2.0 at registry crates-io` synchronously; the 30s pause + `cargo search` verification step in the ship checklist was confirmed-by-design rather than tested-empirically (no propagation lag observed). The retry-once-after-60s contingency was not needed.
- **crates.io publish requires a verified email.** First attempt at `cargo publish -p quorum-core` returned HTTP 400 with `A verified email address is required to publish crates to crates.io`. This is account-hygiene at the registry, not a code or token issue. New publishers (or accounts that never had publishable crates) must complete the email-verify round trip at `https://crates.io/settings/profile` before the first publish. Document for future contributors: first-time publisher checklist includes the verified-email step.
- **AC 95's `(unknown)` fallback is the contract for crates.io installs.** `cargo install quorum-cli` from crates.io produces a binary whose `quorum --version` reports `quorum 0.2.0 (unknown)` rather than the tagged SHA. Expected: published source tarballs have no `.git/`, so `build.rs`'s `git rev-parse --short HEAD` fails and the `unknown` fallback fires. Local source builds embed the real SHA. The contract matches spec AC 95 verbatim; users on `cargo install` see the version + literal `unknown`, users on a git checkout see the version + real SHA.

### Stats

- **197 tests passing** across the workspace at close (Phase 1A close was 60; Phase 1B Stages 1–4 added 134; Stage 5a/6.1 added 3 render unit tests for AC 53).
- Workspace `cargo build --release`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check --all` all green throughout.
- 5 production crates / modules added or substantially modified: `quorum-core::memory`, `quorum-core::git::DiffSource`, `quorum-core::archive` v2, `quorum-cli::tui`, `quorum-cli::hooks`. The `quorum-lippa-client` crate gained no new public surface (the D7 fix was a transport-layer change).

### Live verification (post-Stage-5b)

- **AC 93 LIVE ✓** — `cargo install quorum-cli --root <tempdir>` from crates.io completes in ~27s release build; `quorum --version` reports `quorum 0.2.0 (unknown)` (per the documented tarball-fallback path, AC 95).
- **AC 94 LIVE ✓** — downloaded `quorum-cli-x86_64-pc-windows-msvc.zip` (4180825 bytes) and `quorum-cli-x86_64-unknown-linux-gnu.tar.xz` (3076148 bytes); SHA256 of each matches its per-file sidecar AND the aggregate `sha256.sum` entry. 7 entries in `sha256.sum` cover all 5 platform tarballs + MSI + source.
- **AC 132 PARTIAL** — SHA256SUMS shipped + verified. Sigstore attestation deferred to 0.2.1 per `BACKLOG.md`.

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
