# Quorum — Phase 1C: Conventions-promotion state machine

**Status:** v0.1 draft. Open for Rolf review → peer-review sessions → v0.2.
**Spec home:** `specs/Quorum-Phase1C-Spec-v0_1.md`
**Driver:** Quorum (this repo). Builds on Phase 1B's dismissals foundation; external client of Lippa's `/api/v1/*` surface (forward-compat only in this milestone — no cloud writes).
**References:** `specs/Quorum-Phase1C-Scoping-notes.md` (this draft's adjudicated input); `specs/Quorum-Phase1B-Spec-v1_0.md` (canonical predecessor — Rolf's local copy); `HISTORY.md` Phase 1B and 0.2.1 entries; `SERVICES.md` §2 (bundle budgets), §6 (dismissals store), §8 (DiffSource); `CLAUDE.md` hard constraints; `../lippa/apps/api/app/routes/api_v1/memory.py` + `app/schemas/memory_propose.py` (read-only — forward-compat shape for §7).

**Positions taken from scoping notes §5** (defaults: my lean unless adjudicated otherwise in scoping §3; this draft uses the leans):

- **§5.1 — Recurrence threshold:** **Option A.** Exact-hash recurrence, `N = 3`, all-time, configurable via `.quorum/config.toml` `[memory] candidate_threshold = 3`. Embedding-based fuzzy matching (Option B) deferred to Phase 3; hybrid (Option C) rejected as arbitrary middle ground.
- **§5.2 — Promotion mechanism:** **Option C.** Both CLI (`quorum convention promote <hash>`) and TUI (`p` from a new dismissal-history view). Mid-review prompts (Option D) rejected — interactive blocking inside `quorum review` breaks the hook fail-open contract.
- **§5.3 — Expiry policy:** **Option B.** Asymmetric TTL. Candidates expire after 90 days without re-dismissal; `local_only` and `promoted_convention` never auto-expire (manual `prune` / `demote` only).
- **§5.4 — Bundle placement:** **Option A.** `local_only` entries concatenate into the existing 20 KB memory section under a `## Local conventions (auto-derived)` header. Per-entry cap 500 bytes (configurable). No new bundle budget allocation; no Lippa-side dependency.
- **§5.5 — Source-field allowlist:** **Option C.** Defer Lippa cloud sync. Phase 1C ships the state machine + `.quorum/conventions.md` write-back. `POST /api/v1/memory/propose` is documented as a forward-compat seam in §7 below but is NOT consumed; no Lippa-side spec dependency.

Phase 1C inherits every locked decision from Phase 1A (v1.1) and Phase 1B (v1.0) without revisiting: three crates with strict library independence, cookie auth with Bearer stub, multi-model-always with synthesized severity, `Secret` newtype, per-host keyring with `--no-keyring` file fallback, HEAD-vs-index or `CommitRange` bundle assembly, `serde_json::Value` boundary between `quorum-core` and `quorum-lippa-client`, stable 0/1/2/3 exit-code taxonomy, `tokio` `current_thread` runtime, Phase 1A trust model (`.quorum/conventions.md` trusted only when committed AND byte-identical to HEAD), Phase 1B's three-input `finding_identity_hash`, Phase 1B SQLite schema v1 (including the already-shipped `promotion_state` column with default `'candidate'`).

---

## 1. Mission and scope

> Developer reviews findings repeatedly; the same false-positive keeps coming back. After three independent dismissals of the same finding, Quorum silently promotes the dismissal to a `local_only` rule that feeds the next review's memory context — the noise stops on its own. When the developer is ready to bless the rule for the whole team, they run `quorum convention promote <hash>` (or press `p` in the TUI history view) and Quorum writes a managed block to `.quorum/conventions.md`. The file is committed normally; from that point forward the convention rides in the bundle's conventions section like any other team rule, with full Phase 1A trust-model semantics (committed-and-clean-or-ignored). No cloud writes; no Lippa-side dependency. The state machine and the audit log live in the existing `.quorum/dismissals.sqlite` (now schema v2).

Phase 1B shipped the **signal layer**: one-finding-at-a-time dismissals with reasons, identity hashes, and a recurrence counter. Phase 1C ships the **pattern layer**: how recurring dismissals become recognized local rules, and how local rules get promoted to committed project conventions.

Three states, two transitions:

```
[no entry]
     │
     │ dismissal recorded (Phase 1B behavior, unchanged)
     ▼
[candidate]                  • local SQLite row (existing dismissal row)
     │                       • NOT surfaced in bundle memory section
     │                       • one row per finding_identity_hash
     │                       • expires after 90 days idle (§5.3 lean B)
     │
     │ AUTO transition when recurrence_count ≥ candidate_threshold (default 3)
     │ Trigger fires inside record_seen() at bump time.
     ▼
[local_only]                 • applied to bundle memory context (§5.4 lean A)
     │                       • appears in "## Local conventions (auto-derived)"
     │                       • never expires automatically
     │                       • NEVER sent to Lippa (Phase 1C scope)
     │
     │ EXPLICIT transition via `quorum convention promote <hash>` or TUI `p`
     │
     ▼
[promoted_convention]        • written as a managed block to .quorum/conventions.md
                             • tracked in conventions table (SQLite)
                             • never expires
                             • Phase 1A bundle trust model applies (committed-and-clean)
                             • Lippa cloud sync deferred to a future phase (§5.5 lean C)
```

The mission boundary: ship the pattern layer with **zero new network surface**. Lippa cloud sync was a "directionally important" capability hinted in CLAUDE.md ("cloud writes only on explicit promote OR repeated dismissals OR admin approval"), but the user-facing value of Phase 1C is "the same dismissed thing stops bothering me, and I can promote it into a real rule." That value ships under Option C without cloud. Phase 1D (or its successor name) picks up cloud sync + Lippa-side spec + `/memory/propose` consumption.

---

## 2. Non-goals (Phase 1C scope discipline)

