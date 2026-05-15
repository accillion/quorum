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
