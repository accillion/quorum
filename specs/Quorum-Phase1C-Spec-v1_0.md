# Quorum — Phase 1C: Conventions-promotion state machine

**Status:** v1.0 — committed spec for Phase 1C implementation. Supersedes v0.1 (peer-reviewed draft) and v0.2 (intermediate draft applying adjudicated peer-review feedback). Implementation session pending.
**Spec home:** `specs/Quorum-Phase1C-Spec-v0_2.md`
**Driver:** Quorum (this repo). Builds on Phase 1B's dismissals foundation; external client of Lippa's `/api/v1/*` surface (no Lippa-side dependency in this milestone — no cloud writes, no Lippa-side schema seam).
**References:** `specs/Quorum-Phase1C-Spec-v0_1.md` (the v0.1 this revises); `specs/Quorum-Phase1C-Spec-v0_1-PeerReview-{A,B,C}.md` (the three peer reviews adjudicated into this draft); `specs/Quorum-Phase1C-Scoping-notes.md`; `specs/Quorum-Phase1B-Spec-v1_0.md`; `HISTORY.md` Phase 1B and 0.2.1 entries; `SERVICES.md` §2 (bundle budgets), §6 (dismissals store), §8 (DiffSource); `CLAUDE.md` hard constraints; `../lippa/apps/api/app/routes/api_v1/memory.py` + `app/schemas/memory_propose.py` (read-only — endpoint shape documented in §7 as future reference, not consumed in 1C).

**Positions taken from scoping notes §5** (carried unchanged from v0.1):

- **§5.1 — Recurrence threshold:** **Option A.** Exact-hash recurrence, `N = 3`, all-time, configurable via `.quorum/config.toml` `[memory] candidate_threshold = 3`. Embedding-based fuzzy matching (Option B) deferred to Phase 3; hybrid (Option C) rejected as arbitrary middle ground.
- **§5.2 — Promotion mechanism:** **Option C.** Both CLI (`quorum convention promote <hash>`) and TUI (`p` from a new dismissal-history view). Mid-review prompts (Option D) rejected — interactive blocking inside `quorum review` breaks the hook fail-open contract.
- **§5.3 — Expiry policy:** **Option B.** Asymmetric TTL. Candidates expire after 90 days without re-dismissal; `local_only` and `promoted_convention` never auto-expire (manual `prune` / `demote` only).
- **§5.4 — Bundle placement:** **Option A.** `local_only` entries concatenate into the existing 20 KB memory section under a `## Local conventions (auto-derived)` header. Per-entry cap 500 bytes (configurable). No new bundle budget allocation; no Lippa-side dependency.
- **§5.5 — Source-field allowlist:** **Option C.** Defer Lippa cloud sync. Phase 1C ships the state machine + `.quorum/conventions.md` write-back. `POST /api/v1/memory/propose` is documented in §7 as future reference but is NOT consumed; no Lippa-side spec dependency; no SQLite-level forward-compat columns.

Phase 1C inherits every locked decision from Phase 1A (v1.1) and Phase 1B (v1.0) without revisiting: three crates with strict library independence, cookie auth with Bearer stub, multi-model-always with synthesized severity, `Secret` newtype, per-host keyring with `--no-keyring` file fallback, HEAD-vs-index or `CommitRange` bundle assembly, `serde_json::Value` boundary between `quorum-core` and `quorum-lippa-client`, stable 0/1/2/3 exit-code taxonomy, `tokio` `current_thread` runtime, Phase 1A trust model (`.quorum/conventions.md` trusted only when committed AND byte-identical to HEAD), Phase 1B's three-input `finding_identity_hash`, Phase 1B SQLite schema v1 (including the already-shipped `promotion_state` column with default `'candidate'`).

---

## Changes from v0.1

This v0.2 applies the adjudicated peer-review feedback set as locked at the start of the v0.2 drafting session. The full reviews are in `specs/Quorum-Phase1C-Spec-v0_1-PeerReview-{A,B,C}.md`.

- **§A1 — `--force` / `Shift+P` removed entirely.** Stripped from `quorum convention promote` (§3.2 T2, §5.1) and from the TUI (§5.2). Removed from the `state_transitions.trigger` enum (§4.2). v0.1 ACs 138 and 160 removed. §10 Q11 closed; deferred to Phase 1C v2.
- **§A2 — `lippa_memory_item_id` / `lippa_proposed_at` columns dropped from §4.3.** Phase 1D will introduce them via a v2→v3 migration. v0.1 ACs 147 and 167 removed; §7.4 forward-compat seam test rewritten to a call-site assertion rather than a column-existence assertion.
- **§A3 — §6.2 promote-but-uncommitted bridge added.** When a row is `promoted_convention` but `.quorum/conventions.md` is not committed-and-clean, the bundle renders the row as a `local_only` entry in the memory section. Removes the regression where promoting hid previously-rendered conventions during the uncommitted window. AC 155 rewritten; one new AC added.
- **§B1–§B9 — convergent peer-review fixes.** T1 audit-insert race gated on `rows_affected > 0` (B1); T5 cascade documented as audit-trail-silent with the `'deleted'` virtual state and `'explicit_undismiss'` trigger removed (B2); `forward_compat_min_version = 2` shipped in §4.1 with a binary-side version check (B3); §3.3 timing rewritten to "takes effect on the next review" (B4); §3.2 T2 transactional semantics rewritten to the file-first-rename, then SQLite-commit pattern with the partial-failure window documented (B5); TUI demote key changed from `d` to `D` to avoid the main-list `d`-for-dismiss collision (B6); `--orphans` added to the §5.1 `list` row explicitly (B7); §7.2 source-mapping reframed as a CLAUDE.md constraint with Phase 1D scope (B8); §4.4 idempotent-re-promote rule dropped — re-promote requires demote first (B9).
- **§C1–§C6 — carry list (single-reviewer accepted).** Drop the redundant `UNIQUE (conventions_md_block_id)` (C1); drop `promoted_by_user` from `conventions` for privacy (C2); pin §5.3 stderr emission to a returned event (no callback) (C3); drop `body_snapshot` as auto-derived body default — title-only when `--text` / `--from-editor` not supplied (C4); tighten `convention_text` CHECK to bytes (C5); replace `tokio::spawn_blocking` with `std::thread::spawn` in the parallel-invocation test (C6).
- **§10 — Q9 closed (resolved in §4.1) and Q11 closed (per §A1).** New questions Q12–Q15 added covering atomicity recovery, re-dismiss-after-promotion, hook-mode auto-promote suppression, and promote-on-dirty-conventions.md behavior.

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
                             • bridge: rendered as local_only in memory section
                               while conventions.md is uncommitted (§6.2)
                             • Lippa cloud sync deferred to a future phase (§5.5 lean C)
