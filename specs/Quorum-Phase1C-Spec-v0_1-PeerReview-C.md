# Quorum Phase 1C v0.1 — Peer review

**Reviewer ID:** C
**Subject:** `specs/Quorum-Phase1C-Spec-v0_1.md`
**Session:** independent peer review, fresh chat, no shared context with reviewers A or B.
**Scope:** spec review only (no code, no Phase 1A/1B redesign).

---

## Summary

v0.1 is a competent, mostly-internally-consistent draft that implements the §5 scoping leans cleanly. The state machine spine is sound, the trust-model carryover is faithful, and the migration is correctly framed as v1 → v2 (because the `promotion_state` column already shipped in Phase 1B schema v1).

But the spec carries **three substantive issues that should not survive into v0.2**:

1. **`--force` and `Shift+P` are silently adjudicated**, then hedged with a Q11 fig leaf. They invent a candidate→promoted_convention transition the scoping notes did not authorize, ship ACs and TUI bindings for it, then ask peer review to decide whether to keep them. Pick one or the other; do not ship code-bearing ACs for a feature labelled TBD.
2. **The T1 audit-insert is racy as specified.** The UNIQUE constraint on `(hash, from_state, to_state, ts)` does not catch the race the spec claims it does, and AC 136 will fail under realistic concurrency unless the audit INSERT is explicitly gated on the UPDATE's `rows_affected`.
3. **The promote-but-not-committed window is an unresolved UX hole**, not a small trade-off. The spec acknowledges it (§6.2, AC 155) but does not warn the user clearly enough at promote time, and the design produces a regression — the rule was bundle-visible as `local_only`, then becomes invisible after promotion until `git commit` lands. This needs a stronger user-facing affordance.

Plus a long tail of smaller issues, two silent adjudications (T3 demote, T4 prune, and T5 cascade-to-file did not appear in the scoping notes' transition diagram), several missing ACs, and one byte-vs-char unit mismatch in the `convention_text` CHECK.

Net recommendation: **v0.2 is required.** Most fixes are local; the `--force` decision is the only one that touches user-visible surface area.

---

## 1. Internal consistency

### 1.1 §6.2 contradicts itself about the promote-but-uncommitted window

The first paragraph in §6.2 says "between promotion and commit, the rule is still visible as a `local_only` entry would have been — except it isn't, because the auto-derived block has already been added to conventions.md, which when committed-and-clean is the source of truth." That sentence is doing two contradictory things at once.

The next paragraph (§6.2 second para) clarifies: in the promoted-but-uncommitted window, the rule is **invisible** to the bundle. The state machine moved past `local_only` (so `MemoryStore::load_local_only_conventions` skips it), and conventions.md is uncommitted (so the Phase 1A trust check skips it).

This is a real UX regression: from the user's perspective, the bundle's memory section loses the rule they just promoted, until they `git commit`. The spec accepts this as "a small UX cost." I do not think it is small. The promote action *worsens* the bundle for the duration of the uncommitted window. v0.2 should either:
- **Option A.** Keep the row's `local_only` rendering active until conventions.md is committed-and-clean (i.e., the state machine reads `promoted_convention` but the bundle path falls back to local_only rendering when conventions.md isn't yet committed). This collapses the regression.
- **Option B.** Make `quorum convention promote` print a louder warning: not just the existing `promoted <hash>; commit .quorum/conventions.md to apply` but explicitly `warning: this convention is invisible to bundles until you commit conventions.md` — possibly to stderr in red.
- **Option C.** Accept the gap, document it prominently, and add an AC that the warning is shown.

**My lean: Option A.** It's a small `quorum-core::bundle` change and it removes a footgun that will produce confused user reports. The trust-model semantics are preserved either way (an uncommitted conventions.md still does not enter the *conventions* section; this is purely about whether the rule continues to flow through the *memory* section until conventions.md is committed). Worth a new §10 question (see Q14 below).

### 1.2 T1's audit-insert race is mis-specified

§3.2 T1 says concurrent `record_seen` invocations are guarded by `UNIQUE(hash, from_state, to_state, ts)` on `state_transitions` (§4.2). The note in §4.2 repeats this: "UNIQUE(...) is the idempotency guard for T1's transactional UPDATE."

