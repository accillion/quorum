# Quorum Phase 1C — implementation plan

**Spec:** `specs/Quorum-Phase1C-Spec-v1_0.md` (613 lines, ACs 133–175
with gaps at 138, 147, 160, 167 — 39 ACs total after removals).
**Status:** planning only — no code written this session. Halt for
Rolf review before Stage 1 dispatch prompt is drafted.
**Estimated total CC effort:** 5 stages, 4–10 hours per stage,
~30–38 hours total.

---

## Stage decomposition rationale

The natural cut for Phase 1C follows the Phase 1B precedent of
"library before binary, data model first" but with one extra
split: read paths land before write paths inside the new
`quorum convention` CLI surface. Phase 1B got away with bundling
read+write because dismissals had a small CLI footprint (just
`--no-expire` and the TUI dismiss flow); Phase 1C adds a 6-subcommand
group plus a conventions.md parser/writer with cross-domain
atomicity concerns, which is enough surface area to justify the
extra cut. The spec guidance ("read paths before write paths
gives a debugging surface for the write side") points the same way.

The bridge mechanic (§6.2) lands with the bundle-assembly stage,
not as its own stage — it's a render-time decision in the bundle
path, not a state-machine change. Phase 1B's stage 1 similarly
bundled DiffSource and archive v2 with the dismissals data model
because all three landed in `quorum-core` as the library
foundation for later UI work.

Dependencies between stages are strict:
- Stage 2 (bundle) reads the `local_only` rows Stage 1 produces and
  needs the new `MemoryStore` surface from Stage 1.
- Stage 3 (CLI read) needs both Stage 1 (state column writes) and
  Stage 2 (so `list` output matches what the bundle sees) for
  meaningful integration tests.
- Stage 4 (CLI write) extends MemoryStore with T2/T3/T4 surface
  Stage 1 deferred; it also needs Stage 3's conventions.md parser
  (for orphan detection and demote-remove-block).
- Stage 5 (TUI) wraps the CLI surface from Stages 3+4 in keybindings
  and a new view; depends on the full state-machine surface working.

Phase 1B closed at 197 tests; Phase 1C should add ~50–80 (state
machine concurrency, bundle render, conventions.md parser/writer,
CLI surface, TUI keybindings, crash-recovery harness).

---

## Stages

### Stage 1: Data model + state machine spine (library)

**Scope:**
- SQLite v1→v2 migration in [crates/quorum-core/src/memory/schema.rs](crates/quorum-core/src/memory/schema.rs).
- New tables: `state_transitions`, `conventions`, `schema_meta`.
- Binary-side forward-compat check on every DB open.
- T1 (`candidate → local_only`) auto-transition inside `record_seen()`.
- `TransitionEvent` returned-value channel from `quorum-core::memory`
  to `quorum-cli` (no callback — preserves crate-boundary rule).
- `[memory]` section in [crates/quorum-core/src/config.rs](crates/quorum-core/src/config.rs): `candidate_threshold`, `local_convention_bundle_cap`, `candidate_expire_days` with range validation.
- `MemoryStore` trait additions for the candidate-row state read
  surface used downstream (`load_local_only_conventions`, internal
  state lookups by hash). Write surface for T2/T3/T4 is deferred
  to Stage 4 so this stage stays library-spine only.

**Modules:**
- [crates/quorum-core/src/memory/schema.rs](crates/quorum-core/src/memory/schema.rs) — v2 migration steps.
- [crates/quorum-core/src/memory/sqlite.rs](crates/quorum-core/src/memory/sqlite.rs) — `record_seen` returns `Vec<TransitionEvent>`; UPDATE-then-`rows_affected`-gate pattern for T1; forward-compat check at open.
- [crates/quorum-core/src/memory/mod.rs](crates/quorum-core/src/memory/mod.rs) — public `TransitionEvent` type; trait surface updates.
- [crates/quorum-core/src/config.rs](crates/quorum-core/src/config.rs) — `[memory]` parsing + range validation.
- [crates/quorum-cli/src/commands/review.rs](crates/quorum-cli/src/commands/review.rs) — consume `Vec<TransitionEvent>` from `record_seen`, emit stderr informational note, suppress under `--hook-mode=*`.

**ACs satisfied:** 134, 135, 136, 144, 145, 146, 151, 164, 165, 174.
(Partial: 171 regression coverage continues from this stage onward;
172 clippy/fmt is cross-cutting.)

**Tests added:**
- [crates/quorum-cli/tests/dismissals_store.rs](crates/quorum-cli/tests/dismissals_store.rs) — extend with migration idempotency, schema_meta read-back, forward-compat reject.
- [crates/quorum-cli/tests/dismissals_filter.rs](crates/quorum-cli/tests/dismissals_filter.rs) — extend with T1-fires-once-under-concurrency (`std::thread::spawn`, NOT tokio per §C6 / AC 144), audit-row gated by `rows_affected > 0`.
- New: state-transition log read tests; `TransitionEvent` shape.

**Dependencies on prior stages:** none — this is the foundation.

**Exit criteria:**
- v1→v2 migration runs cleanly on a Phase 1B fixture DB; re-running is a no-op.
- AC 144 concurrent-`record_seen` test green under `cargo test --workspace`.
- AC 174 forward-compat reject test green.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check --all` clean.
- No changes under [crates/quorum-cli/src/tui](crates/quorum-cli/src/tui) (deferred to Stage 5).
- No new `quorum convention` subcommand surface yet (deferred to Stages 3+4).
- No changes under [crates/quorum-lippa-client](crates/quorum-lippa-client/src) (Phase 1C has zero Lippa-side delta).

**Estimated CC session length:** 6–8 hours.

**Risks / open implementation questions:**
- The exact concurrency test (`std::thread::spawn` + barrier to widen the timestamp spread past 1 ms) needs to be deterministic on Windows; if `thread::sleep(Duration::from_millis(2))` between barrier-release and SQLite write is unreliable, fall back to writing the audit rows with explicit `ts` parameters to force the millisecond difference.

---

### Stage 2: Bundle assembly + promote-but-uncommitted bridge (library)

**Scope:**
- `MemoryStore::load_local_only_conventions()` returns the rows
  needed by the bundle layer (state=`local_only` + bridge-eligible
  `promoted_convention` rows).
- Bundle assembly renders the `## Local conventions (auto-derived)`
  subsection in the existing 20 KB `BUDGET_MEMORY`, sorted by
  `recurrence_count DESC, last_seen_at DESC`.
- Per-entry rendering format (§6.1 step 3): title-truncated to 80
  chars, body truncated at `local_convention_bundle_cap`, trailing
  HTML-comment metadata line.
- Per-entry truncation marker (§5.3 form).
- Total-section truncation marker (existing `[memory truncated …]`
  shape from [SERVICES.md](SERVICES.md#2-bundle-assembly) §2).
- §6.2 bridge: at bundle assembly time, every `promoted_convention`
  row runs through the Phase 1A committed-and-clean check on
  `.quorum/conventions.md` (already implemented for the conventions
  section). If dirty/uncommitted/missing, the row is rendered into
  the memory section as a `local_only` entry. SQLite state is
  unchanged.
- Delimiter wrapping (`<<<QUORUM_REPO_CONTENT_BEGIN>>>` … `END>>>`)
  continues to enclose the new subsection — Phase 1A behavior
  preserved, no new wrapping logic.

**Modules:**
- [crates/quorum-core/src/bundle.rs](crates/quorum-core/src/bundle.rs) — new memory subsection assembler, per-entry render fn, bridge fork at render time.
- [crates/quorum-core/src/memory/sqlite.rs](crates/quorum-core/src/memory/sqlite.rs) — `load_local_only_conventions()` query (joins `dismissals` with optional `conventions` row).
- [crates/quorum-core/src/conventions.rs](crates/quorum-core/src/conventions.rs) — reuse the Phase 1A committed-and-clean check; expose a per-call helper for the bundle path (not a global cached state).

**ACs satisfied:** 152, 153, 154, 155, 156, 170, and partial 173
(commit-state toggle correctness — full AC 173 round-trips through
Stage 4 promote/demote, but the render-side toggle is testable
here with fixture rows hand-inserted into SQLite).

**Tests added:**
- [crates/quorum-cli/tests/bundle_assembly.rs](crates/quorum-cli/tests/bundle_assembly.rs) — `## Local conventions (auto-derived)` rendering, sort order, total-section truncation, per-entry truncation, delimiter wrapping (AC 170).
- New: bridge-render fixture — fixture DB with one `promoted_convention` row, conventions.md present-but-dirty → renders in memory section; same row, conventions.md committed-and-clean → renders nowhere in memory section (lives only in conventions section, fed by file).

**Dependencies on prior stages:** Stage 1 (state column writes, `load_local_only_conventions`).

**Exit criteria:**
- Bundle assembly passes existing AC 152–156 + 170 + partial 173.
- Total bundle size still ≤ 200 KB (`BUDGET_TOTAL` per [SERVICES.md](SERVICES.md#2-bundle-assembly) §2 — AC 156).
- Clippy/fmt clean.
- No CLI surface changes yet.

**Estimated CC session length:** 4–6 hours.

**Risks / open implementation questions:**
- The bridge needs the Phase 1A committed-and-clean check called
  *per bundle assembly invocation* with no caching. Confirm the
  existing check is cheap enough (single libgit2 tree-walk for the
  one file) that re-invoking inside the memory-section loop isn't
  a regression. Spec §6.2 wording supports per-bundle-not-per-row.

---

### Stage 3: CLI read paths + conventions.md parser + orphan detection

**Scope:**
- New top-level CLI subcommand group: `quorum convention`.
- Read subcommands: `list [--state … | --orphans | --json]`,
  `show <hash>`, `history <hash>`.
- Short-hash resolution (≥8 hex prefix; ambiguous → exit 2).
- Conventions.md parser: read managed-section fence + per-block id
  enumeration. Needed by `list --orphans` (compare SQLite vs file)
  and reused by Stage 4's writer (round-trip preserves above-fence
  + below-fence user content byte-for-byte).
- Orphan detection: managed block in file with no SQLite row, or
  SQLite `conventions` row with no managed block in file. AC 168
  warning surface (stderr).

**Modules:**
- [crates/quorum-cli/src/commands/convention.rs](crates/quorum-cli/src/commands/convention.rs) — new file, list/show/history dispatch.
- [crates/quorum-cli/src/commands/mod.rs](crates/quorum-cli/src/commands/mod.rs) — register `convention` subcommand group.
- [crates/quorum-cli/src/main.rs](crates/quorum-cli/src/main.rs) — clap surface for the group.
- [crates/quorum-core/src/conventions.rs](crates/quorum-core/src/conventions.rs) — parser: enumerate `<!-- quorum:convention id=… v=… -->` blocks; locate managed-section fence; preserve outside-fence content as a byte slice for round-tripping in Stage 4.
- [crates/quorum-core/src/memory/sqlite.rs](crates/quorum-core/src/memory/sqlite.rs) — read-only queries used by CLI surface (`list_by_state`, `find_by_short_hash`, `load_transitions`).

**ACs satisfied:** 133 (Phase 1B regression sweep checkpoint), 162,
163, 166 (Lippa seam — endpoint-set assertion test added here as it
fits with the new CLI surface), 168 (warning emission path), 169
(orphan detection logic — `list --orphans` flag).

**Tests added:**
- New `crates/quorum-cli/tests/convention_read.rs` — `list` filtering, `show` output shape, `history` ordering, short-hash ambiguity exit 2, `--json` stable ordering, `--orphans` reports both directions.
- New `crates/quorum-core/tests/` is not the project pattern; conventions.md parser tests live as `#[cfg(test)] mod tests` inside [crates/quorum-core/src/conventions.rs](crates/quorum-core/src/conventions.rs) (matches Phase 1A pattern).
- New integration test `tests/conventions_lippa_seam.rs` (per §7.4): the `quorum-lippa-client` call-site enumeration excludes `/api/v1/memory/propose`. AC 166.

**Dependencies on prior stages:** Stage 1 (state column reads, state_transitions log reads); Stage 2 (bundle behavior must be stable so `list` output cross-checks against bundle render).

**Exit criteria:**
- All read CLI surface (`list`, `show`, `history`) produces stable, pipe-friendly output (`--json` for `list`).
- Parser preserves above-fence + below-fence content byte-for-byte (round-trip property test).
- Orphan detection green on both directions (AC 169).
- AC 166 Lippa seam test green.
- No write paths yet — `promote`/`demote`/`prune` subcommands either stubbed-as-exit-2-not-implemented or absent from clap (cleaner if absent; reintroduced in Stage 4).
- Clippy/fmt clean.

**Estimated CC session length:** 6–8 hours.

**Risks / open implementation questions:**
- The parser must handle the first-line marker `<!-- quorum-managed-conventions-md v=1 -->` (§4.4 — used for outside-Quorum-modification detection in §4.5). Stage 3 only consumes this marker; Stage 4 writes it on fresh-file creation. Confirm Stage 3 tolerates files that lack the marker (Phase 1A users have committed conventions.md files without it).

---

### Stage 4: CLI write paths + conventions.md writer + T2/T3/T4/T5

**Scope:**
- Write subcommands: `promote <hash> [--text | --from-editor]`,
  `demote <hash>`, `prune [--state … | --older-than | --dry-run]`.
- Conventions.md writer: temp-file + atomic rename pattern
  (`fs::rename`, with `MoveFileExW` semantics on Windows).
- T2/T3 cross-domain atomicity: BEGIN → file rename → SQLite writes
  → COMMIT. ROLLBACK on rename failure. Partial-failure window
  (file rename succeeded, SQLite COMMIT did not) is recoverable
  via Stage 3's `list --orphans`.
- T4 prune: DELETE matched rows; FK CASCADE drops audit rows. No
  audit row written for prune itself.
- T5 undismiss extension: `MemoryStore::delete(hash)` on a
  `promoted_convention` row removes the managed block from
  conventions.md before SQLite COMMIT (same file-first pattern).
  Audit-trail-silent (§3.2 T5).
- Re-promote precondition: only `local_only` rows promote;
  `promoted_convention` rows must demote first (§4.4, AC 137).
- Line-ending preservation across promote/demote (AC 150).
- Section-fence auto-creation on first promote when absent (§4.4).
- Crash-after-rename harness test (AC 175).

**Modules:**
- [crates/quorum-cli/src/commands/convention.rs](crates/quorum-cli/src/commands/convention.rs) — extend with promote/demote/prune; consume `$EDITOR` for `--from-editor`.
- [crates/quorum-core/src/conventions.rs](crates/quorum-core/src/conventions.rs) — writer: build new file content (preserve outside-fence + reconstruct fence + new/updated/removed block), temp-file + atomic rename.
- [crates/quorum-core/src/memory/sqlite.rs](crates/quorum-core/src/memory/sqlite.rs) — write methods for T2 (`promote`), T3 (`demote`), T4 (`prune`); extend `delete` for T5 cascade.

**ACs satisfied:** 137, 139, 140, 141, 142, 143, 148, 149, 150, 175,
and the remaining half of AC 173 (full round-trip: promote with
uncommitted → render in memory section; commit → next assembly
renders in conventions section).

**Tests added:**
- New `crates/quorum-cli/tests/convention_write.rs` — promote/demote/prune happy paths, exit-2 rejection cases (promote-from-candidate, promote-from-promoted, prune-promoted), `--text`/`--from-editor` mutual exclusion, no-body default produces title-only block.
- New `crates/quorum-cli/tests/conventions_md_format.rs` — round-trip preserves above-fence + below-fence (AC 149), LF-vs-CRLF preservation (AC 150), demote-then-repromote produces single block (AC 148).
- New `crates/quorum-cli/tests/crash_recovery.rs` — AC 175 harness. Uses a `MemoryStore` test seam that panics between file rename and SQLite COMMIT; verifies the next `list --orphans` invocation reports the orphan managed block.
- Extend `crates/quorum-cli/tests/dismissals_store.rs` — T5 cascade through `delete` for a `promoted_convention` row removes the managed block.

**Dependencies on prior stages:** Stage 1 (T1, schema, state column writes); Stage 2 (bridge render — closes AC 173 round-trip); Stage 3 (parser reused by writer; orphan detection used by AC 175 harness).

**Exit criteria:**
- All write subcommands green; exit-code taxonomy honored (Phase 1A 0/1/2/3 stable).
- AC 175 crash-recovery test green; `list --orphans` flags the orphan from a simulated crash.
- AC 173 full round-trip green (promote → bundle assembly renders in memory section; commit conventions.md → bundle assembly renders in conventions section; edit-without-commit → revert to memory section).
- AC 171 regression sweep: all Phase 1B tests still green at HEAD.
- AC 172 clippy/fmt clean.

**Estimated CC session length:** 8–10 hours (most-complex stage — atomicity + crash harness + multi-AC coverage).

**Risks / open implementation questions:**
- Windows `fs::rename` semantics: on Windows the rename across the
  same volume is atomic via `MoveFileExW`; verify Phase 1B's
  archive write path (which also uses temp-rename) for the
  established pattern. The conventions.md write must use the same
  helper to avoid divergent platform behavior.
- `$EDITOR` invocation under `--from-editor` needs to be tested
  hermetically — use a fake editor binary that writes a known
  string and exits 0, parameterized via a test-only env var.
- AC 175 harness needs a way to "crash" the process between
  rename and COMMIT. The cleanest seam is a `#[cfg(test)]` hook
  inside `LocalSqliteMemoryStore` that panics after `rename`
  returns and before `commit()` is called; the test catches the
  panic with `std::panic::catch_unwind` and asserts file-state.

---

### Stage 5: TUI dismissal-history view + p/D keybindings

**Scope:**
- New TUI view: dismissal-history (entered via `H` from main list).
- Columns: short-hash, state (`c`/`L`/`P`), recurrence, reason, title.
- Body pane: title + body_snapshot + state + last 5 transition log entries.
- New keybindings:
  - `H` — toggle history view.
  - `p` — promote-from-history (only on `local_only` rows; modal for body text with title-derived default; Enter-with-empty produces title-only).
  - `D` (capital) — demote-from-history (only on `promoted_convention` rows; `Y/N` confirmation modal). Capital-D avoids collision with main-list `d`-for-dismiss (§B6).
- Status bar: `H back | p promote | D demote | x delete | / filter | ? help`.
- TUI invokes the same `quorum-core::memory` write surface Stage 4 built; no new domain logic.
- Terminal-restore on panic-during-T2 (AC 161 round-trip) — leverages Phase 1B's existing RAII guard + panic-hook chain unchanged.

**Modules:**
- [crates/quorum-cli/src/tui/mod.rs](crates/quorum-cli/src/tui/mod.rs) — new view variant, mode-switch via `H`.
- [crates/quorum-cli/src/tui/state.rs](crates/quorum-cli/src/tui/state.rs) — `AppState` extension for history view (snapshot of rows; no live re-fetch per §2).
- [crates/quorum-cli/src/tui/panels.rs](crates/quorum-cli/src/tui/panels.rs) — history view renderer + body pane extension.
- [crates/quorum-cli/src/tui/dismiss_prompt.rs](crates/quorum-cli/src/tui/dismiss_prompt.rs) — new promote-modal and demote-confirm-modal alongside the existing dismiss modal.

**ACs satisfied:** 157, 158, 159, 161.

**Tests added:**
- Extend [crates/quorum-cli/tests/tui_smoke.rs](crates/quorum-cli/tests/tui_smoke.rs) — `H` enters/exits history view; `p` on `local_only` opens modal; `p` on candidate/`promoted_convention` is rejected at keystroke level; `D` on `promoted_convention` prompts and demotes; `D` on other rows is rejected.
- Panic-during-T2 restoration test reuses Phase 1B's panic-hook test helper.

**Dependencies on prior stages:** Stage 4 (write surface T2/T3 must work). Stage 3 (read queries used to populate the history view). Stage 2 only indirectly (TUI doesn't read the bundle layer).

**Exit criteria:**
- All TUI ACs green (157, 158, 159, 161).
- TUI exit + panic paths leave terminal restored (AC 161, Phase 1B behavior preserved).
- All Phase 1B TUI tests still green (AC 171 regression check).
- Clippy/fmt clean.
- README TUI section + `quorum review --help` text updated for the new keybindings (§5.5).

**Estimated CC session length:** 4–6 hours.

**Risks / open implementation questions:**
- The history view is a snapshot (§2 non-goal: no TUI re-fetch). Confirm `AppState` lifetime is per-TUI-session; quit-and-re-invoke is the refresh path. This matches Phase 1B's modal pattern.

---

## Blockers surfaced during planning

**None — spec is implementation-ready.**

Minor non-blocking coverage notes for the implementation session
(not adjudication-required; surface as non-AC integration tests):

- **Auto-promote stderr informational note has no numbered AC.**
  §3.2 T1 and §5.3 specify the message text and the
  `--hook-mode=*` suppression rule, and AC 135 covers the
  underlying transition. The CLI-side emission (consuming the
  `TransitionEvent` returned value) is testable as a non-AC
  integration check in Stage 1's test file. Lean: cover with a
  named-but-unnumbered test, document the gap in the stage
  report.
- **`candidate_expire_days = 0` behavior.** AC 164 covers range
  validation (0..3650) and §3.4 specifies "0 disables auto-expire."
  The combined behavior (`candidate_expire_days = 0` + `prune`
  with default `--older-than` = 0 rows pruned) has no explicit
  AC. Lean: cover with a non-AC unit test in Stage 4.
- **Q7 (demote when conventions.md is missing).** Spec gives a
  lean ("no-op for the file write, with stderr warning; the
  SQLite state changes proceed") but no AC. Stage 4 will need a
  test deciding this concretely. Lean: implement per the §10 Q7
  lean; document choice in the stage report. If a future user
  hits the missing-file edge, the lean can be revisited without
  schema change.
- **Q12 / Q15 (atomicity recovery + promote-on-dirty-conventions.md
  behavior).** Q12's lean ("rely on `list --orphans`") is
  consistent with AC 175. Q15's lean ("warn-but-proceed on
  outside-fence; warn-and-confirm on inside-fence") is a UX
  choice with no AC. Stage 4 will need to land *some* behavior;
  the spec leans are sufficient guidance, no Rolf adjudication
  required pre-Stage-4. If the implementation session disagrees
  with the lean it should halt and surface — same pattern as
  Phase 1B preflight divergences D1–D8.

If the implementation session uncovers a genuine ambiguity not
listed above, halt per the spec's own halt conditions in §10 and
escalate to Rolf rather than silently picking a behavior.

---

## ACs not directly assigned to a stage

Two ACs are cross-cutting and satisfied continuously rather than in
one stage:

- **AC 171 — Phase 1B regression.** Every stage's exit criteria
  include "all Phase 1B tests still green." The full sweep is run
  at each stage close; no stage exits with red Phase 1B tests.
- **AC 172 — clippy/fmt.** Same — every stage's exit criteria
  require `cargo clippy --workspace --all-targets -- -D warnings`
  and `cargo fmt --check --all` clean.

The 39-AC count breaks down as: Stage 1 (10 ACs: 134, 135, 136,
144, 145, 146, 151, 164, 165, 174), Stage 2 (7 ACs: 152, 153, 154,
155, 156, 170, partial 173), Stage 3 (6 ACs: 133, 162, 163, 166,
168, 169), Stage 4 (10 ACs: 137, 139, 140, 141, 142, 143, 148,
149, 150, 175 + closes 173), Stage 5 (4 ACs: 157, 158, 159, 161),
cross-cutting (2 ACs: 171, 172). Total: 37 + 2 cross-cutting = 39.
Matches the spec's removed-gap-adjusted count (43 numbers − 4
gaps).

---

## Recommended Stage 1 dispatch

**Fire Stage 1 first** — the data model is the load-bearing
foundation for every later stage. No stage can land library or
UI surface that exercises the state machine without the v1→v2
migration in place, and Phase 1B's stage-1 precedent (data model +
DiffSource + archive v2) is the right pattern.

**Halt criteria for Stage 1's execution prompt** (to be drafted by
Rolf after reviewing this plan):

- Stage 1 saves all new schema + record_seen + config + CLI-stderr
  emission code → halt and report. NO further stages launched
  from the Stage 1 session.
- Migration test on a fresh Phase 1B fixture DB green → confirm.
- AC 144 concurrent-`record_seen` green → confirm.
- AC 174 forward-compat reject green → confirm.
- All Phase 1B integration suites still green → confirm.
- Clippy + fmt clean → confirm.
- Catch yourself about to write under [crates/quorum-cli/src/tui](crates/quorum-cli/src/tui), [crates/quorum-core/src/bundle.rs](crates/quorum-core/src/bundle.rs), or any new `quorum convention` CLI surface → halt; that's a later stage's scope.

**Estimated Stage 1 effort:** 6–8 hours.

After Stage 1 closes, Stages 2–5 dispatch sequentially. Each
stage's exit criteria should include a regression-sweep of all
prior-stage tests and a confirmation that the spec sections
exercised by that stage have no behavioral divergences from
implementation.