```

The mission boundary: ship the pattern layer with **zero new network surface**. Lippa cloud sync was a "directionally important" capability hinted in CLAUDE.md ("cloud writes only on explicit promote OR repeated dismissals OR admin approval"), but the user-facing value of Phase 1C is "the same dismissed thing stops bothering me, and I can promote it into a real rule." That value ships under Option C without cloud. Phase 1D (or its successor name) picks up cloud sync + Lippa-side spec + `/memory/propose` consumption.

---

## 2. Non-goals (Phase 1C scope discipline)

- **No Lippa-side `/api/v1/memory/propose` consumption.** Endpoint shape is documented in §7 as future reference but no `POST` is issued. No `proposed_by_model`/`confidence` plumbing in Phase 1C. No SQLite-level forward-compat columns either — those land in Phase 1D's v2→v3 migration when the cloud-write shape is adjudicated.
- **No embedding-based fuzzy hash matching.** Phase 1B's exact-hash `finding_identity_hash` (title + source.type + sorted models) is the matching primitive throughout 1C. Semantic near-duplicates count separately. Phase 3 picks up `sqlite-vec` for fuzzy.
- **No identity-hash redesign.** The three-input hash is fixed. Phase 1B's measured residual cross-finding collision rate (0/190 on the v1.0 fixture set) is the carry-forward baseline; Phase 1C adds no new inputs.
- **No cross-repo / cross-workspace convention sharing.** `.quorum/conventions.md` is per-repo. Workspace-level conventions are Lippa-side or Phase 2+.
- **No LLM-generated rule synthesis.** Promotion writes a managed block with the auto-derived title plus user-supplied body text via `--text` / `--from-editor` / TUI modal; the dismissal `body_snapshot` is NOT used as a default body. Quorum does not call out to a model to "improve" the convention prose. Separate spec if ever pursued.
- **No convention versioning beyond the SQLite state log.** Demote-and-repromote produces a new block with a new timestamp. No semantic version field on conventions.
- **No team-level or per-org convention scoping.** Lippa-side concern (or Phase 1D's cloud sync conflict resolution).
- **No cloud sync conflict resolution.** Deferred with §5.5 lean C.
- **No TUI re-fetch.** Inherits Phase 1B's quit-and-re-invoke workflow. A new dismissal-history view ships, but it does not refresh against the live SQLite mid-session — opened state is a snapshot.
- **No `quorum convention promote --interactive` mass-flow.** CLI promotion is one-hash-at-a-time. Bulk promotion is Phase 1C v2 or deferred.
- **No `--force` / `Shift+P` bypass of the recurrence gate.** Removed in v0.2 per peer-review consensus. Phase 1C v2 may reconsider; see §11.
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
| `promoted_convention` | `dismissals.promotion_state = 'promoted_convention'` + a row in new `conventions` table | conventions section (via `.quorum/conventions.md` write-back; Phase 1A trust model applies). Falls back to memory section as a `local_only` entry while conventions.md is not committed-and-clean (§6.2 bridge). | never | never (Phase 1C; Phase 1D scope) |

The `promotion_state` column already exists in Phase 1B schema v1 with a CHECK constraint covering all three values; Phase 1C just begins writing the non-default values.

### 3.2 Transitions

**T1: candidate → local_only (AUTO).**
- Trigger: `record_seen()` bumps `recurrence_count` to a value `≥ candidate_threshold` (default 3).
- Condition: row's current `promotion_state == 'candidate'`.
- Action (single SQLite transaction): UPDATE the dismissal row with `SET promotion_state = 'local_only' WHERE finding_identity_hash = ? AND promotion_state = 'candidate'`. The application code then checks the UPDATE's `rows_affected`. If `rows_affected == 0` (another writer beat us, or the row was already past `candidate`), the operation exits without writing an audit row. If `rows_affected == 1`, INSERT a row into `state_transitions` (§4.2) and emit a returned event so the CLI can print a stderr informational note `quorum: dismissal <short-hash> auto-promoted to local convention (recurrence=N)` (§5.3).
- Idempotency: the **primary protection** is the WHERE clause — the UPDATE is a no-op on any row already in `local_only` or beyond. The audit INSERT is gated on `rows_affected > 0`, so a no-op UPDATE produces no audit row regardless of timestamp spread. The `UNIQUE(hash, from_state, to_state, ts)` on the audit table (§4.2) is a **secondary guard** for the same-millisecond-collision corner case and is NOT load-bearing for the general concurrent-`record_seen` case.

**T2: local_only → promoted_convention (EXPLICIT).**
- Trigger: `quorum convention promote <hash> [--text <s> | --from-editor]` (CLI) or `p` keypress in the TUI dismissal-history view.
- Condition: row's current `promotion_state == 'local_only'`. Promoting from `candidate` is rejected with `error: <hash> is still a candidate (recurrence=K of N); dismissed too few times to promote`. Promoting from `promoted_convention` is rejected with `error: <hash> is already a promoted_convention; run \`quorum convention demote <hash>\` first to update its text`.
- Action: SQLite alone cannot roll back filesystem writes; the cross-domain pattern is:
  1. Open a SQLite transaction (BEGIN).
  2. Write `.quorum/conventions.md` via temp-file + atomic rename (`fs::rename` is filesystem-atomic on POSIX and near-atomic on Windows via `MoveFileExW`). The new file content is the existing user content + section fence (auto-created if absent) + the new managed block.
  3. On rename success: UPDATE `dismissals.promotion_state = 'promoted_convention'`, INSERT into `conventions` (`finding_identity_hash`, `convention_text`, `promoted_at`, `conventions_md_block_id`), INSERT into `state_transitions` with `trigger = 'explicit_promote'`. Then COMMIT.
  4. On rename failure: ROLLBACK the SQLite transaction; report `error: failed to update .quorum/conventions.md: <io error>; state unchanged` (exit 2). No visible change anywhere.
- **Partial-failure window:** between step 2 (rename succeeds) and step 3 (COMMIT succeeds), a process crash leaves `.quorum/conventions.md` ahead of SQLite — the managed block exists in the file but no `conventions` row exists in the database. This is recoverable: the next CLI invocation that runs orphan detection (`quorum convention list --orphans` or any future read of the conventions section) flags the orphan and surfaces it for the user to demote-by-id or manually edit out. See §4.5 / AC for `list --orphans` and the new partial-failure AC.
- File auto-creation: if `.quorum/conventions.md` does not exist, the temp-file-then-rename writes a fresh file with the first-line marker, the section fence, and the new block — same code path as the append case, just with no pre-existing content.

**T3 (auxiliary): promoted_convention → local_only (DEMOTE).**
- Trigger: `quorum convention demote <hash>`.
- Action: same file-first-then-SQLite pattern as T2. Write the conventions.md file via temp-rename with the managed block for `<hash>` removed; on rename success, DELETE the row from `conventions`, UPDATE `dismissals.promotion_state = 'local_only'`, INSERT into `state_transitions` with `trigger = 'explicit_demote'`, and COMMIT. On rename failure: ROLLBACK, report error.
- Rationale: the demote path exists because conventions.md is a committed file and the user may regret a promotion; demote-then-commit produces a clean diff. Demote is also the path to update a convention's text (re-promote with new `--text` is not supported — see §4.4).

