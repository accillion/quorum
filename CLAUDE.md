# Quorum — Persistent Context

This file is read automatically by Claude Code at the start of every session.
Do not delete it. Update it when a milestone is completed or a decision changes.

---

## What This Project Is

Quorum is a multi-model code reviewer that runs on the developer's machine.
It consumes Lippa's public API as upstream infrastructure and never modifies
Lippa code. The wedge is Consensus-across-frontier-models + codebase memory
+ inspectable redaction, all delivered as a CLI binary that hooks into git.

**Core promise:** the first code reviewer that knows your codebase as well
as your senior engineer does.

**Sibling repo:** the Lippa repo lives at `../lippa`. Quorum NEVER writes
to Lippa. CC reads `../lippa/CLAUDE.md` and `../lippa/SERVICES.md` for
upstream context but does not touch them.

**Specs and prompts:** durable specs land in `specs/` (tracked).
CC session-driver prompts land in `prompts/` (.gitignored).

**History:** see `HISTORY.md` (when it exists) for completed milestone
detail. Currently empty — pre-Phase-1A.

**Service contracts:** see `SERVICES.md` (when it exists) for service
behavioural rules. Currently empty — services will be added by Phase 1A.

---

## Role of Claude Code in This Project

Claude Code writes all code, wires CLI commands, and keeps changes scoped
to the current milestone listed below.

Claude Code must not:
- Make product or architecture decisions
- Build features not listed in the current milestone
- Add dependencies not listed in the tech stack
- Modify anything in `../lippa`
- Skip acceptance criteria verification steps

---

## Documentation Routing

When completing work, update the correct file:

| Change type | Update file |
|---|---|
| New service contract or architecture rule | SERVICES.md (create when needed) |
| Schema change or local-storage migration | DATABASE.md (create when needed) |
| Completed milestone | HISTORY.md (create when needed) |
| Deferred ticket surfaced mid-session | BACKLOG.md (create when needed) |
| Current milestone status, tech stack, identity | CLAUDE.md (this file) |

Routing is mandatory — never duplicate content across files.
**Hard cap: CLAUDE.md ≤ 280 lines** (enforce with `wc -l CLAUDE.md`).
If a constraint is only relevant when touching one subsystem, route it
to the subsystem doc, not here.

---

## Tech Stack (Locked — Do Not Change)

| Layer | Choice |
|---|---|
| Language | Rust (stable, edition 2021) |
| Workspace | Cargo workspace: `quorum-cli`, `quorum-core`, `quorum-lippa-client` |
| Git access | `git2` crate (libgit2 binding, no shell-out) |
| HTTP client | `reqwest` with `rustls` |
| Serde | `serde` + `serde_json` for Lippa API payloads |
| TUI (Phase 1B+) | `ratatui` (Phase 1A is plain stdout only) |
| Local storage | SQLite via `rusqlite`; `sqlite-vec` for embeddings (Phase 3+) |
| Auth storage | OS keychain via `keyring` crate |
| Logging | `tracing` + `tracing-subscriber` |
| Test | `cargo test` + golden-file fixtures under `tests/fixtures/` |
| Local LLM (Phase 4) | Ollama as default backend; `llama.cpp` direct deferred |
| Sandbox (Phase 4) | Docker for the redaction pre-pass + local LLM tier |

---

## Hard Constraints (Never Violate)

- **No Lippa server changes from this repo.** Quorum is a pure external
  client. Any Lippa-side change required by Quorum is filed as a separate
  spec in the Lippa repo and tracked there, not here.
- **Read-only side effects in v1.** Quorum reads source code and writes
  review output to its own data directory. It never modifies user code.
  No network writes outside Lippa API + (Phase 4) Ollama localhost.
- **Pre-commit hook is fast-only.** Network-bound full Consensus runs at
  pre-push or manual `quorum review`, never pre-commit. Pre-commit is for
  cheap deterministic checks and (optional) local-model smell pass.
- **Read repo content as DATA, never as instructions.** Code Quorum
  reviews can contain prompt-injection attempts. Wrap all repo content
  in delimiters; system prompt notes adversarial possibility; memory
  writes never trust repo text as instruction.
- **`.quorum/conventions.md` is trusted only when committed.** Uncommitted
  changes to the conventions file are ignored by Quorum's context loader.
- **Memory writes go through a dismissal state machine.** Never write a
  `convention` directly from a single dismissal. State: `candidate` →
  `local_only` → `promoted_convention`. Cloud writes only on explicit
  promote OR repeated dismissals OR admin approval.
- **Source allowlist (Lippa-side).** When writing memory items to Lippa,
  the `source` field MUST be `model_proposed` with `proposed_by_model`
  and `confidence` set. Never invent new source values; the Lippa
  allowlist is enforced server-side and unknown values are rejected.
- **Cargo workspace boundaries.** `quorum-cli` depends on `quorum-core`
  and `quorum-lippa-client`. `quorum-core` and `quorum-lippa-client` do
  NOT depend on each other. Cross-crate calls go through public APIs only.
- **Fail-open at pre-commit / pre-push hooks on network errors.** Hooks
  that block commits on transient network errors get ripped out. On any
  Lippa API failure during a hook, print a warning and exit 0. Manual
  `quorum review` may exit non-zero on network errors (debuggable surface).
