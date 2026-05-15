# Quorum — Backlog

Tickets surfaced mid-session that the active milestone doesn't cover.
Group by next-target version. Most-recent first.

---

## Closed in v0.3.3 (originally planned for v0.3.1)

- ~~**Stale `--help` text in `convention` subcommand group.**~~ Closed by WI-5 in the v0.3.1 fixes payload (landed at `aed4aa1`); shipped to users in v0.3.3.
- ~~**Cross-process `OsKeyring` round-trip test.**~~ Closed by WI-1 in the v0.3.1 fixes payload; negative-control verified failing on unfixed `aed4aa1` (would have caught BUG 1 pre-v0.3.0).

---

## Next-target: v0.4 (or later)

- **CI: explicit system-dependency declaration.** Both `release.yml` and `publish-crates.yml` currently rely on a manual `Install libdbus on Linux` apt step because cargo-dist 0.31 ignores `[dist.dependencies.apt]` in `dist-workspace.toml`. The duplication caused the v0.3.1 → v0.3.2 → v0.3.3 cascade (v0.3.1 added the manual step in only `release.yml`; v0.3.2 needed `allow-dirty = ["ci"]` to silence cargo-dist's self-check on that file; v0.3.3 added the same manual step to `publish-crates.yml` after `cargo publish --verify` re-compiled `libdbus-sys`). A future cargo-dist upgrade (or a `dependencies` config syntax change) should reduce this to a single declarative config entry. Until then, any new workflow file that runs `cargo build` / `cargo publish --verify` on Linux needs its own libdbus apt step. Tracking item — revisit at next cargo-dist bump.

- **Shared file+SQL orchestrator helper** (Phase 1C Stage 5 — new). Refactor the file-write-then-`commit_promote/commit_demote` sequence into a `quorum-core::conventions` library helper. Currently duplicated ~50 LOC between `commands/convention.rs` (CLI orchestrator) and `tui/mod.rs::tui_promote/tui_demote` (TUI orchestrator). Library-code change so it was correctly deferred from Stage 5's TUI-only scope.

- **TUI pre-flight diagnostics surface** (Phase 1C Stage 5 — new). Library helper `pre_flight_check() -> Vec<String>` callable from both the CLI orchestrator (currently emits to stderr via `format_diagnostic`) and the TUI orchestrator (currently silent). Closes the AC 168 gap on the TUI side. Render as transient status-bar messages on the TUI; suppress on `--hook-mode=*` consistent with Q14 lean.

- **Q15 inside-vs-outside-fence detection** (Phase 1C Stage 4 — deferred). Stage 4 simplified to warn-on-any-pre-flight-discrepancy. If user feedback ever surfaces the distinction mattering, add body-text comparison against `conventions.convention_text` + `--yes` gating for inside-fence modifications, per the v1.1 spec §10 Q15 lean.

- **Auto-promote stderr end-to-end integration test** (Phase 1C Stage 1 — deferred). The `quorum: dismissal <short-hash> auto-promoted ...` message + `--hook-mode=*` suppression has constituent-part test coverage (TransitionEvent shape via AC-135 test; formatter exercised at call site) but no dedicated end-to-end integration test running `quorum review` against a synthetic Lippa fixture. Stage 3's stderr work covered parser diagnostics on `list --orphans`, not this surface; the gap persists.

- **`review.rs` MemoryStore double-open consolidation** (Phase 1C Stage 2 — deferred). Bundle assembly and the dismissals-filter site each open the SQLite store independently per `quorum review` invocation. Consolidate into one handle threaded through the pipeline. Stage 4 design-judgment, not Phase 1C blocker.

- **Transition-log in-memory cache for TUI body pane** (Phase 1C Stage 5 — perf optimization). `Command::LoadTransitions` fires per `j`/`k`/`g`/`G` cursor move in the dismissal-history view. Each query is an indexed `state_transitions` read by hash — fine for typical histories, but a small per-hash cache keyed in the TUI app state would avoid repeated reads when scrolling. Add only if perf data demands it.

---

## Phase 1D / v0.4 candidates from production dogfood (15 May 2026)

### Quorum-side (v0.4)

- **Bundle budget rethink for real repos.** 200KB diff cap truncated ~100 files' contents on the first dogfood run; output quality degraded vs an un-truncated re-run on a smaller staged slice. Options: raise the cap, smarter prioritization of changed-file context, or prominent docs on "stage a slice."
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