**T4 (auxiliary): any state → tombstone (PRUNE).**
- Trigger: `quorum convention prune [--state candidate|local_only] [--older-than N[d|w|m]]`. Default is `--state candidate --older-than 90d` (matches §5.3 lean B's auto-expire). `promoted_convention` cannot be pruned — must be demoted first.
- Action: DELETE from `dismissals` (cascade-deletes from `state_transitions`).
- Audit retention: the audit log is also deleted for the pruned row (FK cascade). Prune itself writes no audit row (the trigger enum has no `'prune'` value); this is intentional and documented in §4.6.

**T5 (boundary): undismiss.**
- Phase 1B's `MemoryStore::delete(hash)` already removes a dismissal row. In 1C, `delete` cascades through `state_transitions` and `conventions`. If the row was in `promoted_convention`, `delete` ALSO removes the managed block from `.quorum/conventions.md` (the file write must succeed for the transaction to commit; same file-first-temp-rename pattern as T2/T3).
- **Audit-trail-silent by design.** The `ON DELETE CASCADE` on `state_transitions(finding_identity_hash)` means that any audit row written for the undismissal would be removed by the cascade in the same transaction. The leak-surface cost of an audit-archive (hash + timestamp survives in a CASCADE-free side table) is not justified by the forensic value of recording rare undismiss events. Undismiss therefore produces no audit footprint. The `state_transitions.to_state` and `trigger` CHECK enums (§4.2) reflect this — neither `'deleted'` nor `'explicit_undismiss'` appears.
- The TUI's existing undo stack from Phase 1B is unchanged in behavior: an undo of a dismissal removes the candidate row before any auto-transition could fire (recurrence=1 → row deleted). Undoing a dismissal that has since auto-promoted (e.g. on a session boundary) is OUT OF SCOPE — the undo stack is per-session and per-Phase 1B v1.0 §3 already discards on TUI exit.

### 3.3 Auto-promote timing

The candidate→local_only auto-transition fires inside `record_seen()`, which runs during the post-Lippa dismissal-suppression filter pass on returned findings (Phase 1B behavior; see `SERVICES.md` §6). For each finding Lippa returns whose hash matches an active dismissal, the filter both suppresses the finding from output AND calls `record_seen()` to bump `recurrence_count`. T1 fires inside that bump when the new count hits `candidate_threshold`.

Because the filter pass runs AFTER bundle submission, the state transition happens AFTER the current review's bundle has already been sent. The newly-promoted `local_only` row therefore does NOT appear in this review's bundle memory section — it rides in the NEXT review's bundle. The user does NOT see the noisy finding in the triggering review (the dismissal filter suppressed it as it has on every review since dismissal #1); they DO see an auto-promote stderr informational note: `quorum: dismissal <short-hash> auto-promoted to local convention (recurrence=N)`. The note is suppressed under `--hook-mode=*` (parity with Phase 1B's `QUORUM_LIPPA_SESSION` precedence note).

From the NEXT review onward, the `local_only` entry rides in the memory section under `## Local conventions (auto-derived)` (§6.1), influencing Lippa's subsequent generations. The dismissal filter continues to suppress F locally — auto-promote does not remove the dismissal row — so the local convention's effect is on Lippa-side generation quality, not on the local filter pass. The auto-promote does NOT re-run `quorum review`; the bundle is built once per invocation.

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
4. `CREATE TABLE IF NOT EXISTS schema_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)` — a generic key/value side table for schema-level annotations that are not the version integer itself.
5. `INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('forward_compat_min_version', '2')` — declares the minimum binary version that may safely open this DB. A 1C binary writes `'2'`; a future v3 binary would write `'3'` on its migration. The semantic is "any binary whose `installed_max_schema_version >= forward_compat_min_version` may read and write this DB."
6. `UPDATE schema_version SET version = 2 WHERE version = 1`.

No backfill of `state_transitions` for pre-v2 dismissals: Phase 1B rows have no audit trail and stay that way. The audit log starts at the v2 migration; reading transitions for a pre-v2 row returns an empty list. This is documented and explicitly tested.

The migration runs on first `quorum review` (or `quorum convention *`) after a Phase 1B→1C binary upgrade. Failure to migrate is a hard error (exit 2 — tooling); the user is told to file a bug and not pointed at a workaround that would silently downgrade.

**Binary-side version check (open path, every invocation).** On opening the SQLite, after the migration runner has had its chance to run, the binary reads `schema_version.version` and `schema_meta.forward_compat_min_version`. Let `installed_max` be the highest schema version this binary knows how to handle (`2` for Phase 1C). If `schema_version > installed_max` OR `forward_compat_min_version > installed_max`, the binary errors out with `error: .quorum/dismissals.sqlite is from a newer Quorum (schema=<v>, forward_compat_min=<m>); upgrade your binary` and exits 2. The check fires before any `record_seen` / `convention *` work.

This forward-compat marker is the resolution of v0.1's open question Q9. A future Phase 1D v2→v3 migration writes `forward_compat_min_version = '3'`; a 1C binary opening such a DB hits the upgrade message instead of silently mis-interpreting the schema.

### 4.2 `state_transitions` table

```sql
CREATE TABLE state_transitions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  finding_identity_hash BLOB NOT NULL REFERENCES dismissals(finding_identity_hash) ON DELETE CASCADE,
  from_state TEXT NOT NULL CHECK (from_state IN ('candidate','local_only','promoted_convention')),
  to_state   TEXT NOT NULL CHECK (to_state   IN ('candidate','local_only','promoted_convention')),
  trigger    TEXT NOT NULL CHECK (trigger    IN ('auto_recurrence','explicit_promote','explicit_demote')),
  ts         INTEGER NOT NULL,           -- unix epoch millis
  by_review_session_id TEXT,             -- session id at the time of transition; nullable for non-review-driven transitions
  recurrence_at_transition INTEGER,      -- recurrence_count value at the moment of the transition; nullable
  UNIQUE (finding_identity_hash, from_state, to_state, ts)
);
CREATE INDEX idx_state_transitions_hash ON state_transitions(finding_identity_hash, ts);
```

Notes:
- `to_state` and `trigger` enums are restricted to the three concrete transition paths (T1, T2, T3). T4 (prune) and T5 (undismiss) are audit-trail-silent. T4 deletes the row outright; the FK CASCADE removes any prior audit rows for that hash. T5 is documented in §3.2 T5 as silent by design — the leak-surface cost of recording undismiss outweighs its forensic value.
- `UNIQUE (finding_identity_hash, from_state, to_state, ts)` is a secondary defense-in-depth guard for the same-millisecond collision corner case only. The primary protection against duplicate audit rows is the application code's `rows_affected > 0` gate on the preceding UPDATE in T1 (§3.2). A no-op UPDATE produces no audit row.
- `ON DELETE CASCADE` from dismissals: pruning a candidate row also drops its audit log. This is by design — the audit log carries no notes/body text but does carry the hash, and we don't want orphan audit rows for findings that no longer have any dismissal evidence.
- `by_review_session_id` is non-null for T1 (review-driven); null for T2/T3 (CLI/TUI-driven). `recurrence_at_transition` is non-null for T1; null for T2/T3.

### 4.3 `conventions` table

```sql
CREATE TABLE conventions (
  finding_identity_hash BLOB PRIMARY KEY REFERENCES dismissals(finding_identity_hash) ON DELETE CASCADE,
  convention_text TEXT NOT NULL CHECK (length(CAST(convention_text AS BLOB)) BETWEEN 1 AND 4096),
  promoted_at INTEGER NOT NULL,           -- unix epoch millis
  conventions_md_block_id TEXT NOT NULL   -- short hex hash (first 12 chars of finding_identity_hash);
                                          -- deterministic prefix of the PK, denormalized for readability
);
```