- **No Lippa-side `/api/v1/memory/propose` consumption.** Endpoint shape is documented in §7 as a forward-compat seam (carried into the `conventions` table schema so a future Phase 1D doesn't need a v2→v3 migration), but no `POST` is issued. No `proposed_by_model`/`confidence` plumbing in Phase 1C beyond a frozen schema column.
- **No embedding-based fuzzy hash matching.** Phase 1B's exact-hash `finding_identity_hash` (title + source.type + sorted models) is the matching primitive throughout 1C. Semantic near-duplicates count separately. Phase 3 picks up `sqlite-vec` for fuzzy.
- **No identity-hash redesign.** The three-input hash is fixed. Phase 1B's measured residual cross-finding collision rate (0/190 on the v1.0 fixture set) is the carry-forward baseline; Phase 1C adds no new inputs.
- **No cross-repo / cross-workspace convention sharing.** `.quorum/conventions.md` is per-repo. Workspace-level conventions are Lippa-side or Phase 2+.
- **No LLM-generated rule synthesis.** Promotion writes a managed block with auto-derived text (title + body excerpt) plus optional user-provided text via `--text` / TUI editor; Quorum does not call out to a model to "improve" the convention prose. Separate spec if ever pursued.
- **No convention versioning beyond the SQLite state log.** Demote-and-repromote produces a new block with a new timestamp. No semantic version field on conventions.
- **No team-level or per-org convention scoping.** Lippa-side concern (or Phase 1D's cloud sync conflict resolution).
- **No cloud sync conflict resolution.** Deferred with §5.5 lean C.
- **No TUI re-fetch.** Inherits Phase 1B's quit-and-re-invoke workflow. A new dismissal-history view ships, but it does not refresh against the live SQLite mid-session — opened state is a snapshot.
- **No `quorum convention promote --interactive` mass-flow.** CLI promotion is one-hash-at-a-time. Bulk promotion is Phase 1C v2 or deferred.
- **No automatic promotion to `promoted_convention` from recurrence alone.** Per scoping note P3: only explicit user action moves an entry to `promoted_convention`. The CLAUDE.md "repeated dismissals" cloud-write trigger applies to the candidate→local_only path, NOT local_only→promoted_convention.
- **No hook-mode promote.** `quorum convention promote` is a manual command. Hooks never promote.
- **No removal of Phase 1B's `recurrence_count` semantics.** The Phase 1B counter is the same counter Phase 1C reads for the threshold check.

---

## 3. State machine (formal definition)

### 3.1 States

| State | Storage | Bundle visibility | Auto-expire | Lippa write |
|---|---|---|---|---|
| `candidate` | `dismissals.promotion_state = 'candidate'` (Phase 1B default) | hidden | 90 d idle (default; configurable) | never |
| `local_only` | `dismissals.promotion_state = 'local_only'` | memory section (§5.4 lean A) | never | never (Phase 1C) |
| `promoted_convention` | `dismissals.promotion_state = 'promoted_convention'` + a row in new `conventions` table | conventions section (via `.quorum/conventions.md` write-back; Phase 1A trust model applies) | never | never (Phase 1C; Phase 1D forward-compat seam — §7) |

The `promotion_state` column already exists in Phase 1B schema v1 with a CHECK constraint covering all three values; Phase 1C just begins writing the non-default values.

### 3.2 Transitions

**T1: candidate → local_only (AUTO).**
- Trigger: `record_seen()` bumps `recurrence_count` to a value `≥ candidate_threshold` (default 3).
- Condition: row's current `promotion_state == 'candidate'`.
- Action: same `record_seen()` transaction sets `promotion_state = 'local_only'`, writes an audit row to `state_transitions` (§4.2), and emits a stderr informational note `quorum: dismissal <short-hash> auto-promoted to local convention (recurrence=N)`.
- Idempotency: the condition `current_state == 'candidate'` is part of the UPDATE's WHERE clause; concurrent `record_seen` invocations cannot double-fire the transition (single audit row guaranteed by `UNIQUE(hash, from_state, to_state, ts)` on the audit table — see §4.2).

**T2: local_only → promoted_convention (EXPLICIT).**
- Trigger: `quorum convention promote <hash> [--text <s>]` (CLI) or `p` keypress in the TUI dismissal-history view.
- Condition: row's current `promotion_state == 'local_only'`. Promoting from `candidate` directly is rejected (`error: <hash> is still a candidate (recurrence=K of N); use --force to bypass`); `--force` raises to `promoted_convention` from any non-promoted state (a §10 Q11 carve-out from scoping note P2; keep / remove / constrain TBD post-peer-review).
- Action: in a single SQLite transaction:
  1. Update `dismissals.promotion_state = 'promoted_convention'`.
  2. Insert into `conventions` table (`hash`, `convention_text`, `promoted_at`, `promoted_by_user`, `conventions_md_block_id`).
  3. Insert audit row into `state_transitions`.
  4. Update `.quorum/conventions.md` — append a managed block (§4.3) within the `<!-- quorum:managed-section -->` fence (auto-created if absent).
  5. If `.quorum/conventions.md` did not exist, create it with just the fence + the new block.
- Failure modes: SQLite transaction is wrapped around the file-write attempt; on file-write IO error, transaction rolls back and the user sees `error: failed to update .quorum/conventions.md: <io error>; state unchanged`. The file is written via a temp-file + rename pattern (write-temp + `fs::rename`) to avoid leaving the conventions file in a partial state.

**T3 (auxiliary): promoted_convention → local_only (DEMOTE).**
- Trigger: `quorum convention demote <hash>`.
- Action: removes the managed block from `.quorum/conventions.md`; clears the `conventions` row; sets `dismissals.promotion_state = 'local_only'`; writes audit row.
- Rationale: the demote path exists because conventions.md is a committed file and the user may regret a promotion; demote-then-commit produces a clean diff. Demote is the only out-of-state-machine-spine transition exposed in 1C.

**T4 (auxiliary): any state → tombstone (PRUNE).**
- Trigger: `quorum convention prune [--state candidate|local_only] [--older-than N[d|w|m]]`. Default is `--state candidate --older-than 90d` (matches §5.3 lean B's auto-expire). `promoted_convention` cannot be pruned — must be demoted first.
- Action: DELETE from `dismissals` (cascade-deletes from `state_transitions`).
- Audit retention: the audit log is also deleted for the pruned row. A separate `audit_archive` JSONL file is NOT shipped in 1C (would expose dismissal notes to the filesystem; Phase 1B's audit-trail leak surface rules apply).

**T5 (boundary): undismiss.**
- Phase 1B's `MemoryStore::delete(hash)` already removes a dismissal row. In 1C, `delete` cascades through `state_transitions` and `conventions`. If the row was in `promoted_convention`, `delete` ALSO removes the managed block from `.quorum/conventions.md` (the file write must succeed for the transaction to commit, same temp-file-rename pattern as T2).
- The TUI's existing undo stack from Phase 1B is unchanged in behavior: an undo of a dismissal removes the candidate row before any auto-transition could fire (recurrence=1 → row deleted). Undoing a dismissal that has since auto-promoted (e.g. on a session boundary) is OUT OF SCOPE — the undo stack is per-session and per-Phase 1B v1.0 §3 already discards on TUI exit.

### 3.3 Auto-promote timing

The candidate→local_only auto-transition fires inside `record_seen()`, which runs during the bundle-assembly filter pass at the start of every `quorum review`. The transition therefore happens BEFORE the bundle is sent to Lippa. A newly-promoted `local_only` entry is included in the SAME review's memory context that triggered its promotion — the developer sees the auto-promote stderr note and the local convention takes effect immediately, on the very review whose dismissal-recurrence pushed it over the threshold.

The auto-promote does NOT re-run `quorum review` to apply the new local rule to the current findings; the bundle is built once. The local convention takes effect on the NEXT review.

### 3.4 Threshold configuration

`.quorum/config.toml` gains a `[memory]` section:

```toml
[memory]
# Number of independent dismissals of the same finding_identity_hash
# before auto-promotion from candidate to local_only.
# Range: 2..100. Default: 3. Lower values are noisier; higher values
# delay the noise-suppression payoff.
candidate_threshold = 3

# Per-entry byte cap for local_only entries in the bundle memory
# section. Bounded above by BUDGET_MEMORY = 20 KB total (§5.4 side-effect).
# Range: 100..2048. Default: 500.
local_convention_bundle_cap = 500

# Days of inactivity before a candidate row auto-expires under
# `quorum convention prune` defaults. Set to 0 to disable auto-expire.
# Range: 0..3650. Default: 90.
candidate_expire_days = 90
```

`config.toml` is not present by default — `quorum link` writes it. Phase 1C reads it with all-defaults fallback when absent; no warning. Out-of-range values produce a CLI error (exit 2 — config error) on the next `quorum review` or `quorum convention *` invocation.

---

## 4. Data model (SQLite v1 → v2 migration)

### 4.1 Migration

Phase 1B shipped schema v1 (per `SERVICES.md` §6). Phase 1C is a v1 → v2 migration, added to the existing migration runner in `quorum-core::memory`. The migration is idempotent (re-running it after a partial failure is safe — guarded by `IF NOT EXISTS` and a `schema_version` row update at the end of the transaction).

Migration steps (single transaction):

1. `CREATE TABLE IF NOT EXISTS state_transitions (...)` — see §4.2.
2. `CREATE TABLE IF NOT EXISTS conventions (...)` — see §4.3.
3. `CREATE INDEX IF NOT EXISTS idx_state_transitions_hash ON state_transitions(finding_identity_hash, ts)`.
4. `UPDATE schema_version SET version = 2 WHERE version = 1`.

No backfill of `state_transitions` for pre-v2 dismissals: Phase 1B rows have no audit trail and stay that way. The audit log starts at the v2 migration; reading transitions for a pre-v2 row returns an empty list. This is documented and explicitly tested.

The migration runs on first `quorum review` (or `quorum convention *`) after a Phase 1B→1C binary upgrade. Failure to migrate is a hard error (exit 2 — tooling); the user is told to file a bug and not pointed at a workaround that would silently downgrade.

### 4.2 `state_transitions` table

```sql
CREATE TABLE state_transitions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  finding_identity_hash BLOB NOT NULL REFERENCES dismissals(finding_identity_hash) ON DELETE CASCADE,
  from_state TEXT NOT NULL CHECK (from_state IN ('candidate','local_only','promoted_convention')),
  to_state   TEXT NOT NULL CHECK (to_state   IN ('candidate','local_only','promoted_convention','deleted')),
  trigger    TEXT NOT NULL CHECK (trigger    IN ('auto_recurrence','explicit_promote','explicit_demote','explicit_undismiss','force_promote')),
  ts         INTEGER NOT NULL,           -- unix epoch millis
  by_review_session_id TEXT,             -- session id at the time of transition; nullable for non-review-driven transitions
  recurrence_at_transition INTEGER,      -- recurrence_count value at the moment of the transition; nullable
  UNIQUE (finding_identity_hash, from_state, to_state, ts)
);
CREATE INDEX idx_state_transitions_hash ON state_transitions(finding_identity_hash, ts);
```

Notes:
- `to_state` includes `'deleted'` (a virtual state for T5 — `undismiss`) so the audit trail captures dismissal removal without leaking the row reference.
- `UNIQUE(hash, from_state, to_state, ts)` is the idempotency guard for T1's transactional UPDATE (concurrent `record_seen` calls would otherwise race the audit insert).
- `ON DELETE CASCADE` from dismissals: pruning a candidate row also drops its audit log. This is by design — the audit log carries no notes/body text but does carry the hash, and we don't want orphan audit rows for findings that no longer have any dismissal evidence.

### 4.3 `conventions` table

```sql
CREATE TABLE conventions (
  finding_identity_hash BLOB PRIMARY KEY REFERENCES dismissals(finding_identity_hash) ON DELETE CASCADE,
  convention_text TEXT NOT NULL CHECK (length(convention_text) BETWEEN 1 AND 4096),
  promoted_at INTEGER NOT NULL,           -- unix epoch millis
  promoted_by_user TEXT,                  -- best-effort: git config user.email at promote time
  conventions_md_block_id TEXT NOT NULL,  -- short hex hash (first 12 chars of finding_identity_hash); used to key the markdown block
  -- Forward-compat for Phase 1D / §7. Frozen as nullable in 1C; never written.
  lippa_memory_item_id TEXT,              -- the id returned by POST /api/v1/memory/propose (Phase 1D)
  lippa_proposed_at INTEGER,              -- unix epoch millis of successful cloud write (Phase 1D)
  UNIQUE (conventions_md_block_id)
);
```

Notes:
- `convention_text` is the *body text* of the managed markdown block (not the full block including delimiters/header). Cap is 4096 chars — generous over the 500-byte bundle cap, because conventions.md is a separate bundle section with its own 10 KB budget (`SERVICES.md` §2 `BUDGET_CONVENTIONS`).
- `lippa_memory_item_id` / `lippa_proposed_at` are nullable columns frozen as forward-compat for Phase 1D cloud sync (§7). Phase 1C does not read or write these columns; tests assert `NULL` on every row.
- `promoted_by_user` is best-effort; falls back to `NULL` if git config has no `user.email`. The CLI does NOT prompt for an identifier — privacy and friction trade-off.

### 4.4 `conventions.md` write-back format

`.quorum/conventions.md` is a user-owned file that pre-dates Phase 1C (Phase 1A bundle input). Phase 1C must coexist with arbitrary user content. The chosen format isolates Quorum-managed blocks behind a marker fence and keys each block by short-hash:

```markdown
<existing user content, untouched>

<!-- quorum:managed-section v=1 -->
<!-- DO NOT EDIT MANUALLY: this section is rewritten by `quorum convention promote/demote`. -->
<!-- To remove an auto-derived convention, run `quorum convention demote <hash>`. -->

<!-- quorum:convention id=a1b2c3d4e5f6 v=1 -->
### Convention: do not block on stylistic ABC

Function bodies under 8 lines do not require docstrings; the project
treats this as a settled stylistic call after three independent
dismissals on review.
<!-- /quorum:convention -->

<!-- quorum:convention id=9876fedcba01 v=1 -->
### Convention: ignore TODO-without-issue-ref findings

The team consciously uses bare TODOs as ephemeral local markers; the
"missing issue ref" finder is suppressed.
<!-- /quorum:convention -->

<!-- /quorum:managed-section -->
```

Rules:
- The marker section is appended once on first promotion if absent. Quorum NEVER edits content above `<!-- quorum:managed-section v=1 -->` or below `<!-- /quorum:managed-section -->`.
- Each managed block is keyed by `id=<12-char-hex-prefix-of-finding_identity_hash>`. Demote removes the block by id. Promote-again on the same hash REPLACES the block (idempotent re-promote).
- `v=1` on the section and each block is a format version. Format upgrades in future phases will introduce `v=2` blocks; v=1 blocks remain readable.
- Block body is plain markdown; no Quorum-specific tags inside. The `### Convention: <title>` header is auto-derived from `Finding.title` at promote time; the body comes from user-provided `--text` if present, else from the truncated `body_snapshot` from Phase 1B's dismissal row.
- Line endings are normalized to the file's existing convention (LF preserved on Unix-authored files, CRLF preserved on Windows-authored files). On a new file, defaults to LF.
- Quorum writes the file with a `quorum-managed-conventions-md` marker as the FIRST line of a freshly-created file: `<!-- quorum-managed-conventions-md v=1 -->`. This is solely for the conventions-md-modified-outside-quorum detection (§4.5); the trust model for the bundle layer is independent and rides on Phase 1A's committed-and-clean check.

### 4.5 Merge-conflict discipline

Conventions.md is committed by the user and may produce merge conflicts when two developers promote different findings on the same day. The section-fenced, block-keyed format minimizes pain:
- Different `id=` blocks NEVER conflict structurally — git's line-based merge handles them cleanly because each block is a contiguous block surrounded by HTML-comment delimiters with unique ids.
- The same `id=` block CAN conflict if two developers promote the same hash with different `--text` bodies on parallel branches. This is a rare but real semantic merge — the developer resolves it manually as plain markdown. No special tooling in 1C.
- `quorum convention promote` does NOT auto-commit conventions.md. The user runs `git add .quorum/conventions.md && git commit` after promotion. The Phase 1A trust model (committed-and-clean-or-ignored) means an uncommitted convention does not yet ride in the bundle.

### 4.6 Audit-trail leak surface

Per Phase 1B (`SERVICES.md` §6 last bullet), the dismissals store keeps free-text notes off-disk-outside-the-gitignored-DB. Phase 1C extends:
- `state_transitions` carries the hash + timestamps + trigger + recurrence — no titles, no notes, no bodies.
- `conventions.convention_text` IS the convention body and is intentionally user-readable (it lands in conventions.md, which IS committed). But: `convention_text` is *user-supplied or title-derived*, NOT the dismissal `note` field. Phase 1B's `note` (free-text rationale, ≤2 KB, possibly sensitive) is NEVER copied into `conventions.convention_text` automatically. The promote command MUST take either explicit `--text` from the CLI user OR fall back to the title-derived placeholder; it does NOT silently lift the dismissal note.

---

## 5. User-facing surface

### 5.1 CLI subcommands

Phase 1C adds one new top-level subcommand group: `quorum convention`. Subcommands:

| Command | Effect | Exit codes (Phase 1A taxonomy) |
|---|---|---|
| `quorum convention list [--state {candidate,local_only,promoted_convention}] [--json]` | Prints rows from `dismissals` joined with current state. With `--json`, prints a JSON array (one element per row) to stdout suitable for piping. | 0 ok / 2 tooling |
| `quorum convention show <hash>` | Prints title + body_snapshot + state + transition history for one row (resolved by short-hash prefix; ambiguous prefix → exit 2). | 0 ok / 2 tooling |
| `quorum convention promote <hash> [--text <s>] [--from-editor] [--force]` | Executes T2 (or T2 with `--force` from any non-promoted state — `--force` is a Q11 carve-out). `--from-editor` opens `$EDITOR` with the title-derived default; rejects `--text` and `--from-editor` together. | 0 ok / 1 unused / 2 tooling (state error, missing hash, IO) / 3 unused |
| `quorum convention demote <hash>` | Executes T3 (promoted → local_only). Removes the managed block from conventions.md. | 0 ok / 2 tooling |
| `quorum convention prune [--state {candidate,local_only}] [--older-than <duration>] [--dry-run]` | Executes T4. Default behavior: `--state candidate --older-than 90d`. `--dry-run` prints the list of rows that would be pruned with no SQL writes. | 0 ok / 2 tooling |
| `quorum convention history <hash>` | Prints the `state_transitions` log for one row, oldest-first. Useful for "why is this a local_only?" forensics. | 0 ok / 2 tooling |

All `convention` subcommands honor `--quorum-dir <path>` (Phase 1A) for tests. None require network — all are pure SQLite + filesystem.

Short-hash resolution: hex prefix of `finding_identity_hash` of length ≥ 8; ambiguous prefix (matches > 1 row) → exit 2 with disambiguation hint. The full 64-hex form is always accepted.

### 5.2 TUI changes

The Phase 1B TUI (ratatui + crossterm; list + body + status bar) gains ONE new view and TWO new keybindings.

**New view: dismissal-history (`H` from the main list view).**
- Shows all rows from `dismissals` ordered by `last_seen_at DESC`.
- Columns: short-hash (8 chars), state (1 char: `c` / `L` / `P`), recurrence (N), reason, title (truncated to fit width).
- Body pane shows the highlighted row's title + body_snapshot + state + most recent 5 transition log entries.
- Status bar shows: `H back | p promote | d demote | x delete | / filter | ?  help`.
- Exits to main view via `H` (toggle), `Esc`, or `q` (which now quits the TUI from any view).

**New keybinding `p` (history view): promote highlighted entry.**
- Opens a single-line modal: `Convention text (Enter to accept default, Esc to cancel):` with a default value derived from `Finding.title`. The user types or accepts.
- On Enter: T2 transition fires (or rejects if state ≠ local_only — error shown in the status bar; `Shift+P` invokes the `--force` path).
- On success: status-bar transient confirmation `promoted <short-hash> to convention; wrote .quorum/conventions.md`.

**New keybinding `d` (history view): demote highlighted entry.**
- Confirmation modal: `Demote <short-hash>? Y/N` (capital Y required).
- On `Y`: T3 transition fires.

**Main list view (`q` / `H` only).**
The main per-review list view from Phase 1B is unchanged except for the addition of `H` to open the history view and the addition of the auto-promote stderr informational note (rendered to the body of the new local-only entry in the next review's bundle, not the current TUI session).

### 5.3 `quorum review` integration

- The bundle assembly step (`quorum-core::bundle::assemble`) consults `MemoryStore::load_local_only_conventions()` and concatenates the resulting entries (each truncated to `local_convention_bundle_cap` bytes) into the memory section under a `## Local conventions (auto-derived)` header. The 20 KB `BUDGET_MEMORY` is shared with CLAUDE.md / AGENTS.md / .cursorrules; overflow produces the standard `[memory truncated …]` marker from `SERVICES.md` §2.
- The per-entry truncation marker, when applied, is `[local convention truncated: <hash-short>, <bytes-elided> bytes elided; raise [memory] local_convention_bundle_cap to see full text]` and counts toward the 20 KB total.
- The auto-promote stderr informational note (§3.2 T1) fires from `quorum-core::memory::record_seen` via a callback or returned event; the CLI prints to stderr. Note is suppressed under `--hook-mode=*` (parity with Phase 1B's QUORUM_LIPPA_SESSION precedence note).

### 5.4 Config keys (added)

```
.quorum/config.toml
└── [memory]
    ├── candidate_threshold       (int, 2..100, default 3)
    ├── local_convention_bundle_cap (int, 100..2048, default 500 bytes)
    └── candidate_expire_days     (int, 0..3650, default 90; 0 disables auto-expire)
```

`config.toml` parsing in Phase 1A's `link` command is extended to validate these. Out-of-range values are surfaced at the `link` site AND on the next `quorum review` (config is re-validated on read; the configuration store is not cached across invocations).

### 5.5 Help text

`quorum --help` gains the `convention` subcommand group. `quorum convention --help` produces the table from §5.1 plus the standard `-h` / `--help` / `--quorum-dir` flags. The dismissal-history view is documented in `quorum review --help` under "TUI keybindings" and in the README's TUI section.

---

## 6. Bundle assembly impact

### 6.1 Memory section layout

The 20 KB `BUDGET_MEMORY` from `SERVICES.md` §2 is consumed in this order (largest-first within each group does not apply here — memory section is order-sensitive):

1. CLAUDE.md / AGENTS.md / .cursorrules (Phase 1A behavior, unchanged).
2. `## Local conventions (auto-derived)` header (~40 bytes).
3. Each `local_only` entry rendered as:

   ```markdown
   ### Local convention: <Finding.title (truncated to 80 chars)>
   <body_snapshot truncated to local_convention_bundle_cap bytes, UTF-8 codepoint boundary>
   <!-- recurrence=N, since=<ISO yyyy-MM-dd>, hash=<short> -->
   ```

   Entries are sorted by `recurrence_count DESC, last_seen_at DESC` so the most-recurring rules sit at the top.

4. If summing the entries exceeds `BUDGET_MEMORY` minus the budget already consumed by step 1, truncation happens at the entry boundary (NOT mid-entry), and the standard `[memory truncated: N bytes exceeded 20KB; review only top portion]` marker (`SERVICES.md` §2) is appended.

5. Per-entry truncation marker (when an individual `body_snapshot` is too long to render in full at `local_convention_bundle_cap`) is the `[local convention truncated: …]` form from §5.3.

### 6.2 Conventions section is unchanged in shape

`promoted_convention` entries live in `.quorum/conventions.md` and ride the existing 10 KB `BUDGET_CONVENTIONS` (`SERVICES.md` §2). Phase 1C does NOT bypass the Phase 1A trust model: an uncommitted promotion (i.e. conventions.md modified-but-not-committed) is IGNORED by the bundle. The dismissal's state is still `promoted_convention` in SQLite, but the file's contribution to the bundle hinges on the commit-and-clean check.

This is intentional. The CLI-level act of "promoting" is a local SQLite + file-write operation; the rule does not become *bundle-visible as a convention* until the user commits conventions.md. Between promotion and commit, the rule is still visible as a `local_only` entry would have been — except it isn't, because the auto-derived block has already been added to conventions.md, which when committed-and-clean is the source of truth.

Concretely: in the "promoted-but-not-yet-committed" window, the rule is INVISIBLE to the bundle (Phase 1A trust model says ignore uncommitted conventions.md; Phase 1C's `local_only` path is no longer triggered because the SQLite state has moved). This is a small UX cost; the spec accepts it. Users are reminded at promote time: `promoted <hash>; commit .quorum/conventions.md to apply`.

### 6.3 Per-entry cap rationale (§5.4 lean A side-effect)

The 500-byte default per-entry cap exists so a single verbose dismissal doesn't crowd out CLAUDE.md inside the 20 KB memory section. The cap is configurable (100..2048 range) for users who deliberately want longer rules to ride in the bundle; the upper bound of 2048 matches the `body_snapshot` storage cap from Phase 1B (so no entry truncation can lose information that the dismissal didn't already discard).

### 6.4 Overflow markers (Phase 1A discipline preserved)

Every truncation point produces a visible marker; no silent truncation. The Phase 1A "no silent truncation" rule from `SERVICES.md` §2 holds for the new entry shape too.

---

## 7. Lippa integration (forward-compat seam only — §5.5 lean C)

Phase 1C makes **NO** outbound Lippa network calls for memory/convention writes. The Lippa `/api/v1/auth/login`, `/api/v1/consensus/sessions`, `/api/v1/me` surfaces from Phase 1A/1B remain unchanged. `POST /api/v1/memory/propose` is documented here so Phase 1D's cloud-sync work doesn't need a schema migration on the Quorum side.

### 7.1 Endpoint (read-only orientation — confirmed via `../lippa/apps/api/app/routes/api_v1/memory.py` and `app/schemas/memory_propose.py`)

```
POST /api/v1/memory/propose
Content-Type: application/json
Cookie: session=<value>

Request body (ProposeRequest, source-of-truth: Lippa schemas/memory_propose.py):
{
  "scope":           "account" | "space" | "project",
  "scope_id":        string | null,           // required for space/project; null for account
  "item_type":       "decision" | "constraint" | "convention" | "task" | "open_question",
  "content":         string (1..500 chars),   // the convention text
  "source":          "model_proposed" | "user_directed",
  "conversation_id": string,                  // ties the proposal to a conversation/turn
  "turn_id":         string,
  "excerpt":         string (1..2000 chars),  // evidence anchor
  "proposed_by_model": string | null (≤128),  // required-shape for source=model_proposed per spec
  "confidence":        float | null (0..1)
}

Response: 200 OK (ProposeResponse)
{
  "status":          "auto_approved" | "auto_approved_with_conflict" | "pending_review" |
                     "duplicate_merged" | "endorsed" | "endorsement_refused" |
                     "account_conflict_pending_ack" | "near_dup_suggested_merge",
  "memory_item_id":  string | null,           // null on endorsement_refused / account_conflict_pending_ack
  ...                                         // additional fields not enumerated here; Phase 1D recons full shape
}

Error surface: 400 (content policy), 403 (recency / rate / feature gates), 404 (ownership miss), 422 (malformed).
```

### 7.2 Source-field allowlist alignment (CLAUDE.md hard constraint)

CLAUDE.md mandates: "When writing memory items to Lippa, the `source` field MUST be `model_proposed` with `proposed_by_model` and `confidence` set. Never invent new source values."

Phase 1D (or its successor) MUST map `quorum convention promote` writes to `source: "model_proposed"` with `proposed_by_model = <the model that flagged the underlying finding>` and `confidence = 1.0` (user-affirmed). This stretches the semantic of `model_proposed` to also cover user-affirmed model-flagged findings; the alternative of pushing for a `user_promoted` allowlist addition on Lippa (scoping §5.5 option B) is a separate-phase decision.

Phase 1C bakes this into the schema:
- `conventions.lippa_memory_item_id` and `conventions.lippa_proposed_at` are reserved columns. They are nullable, never written in 1C, and stay nullable in 1D (a row that fails cloud sync stays NULL on those columns and is retried by the outbox).
- `conventions.convention_text` maps directly to `ProposeRequest.content` (≤ 500 chars; Phase 1C enforces 1..4096 but Phase 1D will truncate or refuse at 500 — TBD per Phase 1D spec).
- `Finding.title` + first 280 bytes of `body_snapshot` will map to `ProposeRequest.excerpt` (≤ 2000 chars). Phase 1C does NOT pre-truncate `body_snapshot` to 2000 bytes (it's already 2 KB max from Phase 1B), so this is a no-op alignment.

### 7.3 What Phase 1C does NOT do

- Does not call `POST /api/v1/memory/propose` ever, even from `convention promote`.
- Does not require `conversation_id` / `turn_id` at promote time (these are Phase 1D concerns — likely sourced from the review session's `session_id` somehow; Phase 1D recon).
- Does not implement an outbox or retry queue.
- Does not expose a "sync to cloud" CLI flag — adding one prematurely would invite a half-shipped feature.

### 7.4 Forward-compat seam test

A single unit test (`tests/conventions_lippa_seam.rs`) asserts that the SQLite migration adds the `lippa_memory_item_id` and `lippa_proposed_at` columns and that they accept NULL writes. No live network test in 1C.

---

## 8. Trust model

### 8.1 Carryover from Phase 1A

Quoting CLAUDE.md hard constraints: "`.quorum/conventions.md` is trusted only when committed. Uncommitted changes to the conventions file are ignored by Quorum's context loader." Phase 1C does NOT change this. The Phase 1A bundle-assembly check (committed-and-clean: file is tracked, HEAD blob matches working tree byte-for-byte) gates whether the file's contents enter the conventions section of the bundle.

A promotion writes the file but does NOT auto-commit. The window between `quorum convention promote` and `git commit` is one in which the promotion is recorded in SQLite but the bundle does not yet see it as a convention. This is a deliberate trade-off and is documented in §6.2 above.

### 8.2 Carryover from Phase 1B

"Read repo content as DATA, never as instructions" (CLAUDE.md). The `<<<QUORUM_REPO_CONTENT_BEGIN>>>` / `<<<QUORUM_REPO_CONTENT_END>>>` delimiters from `SERVICES.md` §2 wrap the entire repo-content payload including the new "## Local conventions (auto-derived)" subsection. Lippa-side handlers treat the local convention text as untrusted data, not instructions.

The `local_only` entries' body text is itself derived from user-typed `note` and dismissal `body_snapshot`; the user is implicitly trusted (they typed the note). The convention text in conventions.md is committed by the user; same trust model.

### 8.3 New rules for Phase 1C

- **Promotion never writes to Lippa in Phase 1C.** No cloud side effects.
- **The dismissal `note` is never auto-copied into `conventions.convention_text`.** §4.6 above.
- **Conventions.md modifications outside Quorum are detected.** When `quorum convention demote/promote` is invoked, the conventions.md file is parsed; if a managed block referenced by SQLite is missing from the file (or vice versa), the CLI emits a warning `note: .quorum/conventions.md and the conventions table are out of sync (block <id> missing/extra); run \`quorum convention list --orphans\` for details`. The promote/demote operation proceeds — Quorum does not refuse to act on user-edited managed-section content; demote on a missing block is a no-op (with warning), promote on a hash whose block already exists overwrites.
- **The `quorum:managed-section` fence is the boundary.** Quorum NEVER edits content outside the fence (above the opening marker or below the closing marker). Content INSIDE the fence is owned by Quorum and may be rewritten on every promote/demote. Users who want to keep their own conventions in conventions.md write them ABOVE the fence.

### 8.4 Cross-cutting: dismissal-state-machine writes are local-only

Cloud writes (when they eventually exist) go through the §5.5 → Phase 1D pipeline. Phase 1C's state machine is entirely local. There is no admin-approval path in 1C either (CLAUDE.md cloud-write trigger #3); admin approval is also Phase 1D.

---

## 9. Acceptance criteria (numbered, testable)

Phase 1B closed at AC 131; AC 132 (sigstore attestation) flipped FULL in 0.2.1. Phase 1C ACs begin at 133.

**State machine (T1 / T2 / T3 / T4 / T5):**

- AC 133 — Phase 1B dismissals behavior preserved. The Phase 1B integration suites pass unchanged after the v1→v2 migration. (Regression.)
- AC 134 — Every new dismissal creates a `dismissals` row with `promotion_state = 'candidate'` and no `state_transitions` entry (the candidate state is implicit / not logged as a transition).
- AC 135 — T1 fires inside `record_seen()`: when `recurrence_count` reaches `candidate_threshold` (default 3) on a `candidate` row, the same transaction sets `promotion_state = 'local_only'` and inserts exactly one `state_transitions` row with `from_state='candidate'`, `to_state='local_only'`, `trigger='auto_recurrence'`.
- AC 136 — T1 is idempotent under repeated `record_seen()` calls past the threshold: a row already in `local_only` is not re-transitioned; no duplicate audit rows are produced.
- AC 137 — `quorum convention promote <hash>` on a `local_only` row sets `promotion_state = 'promoted_convention'`, inserts a `conventions` row, inserts a `state_transitions` row with `trigger='explicit_promote'`, and appends a managed block to `.quorum/conventions.md`. Without `--force`, promoting from `candidate` directly is rejected (exit 2).
- AC 138 — `quorum convention promote <hash> --force` from `candidate` produces the same end state as AC 137 but the audit row's `trigger='force_promote'`.
- AC 139 — `quorum convention promote <hash> --text "<s>"` writes `<s>` to `conventions.convention_text` (subject to 1..4096 length check). With no `--text` and no `--from-editor`, the title-derived placeholder is used. The dismissal `note` field is NEVER read by promote.
- AC 140 — `quorum convention demote <hash>` on a `promoted_convention` row sets `promotion_state = 'local_only'`, removes the row from `conventions`, removes the managed block from `.quorum/conventions.md`, and inserts a `state_transitions` row with `trigger='explicit_demote'`.
- AC 141 — `quorum convention prune` defaults to `--state candidate --older-than 90d`. With `--dry-run`, it prints the candidates that would be deleted and writes nothing. Without `--dry-run`, it DELETEs the matched rows (and their cascade-deleted audit rows).
- AC 142 — `quorum convention prune` refuses to operate on `promoted_convention` rows; the user must demote first.
- AC 143 — `MemoryStore::delete(hash)` (undismiss; existing Phase 1B surface) on a `promoted_convention` row cascades: removes managed block from conventions.md within the same SQLite transaction (temp-file-rename pattern; transaction rolls back on IO error).
- AC 144 — `state_transitions.UNIQUE(hash, from_state, to_state, ts)` prevents duplicate audit rows under concurrent `record_seen` invocations (verified by a parallel-invocation test using two `tokio::spawn_blocking` workers writing the same hash).

**Data model:**

- AC 145 — The v1→v2 migration runs at first `quorum review` (or `quorum convention *`) post-binary-upgrade. The migration is idempotent (re-running it after a partial failure is safe) and updates `schema_version` to 2.
- AC 146 — `state_transitions` carries `ON DELETE CASCADE` from `dismissals(finding_identity_hash)`. Pruning a candidate row also drops its audit log.
- AC 147 — `conventions.lippa_memory_item_id` and `conventions.lippa_proposed_at` are reserved nullable columns; Phase 1C never writes a non-NULL value. (Asserted by a unit test reading the column types and a row dump.)
- AC 148 — The `conventions.md` write-back uses the section-fenced, block-keyed format from §4.4. Demote-then-repromote on the same hash produces a single managed block (not two).
- AC 149 — Content above `<!-- quorum:managed-section v=1 -->` and below `<!-- /quorum:managed-section -->` in conventions.md is preserved byte-for-byte across promote/demote operations.
- AC 150 — Line endings in conventions.md are preserved: a file authored with CRLF stays CRLF after a promote; a file authored with LF stays LF. A new file (Quorum-created) defaults to LF.
- AC 151 — A `convention_text` of 4097 bytes is rejected at the SQLite CHECK; a 1-byte text is accepted.

**Bundle assembly:**

- AC 152 — `local_only` entries appear in the bundle memory section under the `## Local conventions (auto-derived)` header, sorted by `recurrence_count DESC, last_seen_at DESC`. CLAUDE.md / AGENTS.md / .cursorrules content precedes them.
- AC 153 — Per-entry rendering uses the format from §6.1 step 3 (title-truncated, body-truncated at `local_convention_bundle_cap`, trailing recurrence/since/hash HTML comment).
- AC 154 — Per-entry truncation emits the `[local convention truncated: <hash-short>, <bytes-elided> bytes elided; …]` marker; total-section truncation emits the existing `[memory truncated: …]` marker.
- AC 155 — A `promoted_convention` row whose conventions.md has been promoted-but-not-committed is INVISIBLE to the bundle (Phase 1A trust model; row's state is `promoted_convention` but neither the local_only path nor the conventions path renders it until the file is committed-and-clean).
- AC 156 — Total bundle byte budgets (Phase 1A's `BUDGET_TOTAL = 200 KB`, per-section caps) are not raised by Phase 1C. The 20 KB memory section is the shared cap.

**TUI:**

- AC 157 — Pressing `H` in the main TUI view opens the dismissal-history view (list of dismissals with state column). Pressing `H` again or `Esc` returns to the main view.
- AC 158 — In the history view, `p` on a `local_only` row prompts for convention text (default = title-derived), and on Enter executes T2: SQLite + conventions.md write + status-bar transient confirmation.
- AC 159 — In the history view, `d` on a `promoted_convention` row prompts `Y/N` confirmation and on `Y` executes T3.
- AC 160 — `Shift+P` on a `candidate` row executes T2 via the `--force` path (status-bar confirmation shows `force-promoted`); `p` (lowercase) on a `candidate` row rejects with status-bar error.
- AC 161 — TUI exit (any path) leaves the terminal restored (Phase 1B RAII guard + panic-hook chain unchanged). A panic during T2's file write also restores the terminal before the panic propagates.

**CLI / config:**

- AC 162 — `quorum convention list` prints rows by state with short-hash + title; `--state local_only` filters; `--json` produces a stable-ordered JSON array suitable for piping into `jq`.
- AC 163 — Short-hash resolution accepts any hex prefix ≥ 8 chars; ambiguous prefix returns exit 2 with a disambiguation message; full 64-hex always accepted.
- AC 164 — `.quorum/config.toml` `[memory] candidate_threshold = 0` is rejected at validation (range 2..100); same for the other config keys' ranges. Validation errors exit 2 from `quorum review` or `quorum convention *` on the first read after config change.
- AC 165 — Default config (no `config.toml` `[memory]` section) yields threshold=3, cap=500, expire=90d.

**Lippa forward-compat seam (§7):**

- AC 166 — No Phase 1C code path issues `POST /api/v1/memory/propose`. (Asserted by a `quorum-lippa-client` API-surface test that checks the call site list contains only the Phase 1A/1B endpoints.)
- AC 167 — The frozen forward-compat columns (`conventions.lippa_memory_item_id`, `conventions.lippa_proposed_at`) accept NULL writes and reject non-NULL writes from any 1C code path. (No 1C code path attempts a non-NULL write; the test is structural.)

**Trust model:**

- AC 168 — Conventions.md modifications outside Quorum (e.g., manual edit to a managed block, removing a `<!-- /quorum:convention -->` closer) are detected when the next `quorum convention promote/demote` or `quorum convention list --orphans` runs and surface as a stderr warning. The operation proceeds (Quorum does not refuse to act on user-edited managed-section content).
- AC 169 — A managed block that exists in conventions.md but has no SQLite `conventions` row is reported by `quorum convention list --orphans` (a diagnostic sub-flag) as orphaned. Default `list` does NOT show orphans.
- AC 170 — The `<<<QUORUM_REPO_CONTENT_BEGIN>>>` delimiter wraps the entire memory section including the new local-conventions subsection. (Phase 1A behavior preserved.)

**Cross-cutting (Phase 1B regression):**

- AC 171 — All 197 Phase 1B tests pass after the migration. (Tracking number; expected to grow with 1C additions; the regression assertion is "no Phase 1B test fails.")
- AC 172 — `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check --all` pass.

---

## 10. Open questions

Items the v0.1 lean-defaults left unresolved or partially deferred; v0.2 (post-peer-review) should close these.

- **Q1 — Promote-conflict-with-merge handling.** §4.5 says same-`id=` block conflicts during git merge are user-resolved manually. Should `quorum convention promote` detect that the local block diverges from what HEAD shows for the same hash (e.g., a teammate already promoted with different text) and warn at promote time? Cheap to detect; UX cost is low. Lean: yes, warn but don't refuse. Flag for v0.2.
- **Q2 — Audit log retention on undismiss / prune.** §4.2 ON DELETE CASCADE drops the audit trail. Should pruned-candidate audit rows be retained for "why does this finding keep recurring?" forensics? Argues for a separate `audit_archive` JSONL — but that re-introduces a leak surface (hash + ts is borderline; titles would be a clear violation). Lean: keep CASCADE for 1C; reconsider if user feedback asks for it.
- **Q3 — Promote inside a hook.** §5.1 table says all `convention` subcommands are 0/2-exit, no network. Should `quorum install --hook=*` ever wire up a promote path (e.g., a post-commit hook that promotes auto-flagged conventions)? Lean: no; promotion is intentional user action. Flag in §11 forward-compat for Phase 1C v2.
- **Q4 — Threshold per-source-type.** Should `candidate_threshold` be tunable per `Finding.source.type` (e.g., 2 for `false_positive`, 5 for `intentional`)? Tempting for low-cost noise control. Lean: no in v0.1 (single int simpler to explain); reconsider for v0.2.
- **Q5 — `quorum convention sync-check` shape.** §8.3 references this subcommand for "out of sync" detection. The exact shape (list orphans? prompt to reconcile?) is not specified. Lean: v0.1 ships `--orphans` flag on `list` only; `sync-check` is a v0.2 expansion.
- **Q6 — Auto-promote stderr verbosity.** §3.2 T1 prints `quorum: dismissal <short-hash> auto-promoted to local convention (recurrence=N)`. Is one line per transition right, or should multiple transitions per review batch into a summary line? Lean: one line per transition for transparency; reconsider if there's noise feedback.
- **Q7 — Demote without git working tree access.** §3.2 T3 must remove a block from conventions.md. If conventions.md doesn't exist (was deleted, was never created in this repo), what's the behavior? Lean: no-op for the file write, with stderr warning; the SQLite state changes proceed.
- **Q8 — Local convention rendering vs Lippa wire format.** §6.1's per-entry rendering format is markdown-shaped. If Lippa-side review prompts expect a different shape for local conventions specifically (vs general memory), this could need tweaking. Lean: ship markdown for v0.1; if Lippa Phase 1D recon shows otherwise, adjust.
- **Q9 — Migration rollback path.** If a user downgrades from a 1C binary back to a 1B binary, the schema is at v2 but the 1B binary asserts v1. Should the 1C migration write a `forward_compat_min_version` row so 1B fails with a clear error (vs corrupting data)? Lean: yes; cheap to add. Flag for v0.2 to formalize.
- **Q10 — Bundle byte budget revisit.** §5.4 lean A keeps the 20 KB cap. If users actually accumulate many local rules, the noise-suppression intent backfires (CLAUDE.md gets crowded out). Should the cap be raised, or `local_only` get its own sub-budget? Lean: cap stays for 1C; instrument bundle assembly to log truncation rates so Phase 2's budget revisit is data-informed.
- **Q11 — `--force` / Shift+P bypass of the recurrence gate.** `quorum convention promote --force <hash>` (CLI) and Shift+P (TUI) transition candidate→promoted_convention in one step, bypassing the §3.2 T1 auto-promote and the §5.1 candidate threshold. Scoping note P2 stated the principle "promotion to local_only requires recurrence" is fixed; --force is a power-user carve-out the scoping doc didn't anticipate. v0.1 ships --force / Shift+P pending peer-review challenge. Three paths for v0.2: (a) keep as-is, (b) remove and defer to a later phase per §11's Phase 1C v2 list, (c) constrain (e.g. require recurrence ≥ 2 before --force is permitted). Lean: keep, with explicit "you are bypassing the threshold" stderr warning at force-promote time.

---

## 11. Out of scope

(Restating §2 plus what's deferred to numbered future milestones.)

- **Phase 1D (or its successor name):** Lippa cloud sync. `POST /api/v1/memory/propose` consumption. Source-field allowlist alignment (the `source: "model_proposed"` + `confidence: 1.0` user-affirmed convention; see §7.2). Outbox / retry queue. Admin-approval path for cloud writes. `conversation_id` / `turn_id` plumbing into `convention promote`. Lippa-side spec dependency.
- **Phase 2:** Privacy-mode redaction (regex/AST first, local LLM second). Per-team / per-org convention scoping (Lippa-side). PR commenting, IDE plugins, BYO-keys mode, web dashboard. Memory-write outbox built ON TOP of `MemoryStore` (1C's local SQLite stays the sync backbone).
- **Phase 3:** Embedding-based fuzzy hash matching (`sqlite-vec`). Semantic-near-duplicate dismissals counting toward threshold. Phase 1C ships exact-hash recurrence only.
- **Phase 1C v2 (post-1C-ship, pre-1D):**
  - `quorum convention promote --interactive` (bulk-promote modal).
  - Per-source-type threshold tuning (Q4).
  - `quorum convention sync-check` standalone subcommand (Q5).
  - LLM-generated rule synthesis ("draft a convention from these dismissals").
  - TUI re-fetch / live-update of the history view.
  - Hook-mode auto-promote variants (Q3).
- **Indefinite defer / never:**
  - Multi-shell hook variants beyond `#!/bin/sh`. (Phase 1B carry-forward; same rationale.)
  - Two-factor or interactive prompts mid-`quorum review`. (Breaks hook fail-open contract.)
  - Convention semantic versioning. (Demote-and-repromote produces a new timestamp; that's the audit trail.)
  - Cross-repo / cross-workspace convention sharing as a Quorum-driven feature. (Lippa-side scope, or Phase 2+ if at all.)

---

*— end Phase 1C v0.1 spec —*