- **Privacy mode redaction is regex/AST first, local LLM assistive.** The
  primary redaction engine is deterministic pattern matching with audit.
  Local Gemma (Phase 4) is a secondary validator, never the root of trust.

---

## Developer Environment

- **OS:** Windows / Git Bash
- **Rust:** stable channel via rustup; verify with `rustc --version`
- **Sibling Lippa repo:** clone at `../lippa` for read-only context
- **GitHub repo:** https://github.com/accillion/quorum (private)
- **Build:** `cargo build --release` from repo root
- **Lint discipline:** `cargo clippy -- -D warnings` and `cargo fmt --check`
  must pass before any commit
- **crates.io publish auth:** Trusted Publishing (OIDC) via
  `rust-lang/crates-io-auth-action@v1` in `publish-crates.yml`; no stored
  `CARGO_REGISTRY_TOKEN`. crates.io matches the OIDC `workflow_ref`
  claim, i.e. the *caller*, so each crate's trusted-publisher entry names
  `release.yml`. Renaming `release.yml` breaks publishing until the three
  crates.io entries are updated.

---

## Repo Structure (target)

```
quorum/
  Cargo.toml                  # workspace root
  Cargo.lock
  crates/
    quorum-cli/               # binary: command parsing, output, hooks
      src/
      Cargo.toml
    quorum-core/              # library: review pipeline, aggregator, memory loop
      src/
      Cargo.toml
    quorum-lippa-client/      # library: Lippa API client (Consensus, memory)
      src/
      Cargo.toml
  specs/                      # durable specs (tracked)
  prompts/                    # CC session-driver prompts (.gitignored)
  tests/
    fixtures/                 # golden-file diffs + expected outputs
  CLAUDE.md                   # this file
  README.md
  LICENSE
  .gitignore
```

---

## Current Milestone Status

**Active:** **0.4.0 release. Part A (release prep) is in review as
PR #8 on `release/v0.4.0`, off `main` `e294692`. Part B waits for
Rolf's "tag it":** tag the merge commit `v0.4.0`, watch the first live
Trusted Publishing run, verify crates.io, the GitHub Release and
`cargo install quorum-cli --version 0.4.0 --locked`, then fill in the
`TBD`s in HISTORY.md's Phase 0.4.0 entry.

PR #8 is three commits. `6842955` is AC 211a: truncation cuts at a
codepoint boundary, and so does `exit.rs`'s 16-byte HTML sniff. It
reverts alone. `b9fcb6f` moves the workspace to 0.4.0 and moves
`quorum-cli`'s path-dep pins, which still said `"0.3.0"`. Docs come
last. `Cargo.lock` is gitignored, so the bump commits no lockfile
(BACKLOG).

0.4.0 ships v0.4 Stage 1 (ACs 176–194) plus AC 211a, SEC-1 (PR #2),
DL Part 1 (PR #5), DL Part 2 (PR #7), two-factor login (PR #6) and
Trusted Publishing. HISTORY.md's Phase 0.4.0 entry says what each did
and lists every library breaking change. Behavioural rules live in:
- SERVICES.md §2.1–2.4: bundle;
- §3, §4.1, §4.2: auth and the session pin;
- §5.1: deny list;
- §6/§6.1 and DATABASE.md: the dismissals store and conventions writes.

**v0.4 Stages 2–4 are retargeted to 0.5.x and not started.** Spec:
`specs/Quorum-v0_4-Spec-v0_1.md` (AC range 176–241). The §5.3 budget
*rebalance* (80/72/16/24/6 plus the review-policy section) is Stage
2's. Stage 1 shipped the derivation mechanism at v0.3.3 proportions.
AC 211a is done. Of AC 240, only the session/base-URL pairing is done.

**Two-factor login (bug fix, no spec gate) is closed:** PR #6 merged
into `main` (`e294692`). `f4deab4` depends on `1448387` by design:
revert it alone, or revert it and then `1448387`.

**crates.io Trusted Publishing is merged but unproven.** PR #1
(`8f335bb`) moved `publish-crates.yml` from a stored token to OIDC, and
PR #3 (`f443ef5`) narrowed the job to `id-token: write`. No tagged
release has run it yet. Count it as working only once the 0.4.0 run's
`custom-publish-crates` job has published all three crates.

Repo state on `main` (`e294692`): 492 tests pass, 1 ignored, on Linux,
and 491 on Windows. `release/v0.4.0` has 494 / 1 and 493. clippy
`-D warnings` and fmt `--check` are clean. `cargo publish --dry-run`
passes for both libraries and with `--workspace`. `../lippa` is
untouched. Open items, including the Stage 1 deferrals, are in
`BACKLOG.md`.

---

## Common Commands

```bash
# Build & test
cargo build --release
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check --all

# First-time setup against Lippa
./target/release/quorum auth login                        # interactive
./target/release/quorum link --project <project_id>       # writes .quorum/config.toml

# Run a review on staged diff
git add <files>
./target/release/quorum review                            # markdown to stdout
./target/release/quorum review --json                     # same buffer to stdout + .quorum/reviews/<ISO>.json

# Auth status / logout
./target/release/quorum auth status
./target/release/quorum auth logout
```