Notes:
- `convention_text` is the *body text* of the managed markdown block (not the full block including delimiters/header). Cap is 4096 **bytes** — the CHECK explicitly casts to BLOB so the length is byte-count, not codepoint-count. Generous over the 500-byte default bundle cap because conventions.md is a separate bundle section with its own 10 KB budget (`SERVICES.md` §2 `BUDGET_CONVENTIONS`).
- The PK `finding_identity_hash` is the only uniqueness constraint. `conventions_md_block_id` is a deterministic prefix of the PK and is denormalized into the row for readability and audit-log joins; it is not separately UNIQUE because the PK already constrains it.
- Promotion attribution is intentionally NOT stored as a column on this table. The committed `.quorum/conventions.md` carries the canonical "who promoted this" record via `git blame`; duplicating the git config email here would expose it through `.quorum/dismissals.sqlite` (a gitignored file that may leak via debug bundle or inadvertent copy) without adding forensic value.
- Lippa cloud-sync columns (`lippa_memory_item_id`, `lippa_proposed_at`) are NOT shipped in 1C. Phase 1D will introduce them via a v2→v3 migration (cheap nullable `ALTER TABLE ADD COLUMN`); locking the shape now is not justified because the cloud-write design (columns-on-conventions vs separate outbox table vs other) is itself a 1D adjudication.

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
- Each managed block is keyed by `id=<12-char-hex-prefix-of-finding_identity_hash>`. Demote removes the block by id.
- **Re-promote requires demote first.** T2's precondition (state == `local_only`) means a row already in `promoted_convention` cannot be re-promoted directly; the user must `quorum convention demote <hash>` first. This produces a clean two-row audit trail (`explicit_demote` then `explicit_promote`) showing the intent to update the convention text, and avoids silently overwriting any hand-edits a user may have made to the managed block in PR review.
- `v=1` on the section and each block is a format version. Format upgrades in future phases will introduce `v=2` blocks; v=1 blocks remain readable.
- Block body is plain markdown; no Quorum-specific tags inside. The `### Convention: <title>` header is auto-derived from `Finding.title` at promote time. The body comes from user-supplied input only — `--text` (CLI), `--from-editor` (CLI), or the TUI modal. There is no auto-derived body default; if the user provides no `--text` and accepts the title at the TUI modal without typing, the block carries only the title header (no body paragraph). The dismissal `body_snapshot` field is NEVER used as a default body, because `body_snapshot` is repo-content-derived (laundered through Lippa's analysis) and may carry context the user did not consciously authorize for inclusion in a committed conventions file.
- Line endings are normalized to the file's existing convention (LF preserved on Unix-authored files, CRLF preserved on Windows-authored files). On a new file, defaults to LF.
- Quorum writes the file with a `quorum-managed-conventions-md` marker as the FIRST line of a freshly-created file: `<!-- quorum-managed-conventions-md v=1 -->`. This is solely for the conventions-md-modified-outside-quorum detection (§4.5); the trust model for the bundle layer is independent and rides on Phase 1A's committed-and-clean check.

### 4.5 Merge-conflict discipline

Conventions.md is committed by the user and may produce merge conflicts when two developers promote different findings on the same day. The section-fenced, block-keyed format minimizes — but does not eliminate — friction:
- Different `id=` blocks separated by other content typically merge cleanly under git's line-based merge.
- Two developers each appending a new block at the end of the managed section (parallel append to the same line range adjacent to the closing `<!-- /quorum:managed-section -->`) will produce an ordinary git conflict that the developer resolves manually as plain markdown. The unique `id=` does not save this case from git's line-based view.
- The same `id=` block can also conflict if two developers promote the same hash on parallel branches. Same manual resolution.
- `quorum convention promote` does NOT auto-commit conventions.md. The user runs `git add .quorum/conventions.md && git commit` after promotion. The Phase 1A trust model (committed-and-clean-or-ignored) means an uncommitted convention does not yet ride in the bundle as a *convention* — see §6.2 for the bridge that keeps it visible as a `local_only` entry in the memory section during the uncommitted window.

### 4.6 Audit-trail leak surface

Per Phase 1B (`SERVICES.md` §6 last bullet), the dismissals store keeps free-text notes off-disk-outside-the-gitignored-DB. Phase 1C extends:
- `state_transitions` carries the hash + timestamps + trigger + recurrence — no titles, no notes, no bodies.
- `conventions.convention_text` IS the convention body and is intentionally user-readable (it lands in conventions.md, which IS committed). But: `convention_text` is *user-supplied or title-derived*, NOT the dismissal `note` field. Phase 1B's `note` (free-text rationale, ≤2 KB, possibly sensitive) is NEVER copied into `conventions.convention_text` automatically. The dismissal `body_snapshot` (LLM-generated finding body, repo-content-derived) is also NEVER auto-copied. The promote command writes only what the user explicitly types via `--text`, `--from-editor`, or the TUI modal — or the title-only header if no body input is given.
- T4 prune deletes the row and (via FK cascade) any prior audit rows for that hash. Prune itself writes no audit row, so a pruned row is a forensic blackout: there is no record that it existed. This is accepted as the cost of keeping prune simple and consistent with §6's leak-surface discipline.
- T5 undismiss is audit-trail-silent for the same reason (§3.2 T5).

---

## 5. User-facing surface

### 5.1 CLI subcommands

Phase 1C adds one new top-level subcommand group: `quorum convention`. Subcommands:

| Command | Effect | Exit codes (Phase 1A taxonomy) |
|---|---|---|
| `quorum convention list [--state {candidate,local_only,promoted_convention}] [--orphans] [--json]` | Prints rows from `dismissals` joined with current state. With `--json`, prints a JSON array (one element per row) to stdout suitable for piping. With `--orphans`, also reports managed blocks in conventions.md with no SQLite row AND SQLite rows with no matching block in conventions.md. | 0 ok / 2 tooling |
| `quorum convention show <hash>` | Prints title + body_snapshot + state + transition history for one row (resolved by short-hash prefix; ambiguous prefix → exit 2). | 0 ok / 2 tooling |
| `quorum convention promote <hash> [--text <s>] [--from-editor]` | Executes T2. `--from-editor` opens `$EDITOR` with the title-derived default; rejects `--text` and `--from-editor` together. Without either flag, the managed block carries only the title header (no body). Rejects rows that are not in `local_only` with a clear error (exit 2). | 0 ok / 2 tooling (state error, missing hash, IO) |
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
- Status bar shows: `H back | p promote | D demote | x delete | / filter | ?  help`.
- Exits to main view via `H` (toggle), `Esc`, or `q` (which now quits the TUI from any view).

**New keybinding `p` (history view): promote highlighted entry.**
- On a row whose state is `local_only`: opens a single-line modal: `Convention text (Enter to accept title-only, Esc to cancel):` with a default value derived from `Finding.title`. The user types body text or accepts the title-only default.
- On Enter: T2 transition fires. On success: status-bar transient confirmation `promoted <short-hash> to convention; commit .quorum/conventions.md to apply`.
- On a row whose state is NOT `local_only` (i.e. `candidate` or already `promoted_convention`): the keypress is rejected at the keystroke level, before the modal opens. Status bar shows the reason (`promote requires local_only state (this row is candidate; dismiss it more or wait for auto-promote)` or `already a promoted_convention; demote first to update`).

**New keybinding `D` (history view, capital): demote highlighted entry.**
- Confirmation modal: `Demote <short-hash>? Y/N` (capital Y required).
- On `Y`: T3 transition fires.
- The keybinding is capital-D to avoid colliding with Phase 1B's main-list-view `d`-for-dismiss muscle memory. Status-bar text reflects the capital.

**Main list view (`q` / `H` only).**
The main per-review list view from Phase 1B is unchanged except for the addition of `H` to open the history view and the addition of the auto-promote stderr informational note (rendered to the body of the new local-only entry in the next review's bundle, not the current TUI session).

### 5.3 `quorum review` integration

- The bundle assembly step (`quorum-core::bundle::assemble`) consults `MemoryStore::load_local_only_conventions()` and concatenates the resulting entries (each truncated to `local_convention_bundle_cap` bytes) into the memory section under a `## Local conventions (auto-derived)` header. The 20 KB `BUDGET_MEMORY` is shared with CLAUDE.md / AGENTS.md / .cursorrules; overflow produces the standard `[memory truncated …]` marker from `SERVICES.md` §2.
- The per-entry truncation marker, when applied, is `[local convention truncated: <hash-short>, <bytes-elided> bytes elided; raise [memory] local_convention_bundle_cap to see full text]` and counts toward the 20 KB total.
- The auto-promote stderr informational note (§3.2 T1) is emitted via a **returned event**: `quorum-core::memory::record_seen` returns a `Vec<TransitionEvent>` (or equivalent value-type) alongside its existing return shape; the CLI binary consumes this value after the call and prints to stderr. A callback pattern (where `quorum-core` calls into something `quorum-cli` registered) would violate the crate-boundary rule that libraries do not depend on the binary — the returned-event pattern keeps the boundary intact. The stderr note is suppressed under `--hook-mode=*` (parity with Phase 1B's `QUORUM_LIPPA_SESSION` precedence note). Whether the underlying T1 transition itself is suppressed under hook mode is open — see §10 Q14.

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
3. Each `local_only` entry (including bridge-rendered `promoted_convention` rows per §6.2) rendered as:

   ```markdown
   ### Local convention: <Finding.title (truncated to 80 chars)>
   <body_snapshot truncated to local_convention_bundle_cap bytes, UTF-8 codepoint boundary>
   <!-- recurrence=N, since=<ISO yyyy-MM-dd>, hash=<short> -->
   ```

   Entries are sorted by `recurrence_count DESC, last_seen_at DESC` so the most-recurring rules sit at the top.

4. If summing the entries exceeds `BUDGET_MEMORY` minus the budget already consumed by step 1, truncation happens at the entry boundary (NOT mid-entry), and the standard `[memory truncated: N bytes exceeded 20KB; review only top portion]` marker (`SERVICES.md` §2) is appended.

5. Per-entry truncation marker (when an individual `body_snapshot` is too long to render in full at `local_convention_bundle_cap`) is the `[local convention truncated: …]` form from §5.3.

### 6.2 Promote-but-uncommitted bridge

`promoted_convention` entries live in `.quorum/conventions.md` and normally ride the existing 10 KB `BUDGET_CONVENTIONS` (`SERVICES.md` §2). Phase 1A's trust model excludes a `.quorum/conventions.md` that is not committed-and-clean (file is tracked AND HEAD blob matches working tree byte-for-byte) from the conventions section of the bundle. In v0.1 this meant that the window between `quorum convention promote` and `git commit` left the row invisible to the bundle entirely — and worse, because the trust model is file-wide, the *whole* conventions section disappeared until commit, dropping every previously-committed convention too.

**v0.2 collapses this regression via a memory-section fallback (bridge).** At bundle assembly time, for each `promoted_convention` row, the bundle path consults the Phase 1A trust check on `.quorum/conventions.md`:

- If conventions.md is committed-and-clean, the convention rides in the conventions section as a managed block (normal path).
- If conventions.md is NOT committed-and-clean (uncommitted edits, dirty working tree, or file missing from HEAD), the convention is rendered as a `local_only` entry in the memory section — same shape it would have had pre-promotion (recurrence, since, hash all preserved). The conventions section reflects only whatever HEAD's `.quorum/conventions.md` contains under Phase 1A's existing rules; the bridge does NOT make the conventions section visible while dirty.

The bridge applies per-row at render time, not as a state-machine change: the SQLite state remains `promoted_convention` regardless of file state. As soon as the user runs `git add .quorum/conventions.md && git commit`, the next bundle assembly sees the file as committed-and-clean and switches the row's render path from memory-section to conventions-section. No SQLite write fires on the toggle — only the bundle assembly's per-row check changes its answer.

Toggling commit-state back to dirty (e.g. the user edits conventions.md after committing) flips the row's render path back to memory-section bridge.

The user is reminded at promote time: `promoted <hash>; commit .quorum/conventions.md to move it from memory to the conventions section`. The phrasing communicates that the rule is not invisible — it has bridge-fallback to the memory section — but is parked there until commit.

### 6.3 Per-entry cap rationale (§5.4 lean A side-effect)

The 500-byte default per-entry cap exists so a single verbose dismissal doesn't crowd out CLAUDE.md inside the 20 KB memory section. The cap is configurable (100..2048 range) for users who deliberately want longer rules to ride in the bundle; the upper bound of 2048 matches the `body_snapshot` storage cap from Phase 1B (so no entry truncation can lose information that the dismissal didn't already discard).

### 6.4 Overflow markers (Phase 1A discipline preserved)

Every truncation point produces a visible marker; no silent truncation. The Phase 1A "no silent truncation" rule from `SERVICES.md` §2 holds for the new entry shape too.

---

## 7. Lippa integration (deferred to Phase 1D — §5.5 lean C)

Phase 1C makes **NO** outbound Lippa network calls for memory/convention writes. The Lippa `/api/v1/auth/login`, `/api/v1/consensus/sessions`, `/api/v1/me` surfaces from Phase 1A/1B remain unchanged. `POST /api/v1/memory/propose` is documented here for orientation only; Phase 1D will adjudicate the SQLite-side shape (column-on-conventions vs separate outbox table vs other) and the source-field mapping at that time.

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

### 7.2 Source-field constraint (CLAUDE.md)

CLAUDE.md mandates: "When writing memory items to Lippa, the `source` field MUST be `model_proposed` with `proposed_by_model` and `confidence` set. Never invent new source values; the Lippa allowlist is enforced server-side and unknown values are rejected."

For Phase 1C: `source: model_proposed` per CLAUDE.md hard constraint. **Full payload mapping (proposed_by_model, conversation_id, turn_id, confidence semantics) is Phase 1D scope — not adjudicated in 1C.** In particular, the semantics of `confidence` for user-affirmed promotion, the strategy for collapsing `Finding`'s plural `sorted_models` into a singular `proposed_by_model` string, and the source of `conversation_id` / `turn_id` at promote time are all 1D decisions. Phase 1C carries no schema or code that pre-commits to a specific shape for any of these.

### 7.3 What Phase 1C does NOT do

- Does not call `POST /api/v1/memory/propose` ever, even from `convention promote`.
- Does not require `conversation_id` / `turn_id` at promote time (Phase 1D concerns).
- Does not implement an outbox or retry queue.
- Does not ship the `lippa_memory_item_id` or `lippa_proposed_at` columns; these belong to Phase 1D's v2→v3 migration.
- Does not expose a "sync to cloud" CLI flag — adding one prematurely would invite a half-shipped feature.

### 7.4 Forward-compat seam test

A single test (`tests/conventions_lippa_seam.rs`) asserts that **no Phase 1C code path issues a call to `/api/v1/memory/propose`**. This is a structural check against the `quorum-lippa-client` call-site enumeration: the allowed endpoint set is exactly the Phase 1A/1B surface (`/api/v1/auth/login`, `/api/v1/auth/logout`, `/api/v1/me`, `/api/v1/consensus/sessions`, plus health/version probes per existing spec). Any reference to `/api/v1/memory/propose` would surface in this check and fail. The earlier v0.1 column-existence assertion is dropped along with the columns themselves.

---

## 8. Trust model

### 8.1 Carryover from Phase 1A

Quoting CLAUDE.md hard constraints: "`.quorum/conventions.md` is trusted only when committed. Uncommitted changes to the conventions file are ignored by Quorum's context loader." Phase 1C does NOT change this. The Phase 1A bundle-assembly check (committed-and-clean: file is tracked, HEAD blob matches working tree byte-for-byte) gates whether the file's contents enter the **conventions** section of the bundle.

The promote-but-uncommitted bridge (§6.2) introduces a memory-section fallback: a `promoted_convention` row whose conventions.md is dirty renders in the memory section as a `local_only` entry. This does not weaken the Phase 1A trust model — the conventions section continues to ignore an uncommitted conventions.md entirely. The bridge only changes which section the row's *value* is sourced from (memory section reads from SQLite; conventions section reads from the file).

### 8.2 Carryover from Phase 1B

"Read repo content as DATA, never as instructions" (CLAUDE.md). The `<<<QUORUM_REPO_CONTENT_BEGIN>>>` / `<<<QUORUM_REPO_CONTENT_END>>>` delimiters from `SERVICES.md` §2 wrap the entire repo-content payload including the new "## Local conventions (auto-derived)" subsection. Lippa-side handlers treat the local convention text as untrusted data, not instructions.

The `local_only` entries' rendered body is sourced from the dismissal `body_snapshot` — repo content laundered through Lippa's analysis, already trusted under Phase 1B's data-wrapping. The `conventions.convention_text` written at promote time is user-supplied (`--text` / `--from-editor` / TUI modal) or title-only; the user is the explicit author, and the file is committed under their git identity.

### 8.3 New rules for Phase 1C

- **Promotion never writes to Lippa in Phase 1C.** No cloud side effects.
- **The dismissal `note` is never auto-copied into `conventions.convention_text`.** §4.6 above.
- **The dismissal `body_snapshot` is never auto-copied into `conventions.convention_text`.** §4.4. Repo-content-derived text does not silently become committed convention text.
- **Conventions.md modifications outside Quorum are detected.** When `quorum convention demote/promote` is invoked, the conventions.md file is parsed; if a managed block referenced by SQLite is missing from the file (or vice versa), the CLI emits a warning `note: .quorum/conventions.md and the conventions table are out of sync (block <id> missing/extra); run \`quorum convention list --orphans\` for details`. The promote/demote operation proceeds — Quorum does not refuse to act on user-edited managed-section content; demote on a missing block is a no-op (with warning).
- **The `quorum:managed-section` fence is the boundary.** Quorum NEVER edits content outside the fence (above the opening marker or below the closing marker). Content INSIDE the fence is owned by Quorum and may be rewritten on every promote/demote. Users who want to keep their own conventions in conventions.md write them ABOVE the fence.

### 8.4 Cross-cutting: dismissal-state-machine writes are local-only

Cloud writes (when they eventually exist) go through the §5.5 → Phase 1D pipeline. Phase 1C's state machine is entirely local. There is no admin-approval path in 1C either (CLAUDE.md cloud-write trigger #3); admin approval is also Phase 1D.

---

## 9. Acceptance criteria (numbered, testable)

Phase 1B closed at AC 131; AC 132 (sigstore attestation) flipped FULL in 0.2.1. Phase 1C ACs begin at 133. v0.2 keeps v0.1's stable numbers where possible; removed ACs (138, 147, 160, 167) leave gaps rather than reshuffle; new ACs land at 173+.

**State machine (T1 / T2 / T3 / T4 / T5):**

- AC 133 — Phase 1B dismissals behavior preserved. The Phase 1B integration suites pass unchanged after the v1→v2 migration. (Regression.)
- AC 134 — Every new dismissal creates a `dismissals` row with `promotion_state = 'candidate'` and no `state_transitions` entry (the candidate state is implicit / not logged as a transition).
- AC 135 — T1 fires inside `record_seen()`: when `recurrence_count` reaches `candidate_threshold` (default 3) on a `candidate` row, the same transaction sets `promotion_state = 'local_only'` and inserts exactly one `state_transitions` row with `from_state='candidate'`, `to_state='local_only'`, `trigger='auto_recurrence'`.
- AC 136 — Concurrent `record_seen()` invocations on the same hash past the threshold produce exactly one state transition and exactly one audit row, regardless of timestamp spread. (The application-side `rows_affected > 0` gate is the load-bearing check; the SQL UNIQUE is a same-millisecond defense-in-depth guard only.)
- AC 137 — `quorum convention promote <hash>` on a `local_only` row sets `promotion_state = 'promoted_convention'`, inserts a `conventions` row, inserts a `state_transitions` row with `trigger='explicit_promote'`, and writes a managed block into `.quorum/conventions.md` (via temp-file + atomic rename) before the SQLite COMMIT. Promoting from `candidate` is rejected (exit 2); promoting from `promoted_convention` is rejected (exit 2) with the "demote first" message.
- AC 139 — `quorum convention promote <hash> --text "<s>"` writes `<s>` to `conventions.convention_text` (subject to 1..4096 byte CHECK). With no `--text` and no `--from-editor`, the block carries only the title-derived header line and no body paragraph. The dismissal `note` field is NEVER read by promote; the dismissal `body_snapshot` is NEVER used as a body default.
- AC 140 — `quorum convention demote <hash>` on a `promoted_convention` row sets `promotion_state = 'local_only'`, removes the row from `conventions`, removes the managed block from `.quorum/conventions.md` (via temp-file + atomic rename), and inserts a `state_transitions` row with `trigger='explicit_demote'`.
- AC 141 — `quorum convention prune` defaults to `--state candidate --older-than 90d`. With `--dry-run`, it prints the candidates that would be deleted and writes nothing. Without `--dry-run`, it DELETEs the matched rows (and their cascade-deleted audit rows).
- AC 142 — `quorum convention prune` refuses to operate on `promoted_convention` rows; the user must demote first.
- AC 143 — `MemoryStore::delete(hash)` (undismiss; existing Phase 1B surface) on a `promoted_convention` row cascades: removes the managed block from conventions.md (via temp-file + atomic rename) before SQLite COMMIT. No audit row is written (audit is silent by design — §3.2 T5).
- AC 144 — Two parallel `record_seen` invocations on the same hash, executed via `std::thread::spawn` (sync, no tokio), produce exactly one state transition and exactly one audit row even when the threads' write timestamps differ by more than one millisecond.

**Data model:**

- AC 145 — The v1→v2 migration runs at first `quorum review` (or `quorum convention *`) post-binary-upgrade. The migration is idempotent (re-running it after a partial failure is safe) and updates `schema_version` to 2 and `schema_meta.forward_compat_min_version` to `'2'`.
- AC 146 — `state_transitions` carries `ON DELETE CASCADE` from `dismissals(finding_identity_hash)`. Pruning a candidate row also drops its audit log.
- AC 148 — The `conventions.md` write-back uses the section-fenced, block-keyed format from §4.4. Demote-then-repromote on the same hash produces a single managed block at the new text (not two; not zero).
- AC 149 — Content above `<!-- quorum:managed-section v=1 -->` and below `<!-- /quorum:managed-section -->` in conventions.md is preserved byte-for-byte across promote/demote operations.
- AC 150 — Line endings in conventions.md are preserved: a file authored with CRLF stays CRLF after a promote; a file authored with LF stays LF. A new file (Quorum-created) defaults to LF.
- AC 151 — A `convention_text` of 4097 bytes is rejected at the SQLite CHECK; a 1-byte text is accepted. The CHECK enforces byte-count (cast to BLOB), not char-count.

**Bundle assembly:**

- AC 152 — `local_only` entries appear in the bundle memory section under the `## Local conventions (auto-derived)` header, sorted by `recurrence_count DESC, last_seen_at DESC`. CLAUDE.md / AGENTS.md / .cursorrules content precedes them.
- AC 153 — Per-entry rendering uses the format from §6.1 step 3 (title-truncated, body-truncated at `local_convention_bundle_cap`, trailing recurrence/since/hash HTML comment).
- AC 154 — Per-entry truncation emits the `[local convention truncated: <hash-short>, <bytes-elided> bytes elided; …]` marker; total-section truncation emits the existing `[memory truncated: …]` marker.
- AC 155 — A `promoted_convention` row whose conventions.md is uncommitted-or-dirty is rendered as a `local_only` entry in the memory section (the same shape it would have had pre-promotion). Once conventions.md is committed-and-clean, the row renders via the conventions section, and the memory-section local_only render is suppressed.
- AC 156 — Total bundle byte budgets (Phase 1A's `BUDGET_TOTAL = 200 KB`, per-section caps) are not raised by Phase 1C. The 20 KB memory section is the shared cap.

**TUI:**

- AC 157 — Pressing `H` in the main TUI view opens the dismissal-history view (list of dismissals with state column). Pressing `H` again or `Esc` returns to the main view.
- AC 158 — In the history view, `p` on a `local_only` row prompts for convention text (default = title-derived; Enter-with-empty produces title-only), and on Enter executes T2: SQLite + conventions.md write + status-bar transient confirmation. `p` on a non-`local_only` row is rejected at the keystroke (no modal opens).
- AC 159 — In the history view, `D` (capital) on a `promoted_convention` row prompts `Y/N` confirmation and on `Y` executes T3. The keybinding is capital-D to avoid colliding with Phase 1B's main-list-view `d`-for-dismiss.
- AC 161 — TUI exit (any path) leaves the terminal restored (Phase 1B RAII guard + panic-hook chain unchanged). A panic during T2's file write also restores the terminal before the panic propagates.

**CLI / config:**

- AC 162 — `quorum convention list` prints rows by state with short-hash + title; `--state local_only` filters; `--json` produces a stable-ordered JSON array suitable for piping into `jq`.
- AC 163 — Short-hash resolution accepts any hex prefix ≥ 8 chars; ambiguous prefix returns exit 2 with a disambiguation message; full 64-hex always accepted.
- AC 164 — `.quorum/config.toml` `[memory] candidate_threshold = 0` is rejected at validation (range 2..100); same for the other config keys' ranges. Validation errors exit 2 from `quorum review` or `quorum convention *` on the first read after config change.
- AC 165 — Default config (no `config.toml` `[memory]` section) yields threshold=3, cap=500, expire=90d.

**Lippa forward-compat seam (§7):**

- AC 166 — No Phase 1C code path issues `POST /api/v1/memory/propose`. (Asserted by the `quorum-lippa-client` call-site enumeration check — the allowed endpoint set excludes `/api/v1/memory/propose`.)

**Trust model:**

- AC 168 — Conventions.md modifications outside Quorum (e.g., manual edit to a managed block, removing a `<!-- /quorum:convention -->` closer) are detected when the next `quorum convention promote/demote` or `quorum convention list --orphans` runs and surface as a stderr warning. The operation proceeds (Quorum does not refuse to act on user-edited managed-section content).
- AC 169 — A managed block that exists in conventions.md but has no SQLite `conventions` row is reported by `quorum convention list --orphans` (a diagnostic sub-flag) as orphaned; similarly, a SQLite `conventions` row whose managed block is missing from conventions.md is reported. Default `list` does NOT show orphans.
- AC 170 — The `<<<QUORUM_REPO_CONTENT_BEGIN>>>` delimiter wraps the entire memory section including the new local-conventions subsection. (Phase 1A behavior preserved.)

**Cross-cutting (Phase 1B regression):**

- AC 171 — No Phase 1B test fails after the migration. (Regression-shape, not a fixed count: Phase 1B closed at 197 passing tests, but the count grows with 1C additions; the assertion is "every test green at Phase 1B remains green at Phase 1C HEAD.")
- AC 172 — `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check --all` pass.

**New in Phase 1C (v1.0):**

- AC 173 — Toggling commit-state of conventions.md between two `quorum review` invocations correctly switches the row's render path between memory and conventions sections. Specifically: promote without commit → row renders in memory section (bridge); `git add && git commit` → next review renders the row in conventions section, memory-section render is suppressed; subsequent edit-without-recommit → next review reverts to memory-section bridge.
- AC 174 — A SQLite whose `schema_version > 2` OR whose `schema_meta.forward_compat_min_version > 2` is rejected by a Phase 1C binary with `error: .quorum/dismissals.sqlite is from a newer Quorum (...); upgrade your binary` and exit 2. The check runs before any `record_seen` or `convention *` work.
- AC 175 — A crash between conventions.md rename and SQLite commit (simulated via a kill-after-rename harness) leaves the managed block in conventions.md with no matching `conventions` row in SQLite. The orphan is detectable by `quorum convention list --orphans` on the next run.

---

## 10. Open questions

Items v1.0 leaves unresolved or partially deferred; Phase 1C implementation may close some of these. Q9 closed in v1.0 (see §4.1); Q11 closed in v1.0 (see §A1 / §11); both kept as closed notes below for traceability.

**Standing note on `lippa_*` columns:** Phase 1D will introduce these columns via a v2→v3 migration. Rationale for not pre-staging in 1C: the deferred-migration cost equals the ship-now cost (cheap nullable `ALTER TABLE ADD COLUMN`), and there is no value in locking the shape (column-on-conventions vs separate outbox table vs other) before 1D adjudicates it.

- **Q1 — Promote-conflict-with-merge handling.** §4.5 says same-`id=` block conflicts during git merge are user-resolved manually. Should `quorum convention promote` detect that the local block diverges from what HEAD shows for the same hash (e.g., a teammate already promoted with different text) and warn at promote time? Cheap to detect; UX cost is low. Lean: yes, warn but don't refuse.
- **Q2 — Audit log retention on undismiss / prune.** §4.2 ON DELETE CASCADE drops the audit trail. Should pruned-candidate audit rows be retained for "why does this finding keep recurring?" forensics? Argues for a separate `audit_archive` JSONL — but that re-introduces a leak surface (hash + ts is borderline; titles would be a clear violation). Lean: keep CASCADE for 1C; reconsider if user feedback asks for it.
- **Q3 — Promote inside a hook.** §5.1 table says all `convention` subcommands are 0/2-exit, no network. Should `quorum install --hook=*` ever wire up a promote path (e.g., a post-commit hook that promotes auto-flagged conventions)? Lean: no; promotion is intentional user action.
- **Q4 — Threshold per-source-type.** Should `candidate_threshold` be tunable per `Finding.source.type` (e.g., 2 for `false_positive`, 5 for `intentional`)? Tempting for low-cost noise control. Lean: no in v1.0 (single int simpler to explain); reconsider for Phase 1C v2.
- **Q5 — `quorum convention sync-check` shape.** §8.3 references this subcommand for "out of sync" detection. The exact shape (list orphans? prompt to reconcile?) is not specified. Lean: v1.0 ships `--orphans` flag on `list` only; `sync-check` is a Phase 1C v2 expansion.
- **Q6 — Auto-promote stderr verbosity.** §3.2 T1 prints `quorum: dismissal <short-hash> auto-promoted to local convention (recurrence=N)`. Is one line per transition right, or should multiple transitions per review batch into a summary line? Lean: one line per transition for transparency; reconsider if there's noise feedback.
- **Q7 — Demote without git working tree access.** §3.2 T3 must remove a block from conventions.md. If conventions.md doesn't exist (was deleted, was never created in this repo), what's the behavior? Lean: no-op for the file write, with stderr warning; the SQLite state changes proceed.
- **Q8 — Local convention rendering vs Lippa wire format.** §6.1's per-entry rendering format is markdown-shaped. If Lippa-side review prompts expect a different shape for local conventions specifically (vs general memory), this could need tweaking. Lean: ship markdown for v0.1; if Lippa Phase 1D recon shows otherwise, adjust.
- **Q9 — Migration rollback path.** *Resolved in v1.0; see §4.1.* The `schema_meta.forward_compat_min_version = '2'` row + the binary-side check on every open rejects a v3+ DB opened by a v1.0 binary with a clear upgrade message.
- **Q10 — Bundle byte budget revisit.** §5.4 lean A keeps the 20 KB cap. If users actually accumulate many local rules, the noise-suppression intent backfires (CLAUDE.md gets crowded out). Should the cap be raised, or `local_only` get its own sub-budget? Lean: cap stays for 1C; instrument bundle assembly to log truncation rates so Phase 2's budget revisit is data-informed.
- **Q11 — `--force` / Shift+P bypass of the recurrence gate.** *Removed in v1.0 per peer-review consensus (3-of-3 against shipping). Future phase may reconsider with a fresh adjudication if user demand emerges. See §11 Phase 1C v2 list.*
- **Q12 — Atomicity / partial-failure recovery (SQLite-ahead-of-file case).** §3.2 T2 / T3 / T5 are documented for the happy path and the rename-fails-before-commit path. §B5 / AC 175 covers the conventions.md-ahead-of-SQLite crash window (rename succeeded, COMMIT didn't). What's the symmetric recovery for SQLite-ahead-of-file (COMMIT succeeded somehow despite the file write failing — should be impossible under the file-first ordering, but if rename appeared to succeed and the OS later lost the file via crash or fs corruption)? Probably also recovered by `list --orphans` (SQLite has a `conventions` row whose block is missing from the file) but the spec doesn't enumerate the recovery flow. Lean: rely on `list --orphans` symmetrically; demote the orphan SQLite row to re-promote.
- **Q13 — Re-dismiss after promotion.** If a finding is in `promoted_convention` and the user re-encounters it and presses `d` (dismiss) in the TUI, what's the right behavior? Options: (a) silent no-op (it's already a convention); (b) error ("already promoted"); (c) demote-then-dismiss; (d) increment `recurrence_count` only, leaving state untouched. Lean: (d) — counter as observability for "the rule is still catching things"; status bar shows `<short-hash> already promoted to convention; recurrence bumped`. No state change.
- **Q14 — Hook-mode auto-promote suppression.** §5.3 says the auto-promote stderr note is suppressed under `--hook-mode=*`. Is the auto-promote transition itself suppressed under hook mode too, or just the note? A noisy stderr hook makes for poor CI logs; on the other hand, hook fail-open is about network errors, not local SQLite writes. Lean: fire the transition but suppress the note (current spec); reconsider if hook noise is a problem.
- **Q15 — Promote on dirty conventions.md.** If the user runs `quorum convention promote <hash>` while conventions.md has uncommitted modifications outside the managed section, the temp-file-rename pattern preserves the user's edits (the temp file is built from the working-tree contents plus the new block, then renamed). But should the CLI warn? Or refuse if the modifications are *inside* the managed section (where Quorum is about to overwrite)? Lean: warn-but-proceed on outside-fence modifications; warn-and-confirm (`Y/N` prompt) on inside-fence modifications detected by the orphan-check pass.

---

## 11. Out of scope

(Restating §2 plus what's deferred to numbered future milestones.)

- **Phase 1D (or its successor name):** Lippa cloud sync. `POST /api/v1/memory/propose` consumption. Source-field allowlist mapping (the `proposed_by_model` / `confidence` / `conversation_id` / `turn_id` plumbing — see §7.2). SQLite v2→v3 migration adding `lippa_memory_item_id` and `lippa_proposed_at` (or the chosen outbox shape). Outbox / retry queue. Admin-approval path for cloud writes. Lippa-side spec dependency.
- **Phase 2:** Privacy-mode redaction (regex/AST first, local LLM second). Per-team / per-org convention scoping (Lippa-side). PR commenting, IDE plugins, BYO-keys mode, web dashboard. Memory-write outbox built ON TOP of `MemoryStore` (1C's local SQLite stays the sync backbone).
- **Phase 3:** Embedding-based fuzzy hash matching (`sqlite-vec`). Semantic-near-duplicate dismissals counting toward threshold. Phase 1C ships exact-hash recurrence only.
- **Phase 1C v2 (post-1C-ship, pre-1D):**
  - `--force` / Shift+P bypass of the recurrence gate — peer review adjudicated against shipping in 1C; reconsider if demand emerges.
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

*— end Phase 1C v1.0 spec —*