This is only true if the two invocations land at the **same `ts` millisecond**. If two invocations happen ~10ms apart, both reach the audit INSERT step, both pass the UNIQUE check (different `ts`), and both INSERT — even if only one of them actually transitioned a row (the second's UPDATE updates 0 rows because the WHERE clause `current_state == 'candidate'` no longer matches).

The spec's reliance on SQLite write-serialization (WAL single-writer) does protect the *UPDATE* from a TOCTOU, but it does **not** protect the audit INSERT from firing twice with different timestamps. AC 136 ("no duplicate audit rows are produced") will fail under realistic concurrency.

**Fix:** the audit INSERT must be gated on the UPDATE having affected ≥ 1 row. A clean SQL pattern is `INSERT INTO state_transitions ... SELECT ... WHERE EXISTS (SELECT 1 FROM dismissals WHERE finding_identity_hash = ? AND promotion_state = 'local_only')` — or implementation-side check `rows_affected > 0` before INSERT. Either way, the spec needs to say this explicitly; "UNIQUE catches it" is not load-bearing.

### 1.3 §4.4 "different `id=` blocks NEVER conflict structurally" is too strong

§4.5 claims: "Different `id=` blocks NEVER conflict structurally — git's line-based merge handles them cleanly because each block is a contiguous block surrounded by HTML-comment delimiters with unique ids."

This is false when both developers append a new block at the end of the managed section. Git sees two parallel additions to the same line range (the last block + the closing `<!-- /quorum:managed-section -->` line). It will produce a conflict. The unique id is irrelevant — git's merge driver doesn't know about ids.

**Fix:** v0.2 should either (a) walk back the "never" claim and document that adjacent-append produces ordinary git conflicts the user resolves manually (which is true and acceptable), or (b) insert new blocks in id-sorted order so concurrent appends land in different positions (eliminates one class of conflict, costs a small re-sort on every promote).

**My lean: (a).** Don't over-engineer the file format; accept that conventions.md merges like any other file. But fix the claim in §4.5.

### 1.4 Re-promote silently overwrites user hand-edits

§4.4 says "Promote-again on the same hash REPLACES the block (idempotent re-promote)." §8.3 says "Quorum does not refuse to act on user-edited managed-section content."

Combined behavior: if the user has manually edited the body of an existing managed block to refine the wording (a perfectly reasonable thing to do — the file is committed and they're iterating in PR review), then re-runs `quorum convention promote <same-hash>` for any reason (e.g., adding `--text` to update the text from CLI), the manual edits are silently overwritten.

This isn't a security issue, but it is a footgun. v0.2 should at least add: re-promote that detects a divergence between the existing managed block body and the value Quorum would write emits a warning before overwriting, identifying which block changed.

### 1.5 T3 error message vs AC 137 message are not pinned consistently

§3.2 T2's rejection message is specified as `error: <hash> is still a candidate (recurrence=K of N); use --force to bypass`. AC 137 just says "rejected (exit 2)" without enumerating the message text. v0.2 should either lift the message into AC 137 (so it's testable) or drop it from §3.2 (so it's not a frozen contract).

### 1.6 Migration `UPDATE schema_version SET version = 2 WHERE version = 1` silently no-ops on edge cases

Migration step 4 won't update if there's no `version=1` row (fresh install with `schema_version` table but empty), or if there's already a `version=2` row (re-run after partial commit — though the spec says single transaction, so this shouldn't happen). The migration runner should also assert post-migration that `SELECT version FROM schema_version` returns exactly one row with value 2, or fail loudly. v0.2 should either tighten the assertion or remove the WHERE clause (using `UPDATE schema_version SET version = 2` unconditionally, with an `INSERT OR IGNORE` fallback).

---

## 2. Coherence with scoping notes

### 2.1 Scoping leans implemented correctly

§5.1 lean A → §3.4, §4 ✓. §5.2 lean C → §5.1, §5.2 ✓. §5.3 lean B → §3.2 T4, §5.4 config ✓. §5.4 lean A → §6.1 ✓. §5.5 lean C → §2, §7 ✓.

### 2.2 Silent adjudication 1: `--force` / `Shift+P` invented in v0.1

The scoping notes' P2 stated: "promotion to local_only requires recurrence, not just one dismissal" — a principle, not a question.

v0.1 introduces `--force` (CLI) and `Shift+P` (TUI) as a candidate-direct-to-promoted_convention transition that bypasses the recurrence gate entirely (a stronger bypass than P2 even contemplates — it skips local_only as well). AC 138 tests it, AC 160 tests Shift+P, and §10 Q11 then asks the peer reviewers to decide whether to keep it.

This is having it both ways. Either commit (the spec ships the feature, full stop) or don't (the feature is removed and Q11 becomes a "should we add this in 1C v2?" open question). The current configuration — shipping the feature with ACs while flagging it as TBD — is the worst of both worlds.

**My position: remove `--force` and `Shift+P` from v0.2 and defer them to Phase 1C v2 (§11).** Reasoning:

1. The user-facing value of Phase 1C is "the same dismissed thing stops bothering me, and I can promote it into a real rule." `--force` does not advance that value; it caters to a power user who wants to skip the recurrence gate, which is precisely the gate the scoping notes' P2 said was the *principle* of the design.
2. The TUI surface has `Shift+P` already; once shipped, removing it later is a breaking change to muscle memory. Better to never ship it than to ship-then-remove.
3. Phase 1C v2 is the right place for power-user carve-outs once the base flow has user feedback. Adding the carve-out in v1.0 of 1C means the data on "is the recurrence gate too restrictive?" is contaminated by `--force` usage.

If v0.2 elects to **keep** `--force`, then:
- Q11 must be closed (the spec ships it; it's not an open question).
- The behavior must be specified for the "no dismissal row exists for this hash" edge case — does `--force` create the row? Almost certainly no, since there's no `body_snapshot` to source text from. Spec should say.
- The `--force` warning ("you are bypassing the threshold") should be tested by an AC.

### 2.3 Silent adjudication 2: three transitions not in the scoping diagram

The scoping notes' §3 ASCII diagram shows **two transitions**: candidate → local_only (auto) and local_only → promoted_convention (explicit). v0.1 ships **five**:

- T1 candidate → local_only (in scoping, ✓)
- T2 local_only → promoted_convention (in scoping, ✓)
- T3 promoted_convention → local_only (demote — **new in v0.1**)
- T4 any → tombstone (prune — **new in v0.1**)
- T5 boundary undismiss-cascade-to-file (**new in v0.1, with notable surface area**)

T3 and T4 are reasonable additions and I do not object to them — conventions.md is committed and users will regret promotions; SQLite will accumulate dead rows. But the spec frames them as if they were always in scope. They were not. v0.2 should either:
- Acknowledge they are extensions of the scoping notes' state machine, justified by operational realism (preferred), or
- Move them to a §10 question (less appropriate at this stage).

T5 (undismiss cascading to conventions.md write) is the most surface-bearing. Phase 1B's `MemoryStore::delete(hash)` was a SQLite-only operation. v0.1's T5 turns it into a file-writing operation when the row was in `promoted_convention`. This is reachable through the Phase 1B TUI undo stack (§3.2 T5 second paragraph). A user undoing a stale dismissal from a previous session could be surprised to see conventions.md modified. v0.2 should at minimum:
- Warn at the TUI undo site when the entry is in `promoted_convention` (confirmation prompt, parallel to the new `d` keybinding's Y/N), or
- Refuse to undo a `promoted_convention` entry and require explicit `demote` first (cleaner; preserves Phase 1B undo as a non-file-touching surface).

**My lean: refuse to undo a `promoted_convention`.** Phase 1B undo stays clean; the user uses `demote` then `delete` if they really want to remove the row entirely.

### 2.4 Silent adjudication 3: scoping §10 risk "hash brittleness" instrumentation dropped

The scoping notes §10 said: "v0.1 spec should set N=3 but also instrument the SQLite to measure 'near-miss' rates (hashes with N-1 hits but no Nth) so Phase 3's fuzzy work is data-informed."

v0.1 does not include this instrumentation. The recurrence-counter exists (Phase 1B), but there's no specific "how many hashes hit N-1 and stalled?" counter or query. This is a low-cost addition (a SQL query over the existing `dismissals` table) that the scoping notes specifically asked for. v0.2 should add it — either as a `quorum convention list --near-threshold` flag or as an internal metric exposed via `--json`. Worth being explicit about which.

### 2.5 Silent adjudication 4: §7 source-allowlist mapping is Phase 1D design

§7.2 specifies how the future Phase 1D will map convention promotions to `source: model_proposed` (with `proposed_by_model = <the model that flagged the underlying finding>` and `confidence = 1.0`). This is Phase 1D design happening in the Phase 1C spec. It is also questionable on its merits:

- Phase 1B's identity hash includes `sorted_models` (plural). A `Finding` has multiple flagging models. `proposed_by_model` is a single string ≤128 chars. The mapping isn't 1-to-1. v0.1 says singular "the model" but the underlying data is plural. Pick the first sorted model? Concatenate? Join with `;`? This isn't specified.
- The stretch creates an irreversible loss of audit information at the Lippa side. A Lippa memory item with `source=model_proposed, confidence=1.0` looks like a confident model auto-proposal. There's no way for a future reader of Lippa memory to distinguish user-affirmed-via-Quorum from model-confident-auto-proposal.

v0.2 should either (a) drop §7.2's mapping commitment entirely and leave it as "TBD per Phase 1D spec" (the right call IMO — defer Phase 1D design to Phase 1D), or (b) acknowledge the plural-models problem and the audit-loss concern explicitly. The frozen DB columns can stay either way; their shape (`lippa_memory_item_id`, `lippa_proposed_at`) is the bare minimum and is committed independent of the source-mapping choice.

---

## 3. CLAUDE.md compliance

### 3.1 Three-crate boundary

§5.3 says bundle assembly calls `MemoryStore::load_local_only_conventions()`. Both are in `quorum-core`. `quorum-cli::commands::convention` calls into `quorum-core` only. No `quorum-lippa-client` involvement in Phase 1C. **Compliant.**

### 3.2 Read repo content as DATA

§8.2 covers this. Local conventions wrapped in `<<<QUORUM_REPO_CONTENT_BEGIN>>>`. AC 170 enforces. **Compliant.**

Minor: §8.2 says "the user is implicitly trusted (they typed the note)" — this is sloppy. The dismissal `note` is user-typed (Phase 1B free-text), but the `body_snapshot` is **repo content laundered through Lippa's analysis**, not user-typed. Since §4.6 says the convention body comes from user-provided `--text` if present, else from the truncated `body_snapshot`, the latter case is repo-content-derived. The trust wrap is still honored (delimiters do their job), but the prose in §8.2 conflates two sources. v0.2 should disentangle.

### 3.3 Source allowlist (Lippa-side)

Not directly applicable — Phase 1C writes no cloud memory items (§5.5 lean C, AC 166). Forward-compat columns are frozen and tested to never be written (AC 147, AC 167). **Compliant.**

The §7.2 forward-compat statement about mapping has issues (see §2.5 above) but does not violate the allowlist in Phase 1C itself.

### 3.4 Conventions.md trusted only when committed AND byte-identical

§8.1 carries this over. AC 155 codifies it. **Compliant.**

### 3.5 Cloud writes only on explicit promote OR repeated dismissals OR admin approval

Phase 1C ships none of these (all deferred to Phase 1D per §5.5 lean C). §8.4 explicit. **Compliant.**

### 3.6 Pre-commit hook is fast-only

All `quorum convention *` subcommands are pure SQLite + filesystem, no network (§5.1, AC 166). They are not wired into hooks (Q3 leans no). **Compliant.** Q3 should move from §10 to §11 forward-compat (it's not really an open question — the answer is clearly no for the same reason mid-review prompts are deferred).

---

## 4. AC coverage gaps

### 4.1 Missing ACs

The following behaviors are specified in prose but lack a numbered AC:

- **§3.3 auto-promote timing.** The spec says "A newly-promoted `local_only` entry is included in the SAME review's memory context that triggered its promotion" but also "The auto-promote does NOT re-run `quorum review`... the local convention takes effect on the NEXT review." These two sentences are mutually contradictory unless very carefully parsed (the auto-promoted entry is in the SAME review's *memory section* because the bundle is built once after the transition fires, but the *new local rule's effect* is only on the next review because the bundle was already built before the new rule was added). v0.2 should pick one framing and add an AC explicitly verifying which review sees the entry first.
- **§5.3 stderr suppression under `--hook-mode=*`.** No AC.
- **§5.2 TUI keybindings — `Esc to cancel` on the promote modal.** AC 158 covers Enter; doesn't cover Esc.
- **§3.2 T2 temp-file-rename atomic write.** No AC verifies IO crash mid-write leaves no partial conventions.md.
- **§5.1 `quorum convention show <hash>`.** Listed in subcommand table; no AC.
- **§5.1 `quorum convention history <hash>`.** Same.
- **§5.1 short-hash ambiguity disambiguation message.** AC 163 covers it for one command; doesn't generalize.
- **§4.4 first-line marker `<!-- quorum-managed-conventions-md v=1 -->`.** No AC tests read-side behavior when the marker is missing.
- **§5.1 `quorum convention prune --dry-run` writes nothing.** AC 141 mentions `--dry-run` but doesn't assert no SQL writes.
- **§6.2 promote-then-not-committed warning at promote time** (the `promoted <hash>; commit .quorum/conventions.md to apply` message). No AC.
- **§3.2 T5 cascade on undismiss when row is in `promoted_convention`.** AC 143 covers it for `MemoryStore::delete`, but doesn't enumerate the TUI undo path explicitly.

### 4.2 Weak ACs

- **AC 136** ("no duplicate audit rows"). As discussed in §1.2, this AC will fail with the spec as written. The fix requires both spec change and AC strengthening: verify under concurrent invocations spread across timestamps, not just same-timestamp.
- **AC 144** ("concurrent record_seen via two tokio::spawn_blocking workers"). The spec's locked `tokio` runtime is `current_thread` (per Phase 1A). `spawn_blocking` on a current-thread runtime executes blocking tasks on the blocking thread pool, so genuine parallelism is achievable. Good. But the test must intentionally span timestamps (sleep between invocations) to exercise the case from §1.2 above; same-timestamp is a degenerate case.
- **AC 138** (`--force` from `candidate`). Doesn't test `--force` from "no row exists for this hash." Spec doesn't say what should happen; v0.2 must specify.
- **AC 155** (promoted-but-not-committed invisible). Tests invisibility but doesn't test the user-facing messaging at promote time, which is the user's only signal that the promotion is in a transitional state.
- **AC 168** (modifications-outside-Quorum detection). Tests detection at promote/demote sites only. What about `quorum review`? The bundle path reads `conventions.md` and applies the Phase 1A trust check; that's a *different* detection surface (commit-and-clean vs managed-section parsing). v0.2 should clarify whether the §8.3 "out of sync" warning fires from `quorum review` too.

### 4.3 An AC I would add

**AC 173** (proposed) — `convention_text` byte-length vs char-length. §4.3 CHECK is `length(convention_text) BETWEEN 1 AND 4096`. SQLite's `length()` on TEXT counts characters, not bytes. A user-supplied 4096-char string of 4-byte UTF-8 codepoints lands as 16 KB in `conventions.md`, exceeding `BUDGET_CONVENTIONS = 10 KB` (SERVICES.md §2) in a single entry. v0.2 must pick: either (a) tighten the CHECK to bytes (`length(CAST(convention_text AS BLOB))` in SQLite parlance) with a sensible byte cap (say 2500), or (b) document that single-entry overflow is possible and accept the truncation marker behavior. **My lean: (a) — tighten to bytes.** The 4096-char cap was clearly intended as a generous over-provisioning vs the 500-byte bundle cap, but the unit slippage means it's actually under-provisioning vs the 10 KB conventions budget.

---

## 5. Open-question quality

Of Q1–Q11 in §10:

- **Q1** (promote-conflict-with-merge warning) — genuine. Lean reasonable.
- **Q2** (audit log retention on prune) — genuine. Lean reasonable; the privacy trade-off is real.
- **Q3** (promote in hooks) — **not a genuine open question.** Move to §11 forward-compat as a deferred-by-design item. Promote is intentional user action; that's settled.
- **Q4** (per-source-type threshold) — genuine. Defer.
- **Q5** (`sync-check` shape) — **hidden adjudication.** The spec invents the name `sync-check` in §8.3, references it as if it existed, then says it's deferred. Either drop the name (use `list --orphans` only, which is what 1C ships) or commit to the future shape. Don't half-name a thing.
- **Q6** (stderr verbosity) — genuine but minor.
- **Q7** (demote without conventions.md) — genuine edge case. Lean reasonable.
- **Q8** (Lippa wire-format mismatch for local conventions) — moot in 1C; defer to 1D.
- **Q9** (migration rollback / `forward_compat_min_version`) — should just **do** in v0.2. It's cheap and it prevents a real footgun (1C user opens repo with 1B binary, schema is at v2, 1B asserts v1, behavior is undefined). Not a question.
- **Q10** (bundle byte budget revisit) — genuine. Important. The noise-suppression intent backfires if local_only rules crowd CLAUDE.md. The instrumentation (log truncation rates) is the right starting move; do it in v0.2.
- **Q11** (`--force` carve-out) — see §2.2. **Pick one.** Either ship and close the question, or remove and defer.

### Proposed additional questions

**Q12** — Hash brittleness instrumentation (scoping §10 risk D). Should v0.2 include a `quorum convention list --near-threshold` flag (or similar) that surfaces hashes with `recurrence_count = candidate_threshold - 1`? Lean: yes, scoping notes explicitly asked for it.

**Q13** — Re-promote on user-edited managed block. Should re-promote detect divergence from the existing block body and warn (Issue 1.4)? Lean: yes, warn but proceed.

**Q14** — Promote-but-not-committed bundle behavior (Issue 1.1). Should `local_only` rendering persist until conventions.md is committed-and-clean? Lean: yes (my §1.1 Option A).

**Q15** — `--force` from a hash with no dismissal row. Should this be allowed (creating a row from scratch)? Lean: no — `--force` is a candidate→promoted shortcut, not a row-creation tool. If `--force` survives v0.2 at all, this needs explicit rejection.

**Q16** — TUI undo of a `promoted_convention` entry (§2.3 / T5 boundary case). Should Phase 1B's undo refuse this case and require explicit demote first? Lean: yes, refuse.

**Q17** — `convention_text` byte vs char cap (§4.3 footgun). Lean: byte cap.

---

## 6. Data model rigor

### 6.1 Migration

Single-transaction with IF NOT EXISTS guards is sound. The `UPDATE schema_version SET version = 2 WHERE version = 1` is brittle on edge cases (no row, multiple rows, version=0) — see §1.6. Tighten to assert post-state.

The migration ordering (step 1 state_transitions → step 2 conventions → step 3 index → step 4 version update) is correct: state_transitions references dismissals (Phase 1B), conventions references dismissals (Phase 1B). No forward references. ✓

### 6.2 state_transitions table

Schema is well-formed. The `UNIQUE(hash, from_state, to_state, ts)` claim about race protection is mis-specified (see §1.2). Otherwise sound. The `to_state` extension to include `'deleted'` for T5 is a clean way to log undismiss without leaving a dangling reference.

**Indexing.** `idx_state_transitions_hash ON state_transitions(finding_identity_hash, ts)` is appropriate for `convention history <hash>` lookups. No other queries enumerated in §5.1 require additional indexes.

**Cascade behavior.** `ON DELETE CASCADE` from `dismissals`: pruning a row drops its audit log. Documented and intentional (§4.2 note, Q2 lean). Acceptable for 1C.

### 6.3 conventions table

- **`convention_text` cap unit slippage** (§4.3). See §4.3 above. **Fix required.**
- **`promoted_by_user` PII consideration.** Best-effort `git config user.email`. Lands in `.quorum/dismissals.sqlite` which is `.gitignore`-d. Acceptable but worth noting: this is the first user-identifying field in the SQLite store (Phase 1B's `note` was free-text, not necessarily identifying). v0.2 should be explicit about this and consider whether `quorum auth status` or `quorum convention show` ever displays it (probably yes for show; the user wants to know who promoted).
- **`conventions_md_block_id` collision space.** 12 hex chars = 48 bits. Birthday-bound 50% at ~16M rows. Not a practical concern for 1C. Worth a future-edge-case note.
- **UNIQUE on `conventions_md_block_id`** is redundant with the implicit uniqueness from PK = `finding_identity_hash` (the block id is derived deterministically from the hash prefix; collision-free for a given PK set). Keep the explicit UNIQUE for defense-in-depth, but it's belt-and-suspenders.

### 6.4 Idempotency guards

- T1 idempotency: see §1.2 — needs the audit INSERT gating fix.
- T2 idempotency: §4.4 "Promote-again on the same hash REPLACES the block" — needs the manual-edit-detection warning (§1.4).
- Migration idempotency: §4.1 — needs the schema_version assertion tightening (§1.6).

---

## 7. Forward-compat discipline (Lippa seam)

### 7.1 Should `lippa_memory_item_id` and `lippa_proposed_at` exist in v0.1?

The argument for: avoid a v2 → v3 migration in Phase 1D. The argument against: 1C should not carry placeholder state for a phase that hasn't been scoped.

**My position: keep the columns.** Cost is small (two NULL cells per row), the AC structure (147, 167) holds the line on no-writes-in-1C, and the column shape is the absolute minimum (no enums, no booleans, just an ID string and a timestamp). The seam is well-isolated.

But: §4.3's claim that this avoids a Phase 1D migration is **overstated**. SQLite `ALTER TABLE ADD COLUMN` for nullable columns is cheap and well-understood; adding the columns in Phase 1D would not be painful. The real reason to keep them now is *consistency* — Phase 1D shouldn't have to think about migration ordering on a small user base. That's a real but smaller argument. v0.2 should soften the rationale.

### 7.2 Has §7 crept toward partial implementation?

§7.1 (endpoint shape) — fine; documentation only.
§7.2 (source-allowlist mapping) — **yes, this is Phase 1D design happening in 1C.** See §2.5. v0.2 should defer the mapping decision to Phase 1D.
§7.3 (what Phase 1C does NOT do) — appropriate boundary documentation.
§7.4 (seam test) — fine; just asserts schema and NULL acceptance.

The fix is local: rewrite §7.2 to specify only the *required* constraint (CLAUDE.md's source-allowlist hard constraint) and explicitly defer the *mapping choice* to Phase 1D.

---

## 8. UX coherence

### 8.1 CLI

Subcommand table is coherent. Exit-code mapping (0 ok / 2 tooling) is consistent with Phase 1A taxonomy. `--text` / `--from-editor` mutual exclusion is appropriate. `--quorum-dir` honored. No network. ✓

**Issues:**
- `convention promote --force` (see §2.2 — pick one).
- `convention prune` on `local_only` drops the audit log silently. Q2 leans accept-CASCADE; that's fine for *candidate* prune, but `local_only` prune is more user-visible and more deliberate. v0.2 should at least surface a confirmation prompt for `--state local_only` prune (parallel to `git push --force`'s confirmation cadence), or document that the audit log loss is part of the deal.
- No `convention undo` for post-session reversal. Users who run `convention promote` and immediately regret it have to run `convention demote` (which leaves a paper trail of promote+demote in the audit log, not just a no-op). Acceptable but worth a §10 note.

### 8.2 TUI

`H` for history view is a sensible binding. `p`/`d`/`x` in history view are reasonable. **But:**
- `x` (delete) in history view has no confirmation modal per spec; `d` (demote) does. Inconsistent destructiveness gates. v0.2 should align — likely add Y/N to `x` too. (Or the spec is assuming `x` inherits a Phase 1B confirmation pattern — if so, say so.)
- `Shift+P` (force-promote) — see §2.2.
- Body pane shows "most recent 5 transition log entries" (§5.2). Pagination behavior for >5 transitions is unspecified. v0.2 should say.
- Filter `/` semantics unspecified (what fields are searched). Likely inherited from Phase 1B; say so.
- TUI snapshot is per-session (§2). Acceptable Phase 1B carry-forward but worth a one-liner in §5.2 documenting the user-visible implication.

### 8.3 Help text

§5.5 is fine. Minimal. Acceptable.

---

## 9. Trust model carryover

### 9.1 Phase 1A preserved

`.quorum/conventions.md` committed-and-clean check holds (§8.1, AC 155). ✓

### 9.2 Phase 1B preserved

Repo content as DATA, wrapped in delimiters (§8.2, AC 170). ✓ (Modulo the §3.2 sloppiness about "user-typed note" vs body_snapshot — see §3.2 of this review.)

### 9.3 New attack surface

Modest, mostly via conventions.md:

1. **Conventions.md as instruction-injection vector.** A teammate writing a managed-section block with malicious payload — neutralized by the existing `<<<QUORUM_REPO_CONTENT_BEGIN>>>` delimiter wrap at bundle time. Low impact.
2. **First-line marker (`<!-- quorum-managed-conventions-md v=1 -->`) trust.** §4.4 says this is "solely for detection." Read-side behavior when missing is unspecified. v0.2 should pin: lean is warn-but-proceed.
3. **Out-of-sync managed blocks.** §8.3 covers detection at promote/demote sites; AC 168 verifies. Does `quorum review` also detect? Spec is silent. v0.2 should clarify.

### 9.4 Net trust assessment

Compliant. The new surface is small and well-contained. The minor gaps are spec-clarity issues, not design holes.

---

## 10. Recommended actions for v0.2

In priority order:

1. **Decide `--force` / `Shift+P`** (Q11). Ship it or remove it. Don't both.
2. **Fix T1 audit-insert race** (§1.2). Gate the audit INSERT on UPDATE rows_affected > 0; strengthen AC 136 and AC 144 to span timestamps.
3. **Resolve promote-but-not-committed window** (§1.1, Q14). Pick Option A / B / C; instrument with an AC.
4. **Tighten `convention_text` cap to bytes** (§4.3, Q17). Pick a byte cap (say 2500 worst-case, 500 default consistent with bundle cap).
5. **T5 undismiss behavior on `promoted_convention`** (§2.3, Q16). Refuse and require demote first, or warn loudly.
6. **Add hash brittleness instrumentation** (§2.4, Q12). Cheap; scoping notes asked for it.
7. **Add migration rollback marker** (Q9). Just do it; no need for a question.
8. **Walk back §4.5's "never conflict" claim** (§1.3). Honest documentation beats overstated guarantee.
9. **Re-promote silent-overwrite warning** (§1.4, Q13).
10. **Defer §7.2 source-mapping to Phase 1D spec** (§2.5). Keep the columns; drop the mapping commitment.
11. **Disentangle §8.2's "user-typed note" prose** (§3.2). Clarify body_snapshot is repo-derived.
12. **Move Q3 to §11** (§5). Not an open question.
13. **Resolve Q5 `sync-check` naming** (§5). Either commit or drop.
14. **Fill AC coverage gaps** (§4.1).
15. **Pin error message in T2 or remove from §3.2** (§1.5).
16. **Tighten schema_version migration assertion** (§1.6).

Items 1, 2, 3, 4, 5 are blocking. Items 6–10 are strongly recommended. Items 11–16 are polish.

---

## Closing

v0.1 is a serviceable draft. The state machine is well-formed, the data model is mostly clean, the trust-model carryover is faithful, and the §5 scoping leans are implemented. The objections above are correctable in a v0.2 pass; none of them require redesigning the spine.

The single biggest concern is the `--force` / Q11 situation, which is a pattern I want to flag generally: spec drafts that ship features with ACs while flagging the feature as TBD make the peer-review step into a feature-decision step, which it should not be. Peer review should adjudicate edges; the spine should come in unhedged. v0.2 should be unhedged on `--force` one way or the other.

*— end review C —*
