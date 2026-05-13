# Quorum Phase 1C — v0.1 spec peer review

**Reviewer:** A (fresh-session Claude; no shared context with other reviewers)
**Date:** 2026-05-13
**Subject:** `specs/Quorum-Phase1C-Spec-v0_1.md`
**Inputs read in full:** CLAUDE.md, HISTORY.md (Phase 0 → 0.2.1), SERVICES.md, Phase 1C scoping notes, Phase 1C v0.1 spec.

---

## 0. Executive summary

v0.1 is a competent and unusually disciplined first draft. The five §5 leans from the scoping notes are adopted cleanly, the §2 non-goals genuinely constrain scope, the migration design is conservative, and the trust-model carryover in §8 is the strongest section of the document. The author resists most temptations to over-engineer.

That said, I want v0.2 to make six concrete changes before going to Rolf. In rough order of severity:

1. **Fix the `state_transitions` / `undismiss` (T5) contradiction in §3.2 + §4.2.** Audit rows for T5 cannot survive the `ON DELETE CASCADE` from `dismissals`. The spec asserts the audit shape but the FK guarantees the row evaporates the instant it's inserted. Either drop T5 from the audit table (and remove `'deleted'` + `'explicit_undismiss'`), or move undismiss audit to a separate, FK-free trail. §10 Q2 touches this but takes the wrong position.

2. **Resolve `--force` / `Shift+P` against the CLAUDE.md hard constraint.** CLAUDE.md says "Never write a `convention` directly from a single dismissal." `--force` from a `candidate` with `recurrence=1` does exactly that. §10 Q11 flags it as a scoping-note carve-out but doesn't engage with the harder framing: this is a potential hard-constraint violation, not a P2 carve-out. My lean: constrain (Q11 option c, with recurrence ≥ 2 floor) or remove (Q11 option b). Shipping as-is (Q11 option a) is the riskiest of the three.

3. **Pull the `lippa_memory_item_id` / `lippa_proposed_at` columns from §4.3 back out of v0.1.** §7's promise to be "forward-compat seam only" creeps into partial Phase 1D implementation when frozen columns ship in 1C. The Phase 1D shape is not adjudicated (§7.1 ends with "Phase 1D recons full shape"), and a future v2→v3 migration to add columns is cheaper than locking in pre-design shape now. Reading carefully, the spec is asking 1C to bake assumptions about 1D that 1D might overturn.

4. **Resolve the §3.3 timing contradiction.** Two adjacent paragraphs say "the local convention takes effect immediately, on the very review whose dismissal-recurrence pushed it over the threshold" and "The local convention takes effect on the NEXT review." Both meanings are defensible but they're not the same meaning. Cleanup is mechanical; surfacing the right one matters because it determines what Lippa sees in the T1-triggering review.

5. **Re-weight the §6.2 "promoted-but-not-committed" UX cost.** The spec calls it "a small UX cost." It's larger than that: Phase 1A's trust model excludes a dirty `.quorum/conventions.md` from the bundle entirely, so a single uncommitted promotion makes the bundle lose ALL previously-committed conventions (not just the new one) until the user commits. A "small cost" framing misrepresents the regression. v0.2 should either bridge the gap (keep `local_only` rendering until the file is committed-and-clean), or accept the cost with a much stronger user-facing notice.

6. **Clarify the SQLite + filesystem transaction boundary in §3.2 T2 / T3 / T5.** "SQLite transaction is wrapped around the file-write attempt" is misleading. SQLite cannot roll back filesystem operations; what's really happening is temp-file-rename-then-commit, with a partial-failure window if the commit fails after the rename. The orphan path is recoverable (AC 169) but the failure semantics are worth a paragraph.

Everything else in this review is supporting detail or coverage gaps. v0.1 is solid enough that the document is mostly hardening, not redesign.

---

## 1. Internal consistency

### 1.1 §3.2 T5 (undismiss) vs §4.2 cascade — hard contradiction

§4.2 declares `to_state` accepts `'deleted'` and the `trigger` enum includes `'explicit_undismiss'`. The clear intent is that T5 (undismiss) writes an audit row capturing the deletion. But §4.2 also declares:

```
finding_identity_hash BLOB NOT NULL REFERENCES dismissals(...) ON DELETE CASCADE
```

When `MemoryStore::delete(hash)` runs, the cascade fires within the same transaction. If the audit row is inserted before the DELETE, the cascade removes it. If after, the FK constraint rejects it (parent row already gone). There is no transaction ordering that lets a T5 audit row survive.

Reading §3.2 T5 narrowly, it does NOT explicitly say "writes an audit row" — only T3 does. If T5 doesn't write an audit row, then `to_state='deleted'` and `trigger='explicit_undismiss'` are unreachable values in the CHECK constraints, and §4.2 should drop them.

This is the most concrete bug in v0.1. §10 Q2 flirts with it ("Should pruned-candidate audit rows be retained?") but takes the wrong frame — it's about prune-retention, not the structural impossibility of T5 audit. v0.2 needs to pick:

- **(a)** Drop `'deleted'` and `'explicit_undismiss'` from §4.2; document T5 as audit-trail-silent.
- **(b)** Move undismiss audit to a separate table (`audit_archive` or similar) with no FK back to `dismissals`. Re-raises the leak-surface concern §10 Q2 flagged — the hash + timestamp combo without title/note is borderline acceptable; adding any title would be a violation.
- **(c)** Change the FK to `ON DELETE SET NULL` and let audit rows survive with a NULLed parent. Awkward but workable.

I lean (a). T5 is rare (Phase 1B's undo stack is per-session and dismissals are rarely undone explicitly via the API). The forensic value of "this hash was undismissed at time T" is low compared to the structural complication of supporting it.

### 1.2 §3.3 — auto-promote timing contradiction

§3.3 contains two paragraphs that disagree:

> "A newly-promoted `local_only` entry is included in the SAME review's memory context that triggered its promotion — the developer sees the auto-promote stderr note and the local convention takes effect immediately, on the very review whose dismissal-recurrence pushed it over the threshold."

> "The auto-promote does NOT re-run `quorum review` to apply the new local rule to the current findings; the bundle is built once. The local convention takes effect on the NEXT review."

The author probably means: the convention rides in THIS review's bundle memory context (i.e. Lippa sees it as input), but the dismissals-filter pass that runs before bundle assembly already happened, so the underlying noisy finding still appears in THIS review's output. The convention only suppresses that finding starting on the NEXT review.

If that's the intent, the second paragraph's wording is correct and the first paragraph's "takes effect immediately" is wrong. Fix by replacing the first paragraph's last clause with something like: "the local convention is included in this review's memory context (visible to Lippa) but does not retroactively suppress findings already emitted in the current run."

This matters operationally because the developer behavior under each meaning differs: if the convention "takes effect immediately" they expect the noisy finding to disappear from this review; if it "takes effect on the NEXT review" they expect to see it once more.

### 1.3 §3.2 T2 / T3 — asymmetric failure-mode treatment

T2's "Failure modes" paragraph spells out the temp-file-rename pattern and the rollback behavior on IO error. T3 (demote) doesn't. T5 has an AC (AC 143) that mentions temp-file-rename for the cascade case, but T3 has neither prose nor AC pinning the symmetric behavior. §10 Q7 partially addresses the missing-file edge case for demote but not the general IO-failure case.

T3 should mirror T2's failure modes. AC 140 should add the temp-file-rename + transaction rollback assertion.

### 1.4 §3.2 T2 — "single SQLite transaction wraps file-write" framing

The phrase suggests cross-domain atomicity that SQLite alone cannot deliver. The real semantics are:

1. BEGIN.
2. UPDATE dismissals, INSERT conventions, INSERT state_transitions.
3. Write temp file (`.quorum/conventions.md.tmp`).
4. Rename temp → `.quorum/conventions.md` (filesystem-atomic on POSIX rename; near-atomic on Windows `MoveFileExW`).
5. COMMIT.

Failure cases:
- Step 3 or 4 fail → ROLLBACK, no visible change anywhere. Clean.
- Step 5 fails after step 4 succeeds → the file now has a managed block but no `conventions` row in SQLite. Orphan detected by `list --orphans` (AC 169). User must demote-by-id or manually edit the block out.

This is the right design; the prose just needs to describe it accurately. The "single transaction" framing is what makes me worry an implementer will try to do step 5 before step 4 (the simpler interpretation) and end up with the inverse orphan: SQLite says promoted, file unchanged.

### 1.5 §6.2 — describes a UX regression as a "small cost"

Phase 1A's trust check is committed-AND-byte-identical-to-HEAD. After `quorum convention promote`, conventions.md is byte-modified vs HEAD. The whole file becomes invisible to the bundle until commit. This means:

- Pre-promote: bundle has 5 committed conventions + 1 noisy finding kept by `local_only`.
- Post-promote (uncommitted): bundle has **0 conventions** (file is dirty) + the dismissed finding is also no longer in `local_only` (its state moved to `promoted_convention`).

That's a net loss of 5 conventions plus the dismissal — the entire conventions context disappears from the bundle, not just the new one. §6.2 acknowledges the new entry being invisible but doesn't acknowledge the cascading loss of every other committed convention.

Mitigations to consider for v0.2:

- **Bridge state:** keep `local_only` bundle rendering active for a row whose state is `promoted_convention` AND whose conventions.md is uncommitted. Requires checking git tree state per-row at bundle time; doable via the same blob-vs-HEAD check Phase 1A uses, just inverted.
- **Stronger nudge:** at promote time, print not just `commit .quorum/conventions.md to apply` but a literal `git add .quorum/conventions.md && git commit -m '...'` snippet on stderr, plus a one-line warning: "**Until you commit, your conventions section is empty in the bundle.**" That makes the cost visible.
- **Auto-stage (no auto-commit):** `quorum convention promote` runs `git add .quorum/conventions.md` automatically. Doesn't help the trust-model gap (staged ≠ committed) but reduces the friction of "promote then forget."

I lean toward bridge state OR the stronger nudge. Auto-stage is a partial fix and probably violates the read-only spirit of CLAUDE.md ("Read-only side effects in v1. Quorum reads source code and writes review output to its own data directory. It never modifies user code"). One could argue `git add` is staging metadata, not user code; conservative reading says don't.

### 1.6 §3.2 T2 steps 4 and 5 — redundant or sequenced?

Step 4 says "append a managed block within the `<!-- quorum:managed-section -->` fence (auto-created if absent)" and Step 5 says "If `.quorum/conventions.md` did not exist, create it with just the fence + the new block." These overlap. Step 4 talks about the FENCE being auto-created in an existing file; Step 5 talks about the FILE being auto-created. Combine into a single rule that handles the three cases: file absent / file present without fence / file present with fence.

### 1.7 Minor — AC 144 references `tokio::spawn_blocking`

AC 144 specifies "verified by a parallel-invocation test using two `tokio::spawn_blocking` workers." Per criterion 36 of Phase 1A (HISTORY.md), `quorum-core` has no tokio dep. If the test lives in `quorum-core`, this AC induces a tokio dev-dependency on `quorum-core`. Use `std::thread::spawn` for the parallel-invocation test; matches the sync trait surface.

This is a small wording fix but it points at a real architectural concern: where does the test live? If in `quorum-cli/tests/`, the test is testing internal memory-module behavior through the binary, which is awkward. If in `quorum-core/tests/`, it pulls tokio for no reason. Best fix: `std::thread` in `quorum-core`.

---

## 2. Coherence with scoping notes

### 2.1 The §5 leans were adopted cleanly

§5.1A (exact-hash, N=3, all-time, configurable) → spec §3.4. ✓
§5.2C (CLI + TUI) → spec §5.1 + §5.2. ✓
§5.3B (asymmetric 90d) → spec §3.4, T4. ✓
§5.4A (concatenate into memory) → spec §6.1. ✓
§5.5C (defer cloud) → spec §7. ✓ with creep, see §7 below.

No silent adjudication on the five labeled forks. The author was explicit about taking the leans.

### 2.2 Silent additions to the surface area beyond scoping §8 sketch

Scoping §8 sketched ~12 ACs. v0.1 §5.1 expands the CLI surface by adding several subcommands and flags not anticipated:

- `quorum convention show <hash>` — new.
- `quorum convention history <hash>` — new.
- `quorum convention prune --dry-run` — new.
- `quorum convention prune --older-than <duration>` — new (scoping said only "prune").
- `quorum convention list --json` — new.
- `quorum convention list --orphans` — new (acknowledged in §10 Q5 as a partial answer to a deferred "sync-check" question).
- `quorum convention promote --from-editor` — new.
- `quorum convention promote --force` — new (acknowledged in §10 Q11).

Most of these are reasonable additions for a v1.0-bound spec to make. But two of them (`--force` and `list --orphans`) materially change the system's design, not just its surface, and they deserve scoping-level adjudication, not v0.1 prose adjudication. `--force` is treated below in §3 (CLAUDE.md compliance) and §5 (Open-question quality). `list --orphans` is fine but §5.1's table omits it; AC 169 references it but §5.1 doesn't list it. Fix the table.

### 2.3 `lippa_*` columns — borderline silent adjudication

Scoping §5.5 lean C said: "`/api/v1/memory/propose` stays unused but its forward-compat seam (the source-field shape) is preserved." That's ambiguous between (a) preserve the conceptual mapping in §7 prose; (b) preserve concrete SQLite columns in v0.1 schema. v0.1 interprets it as (b) and ships frozen columns. This wasn't called out as an open question in scoping §10. The author should explicitly flag it as a Phase 1D-coupling decision; right now §7.2 just slips it in.

See §7 of this review for the substantive position.

### 2.4 `state_transitions` extra columns

Scoping AC sketch said audit captures `(hash, from_state, to_state, trigger, timestamp)`. v0.1 adds `by_review_session_id` and `recurrence_at_transition`. Both are reasonable forensics columns. Not a silent adjudication, just refinement — but worth mentioning so it's not lost.

---

## 3. CLAUDE.md compliance

### 3.1 The `--force` constraint problem

CLAUDE.md verbatim: "Memory writes go through a dismissal state machine. **Never write a `convention` directly from a single dismissal.** State: `candidate` → `local_only` → `promoted_convention`. Cloud writes only on explicit promote OR repeated dismissals OR admin approval."

Two readings of the bolded sentence:

- **Literal reading:** "Never write a convention directly from a single dismissal" = if there has been only one dismissal, never write a convention. This forbids `--force` on a `candidate` row with `recurrence=1`.
- **Spirit reading:** "Never AUTOMATICALLY write a convention as a side effect of one dismissal" = the system itself never auto-promotes from a single dismissal; user-initiated `--force` is a separate intent. This permits `--force`.

§10 Q11 frames the issue as a scoping-note-P2 carve-out: "Scoping note P2 stated the principle 'promotion to local_only requires recurrence' is fixed; --force is a power-user carve-out the scoping doc didn't anticipate."

P2 isn't the right reference. P2 is a scoping convention. CLAUDE.md is a hard constraint. The peer-review job is to surface this gap. v0.1 shouldn't ship `--force` without Rolf's explicit ruling on which reading of CLAUDE.md governs.

My lean for v0.2: **constrain (Q11 option c), with a floor of `recurrence ≥ 2`**, AND require `--force` to also pass `--allow-bypass-threshold` (or equivalent) to invoke. The floor satisfies the literal reading ("not a single dismissal"); the double-flag prevents accidental invocation. Status-bar wording for `Shift+P` should reflect the floor: a `candidate` row with `recurrence=1` is rejected even under `Shift+P` (status bar: "force-promote requires recurrence ≥ 2").

The fallback position, if the Rolf reading is that CLAUDE.md is intent-permissive: leave `--force` as-is but add a CLAUDE.md amendment that explicitly says "user-initiated promotion may bypass the threshold." Make the constraint say what the system does, not less.

### 3.2 Other CLAUDE.md constraints — compliant

- **Three-crate boundaries:** v0.1 adds `MemoryStore::load_local_only_conventions()` to `quorum-core::memory` and calls it from `quorum-core::bundle`. No cross-crate dep change. ✓
- **No Lippa server changes from this repo:** Phase 1C is pure forward-compat seam; nothing required on the Lippa side. ✓ (modulo §7's seam-vs-implementation question.)
- **Read-only side effects in v1:** Phase 1C writes to `.quorum/dismissals.sqlite`, `.quorum/conventions.md`, and `.quorum/config.toml` (the last only via `quorum link`). All in `.quorum/`. No writes to user code. ✓
- **Read repo content as DATA:** §8.2 carries forward the delimiter wrap. ✓
- **`.quorum/conventions.md` trusted only when committed:** §8.1 carries forward. ✓ (UX cost addressed in §1.5 of this review.)
- **Source allowlist on Lippa writes:** §7.2 reaffirms `source: model_proposed`. Since Phase 1C doesn't write, the constraint is structural, not behavioral, in this milestone. ✓
- **Cargo workspace boundaries:** unchanged. ✓
- **Fail-open at hooks on network errors:** unchanged; convention subcommands have no network surface. ✓
- **Privacy-mode redaction discipline:** Phase 1C doesn't touch redaction. ✓

The `--force` issue is the only material CLAUDE.md compliance concern.

---

## 4. AC coverage (§9, AC 133 – 172)

Coverage is good for the state-machine spine and the data model. Gaps are concentrated in error-paths, edge cases, and the UX surface.

### 4.1 Missing ACs I'd add

- **T5 audit behavior** — whichever resolution §1.1 lands on, ship an AC that pins it. (E.g., "AC 174: `MemoryStore::delete(hash)` on a `promoted_convention` row produces no `state_transitions` row" if option (a); or "AC 174: produces an audit row in `audit_archive` with `trigger='explicit_undismiss'`" if option (b).)
- **T2 / T3 / T5 atomicity** — pin the temp-file-rename pattern in an AC for each (currently only AC 143 covers T5). Test that a simulated rename failure leaves the SQLite state unchanged.
- **Migration failure** — AC 145 covers happy-path migration and idempotency. Add: migration failure surfaces exit 2 with no schema_version bump and no partial table creation visible.
- **Migration rollback / downgrade** — §10 Q9 lean is "yes, add `forward_compat_min_version` row." If shipped, AC for it. If not, AC stating "a 1B binary opening a 1C-migrated DB exits 2 with a clear error message."
- **`--from-editor` flow** — AC 139 covers `--text` and the title-derived placeholder, but not `--from-editor`. Add AC: `--from-editor` opens `$EDITOR`, captures its content, applies the same 1..4096 length check as `--text`. Mutual exclusion with `--text` already in §5.1; should be tested.
- **Hook-mode auto-promote stderr suppression** — §5.3 mentions suppression under `--hook-mode=*`. No AC. Add.
- **Configuration re-validation per invocation** — §5.4 says "configuration store is not cached across invocations." Add AC: out-of-range config value in `.quorum/config.toml` is rejected on the next CLI invocation, not on the first one that opened the store.
- **`candidate_expire_days = 0` interaction with `prune --older-than`** — §3.4 says 0 disables auto-expire, but §3.2 T4's default is `--older-than 90d`. What does `prune` (no flags) do when `candidate_expire_days = 0`? Document and AC.
- **Conventions.md auto-creation** — §3.2 T2 step 5 says the file is auto-created when absent. AC 148 mentions block-keying behavior but doesn't test the create-from-scratch case.
- **`promoted_by_user` NULL fallback** — §4.3 says NULL when git config has no `user.email`. AC missing.
- **`conventions_md_block_id` UNIQUE collision behavior** — §4.3 has UNIQUE. AC for what happens on a (vanishingly rare) collision: T2 fails with exit 2 and a clear message; the suggested remediation is to demote the conflicting hash. Even if the test is structural rather than behavioral, document the failure mode.
- **CLAUDE.md / AGENTS.md / .cursorrules ordering preservation under local-conventions interleaving** — AC 152 says they precede local conventions. Add: the existing inter-file ordering among the three is unchanged.
- **`convention show` and `convention history`** — both subcommands exist in §5.1 but only `list` has an AC. Add ACs covering output shape and exit-code semantics.

### 4.2 Existing ACs worth tightening

- **AC 144** — see §1.7 of this review. Use `std::thread::spawn`, not `tokio::spawn_blocking`.
- **AC 155** — pins that the bundle is invisible during the promoted-but-not-committed window. Should also pin the BEHAVIOR of conventions.md when the WHOLE file is dirty (Phase 1A trust model excludes it entirely). The current AC reads as "this one row is invisible," but the consequence is broader. See §1.5 of this review for the framing.
- **AC 168** — "modifications outside Quorum are detected... and surface as a stderr warning. The operation proceeds." Add a case: if the entire `quorum:managed-section` fence is missing (e.g., user deleted it), promote re-creates it (idempotent fence creation).
- **AC 171** — "All 197 Phase 1B tests pass after the migration." The 197 number is stale-prone (it appeared in HISTORY.md Phase 1B close and also Phase 0.2.1 close, unchanged). Better: "All Phase 1B tests pass; no Phase 1B test fails after the migration." Drop the count; track regression-shape, not regression-count.

### 4.3 ACs that overlap with §10 open questions

If v0.2 closes any of Q1, Q5, Q6, Q9, Q10 in the direction of "ship it," they need ACs. Currently they're prose questions, which is appropriate for v0.1 but won't survive v1.0.

---

## 5. Open-question quality (§10 Q1 – Q11)

Most of §10 is genuine. The exceptions:

### Q8 (Lippa wire-format compatibility) is not a 1C question

Q8 asks whether local conventions' markdown format will play nicely with Lippa Phase 1D. That's a 1D recon question. Either defer to 1D scoping or remove. Keeping it in 1C's §10 invites the implementer to anticipate Lippa-side decisions that don't belong here.

### Q9 (migration rollback) — the lean is "yes, do it," so just ship it

Q9 says: "Should the 1C migration write a `forward_compat_min_version` row so 1B fails with a clear error (vs corrupting data)? Lean: yes; cheap to add. Flag for v0.2 to formalize." The lean is decisive and the cost is one column. v0.2 should ship this in §4.1 plus an AC, not leave it as an open question.

### Q10 (bundle byte budget revisit) — should be a §11 forward-compat note

Q10 says the 20 KB cap might be wrong if many local rules accumulate; the lean is "ship the cap, instrument truncation for data." That's a §11 future-work item, not an open question. The instrumentation could be added to v0.1 directly as part of §6.4's overflow-marker rules (one line of additional tracing).

### Q11 (--force / Shift+P) — should be promoted to a hard adjudication

Already covered in §3.1 of this review. Q11 frames it as one of three paths to choose; my position is that the framing needs to be redone against CLAUDE.md's hard constraint, not against scoping P2. Either way, v0.2 must take a position.

### Additions I'd suggest for §10 (Q12 +)

- **Q12 — T5 audit semantics.** What does `MemoryStore::delete` produce in `state_transitions`, given the cascade conflict? (See §1.1.)
- **Q13 — §3.3 timing wording.** Which paragraph is the source of truth? Resolving this isn't really an open question — the author probably intended one specific meaning — but flag it explicitly for v0.2.
- **Q14 — §6.2 promoted-but-not-committed UX.** Accept the regression, bridge-state to mitigate, or aggressive promote-time nudge? (See §1.5.)
- **Q15 — SQLite + filesystem transaction failure semantics.** What does the user see, and what's the recovery path, when commit succeeds and rename fails, or vice versa? (See §1.4.)
- **Q16 — `conventions_md_block_id` prefix length.** 12 hex chars = 48 bits. Birthday-paradox collision is at ~17M entries; realistic? Probably yes for v0.1, but document the reasoning so future-work knows when to widen.

The rest of Q1 – Q7 are well-formed; ship them through to v0.2 adjudication.

---

## 6. Data model rigor

### 6.1 §4.1 migration — solid, with two edge cases to harden

- Idempotency via `IF NOT EXISTS` is right.
- `UPDATE schema_version SET version = 2 WHERE version = 1` silently does nothing on version=0, NULL, missing row, or version=3+. Add an explicit pre-migration assertion: "if `schema_version != 1`, fail with exit 2 and a clear message that this binary expects schema v1 input." This is the same defensiveness the spec applies elsewhere (e.g., conventions.md modifications detected). Trivially small to add and prevents the silent-no-op trap.
- `CREATE TABLE IF NOT EXISTS` won't update an existing table with a different schema. If a previous partial 1C migration left a wrong-shape `state_transitions` table behind (e.g., the user ran a pre-release binary), v0.1 will silently use the wrong-shape table. Defensive option: `DROP TABLE state_transitions; CREATE TABLE state_transitions ...` inside the migration, gated on `schema_version = 1`. Arguable against — destroys data on retry — but the table is empty after a failed migration that didn't reach the UPDATE.

The migration runs on first `quorum review` or `quorum convention *`. AC 145 covers happy-path. Add an AC for the schema-mismatch-then-abort case if the defensive change ships.

### 6.2 §4.2 `state_transitions` — strong other than the cascade issue

Covered in §1.1 above. Other observations:

- `id INTEGER PRIMARY KEY AUTOINCREMENT` — fine. AUTOINCREMENT in SQLite prevents rowid reuse, which is the right call for an append-only audit log.
- `UNIQUE (finding_identity_hash, from_state, to_state, ts)` — fine as defense-in-depth. The real idempotency guard is the `WHERE promotion_state = 'candidate'` clause in the UPDATE (§3.2 T1), which means concurrent transactions that both pass the read see the same `candidate` state, but SQLite's WAL serialization means only one can actually flip it; the second's UPDATE returns 0 rows changed, at which point the application code shouldn't INSERT the audit row at all. Spec §3.2 T1 should make this loop explicit: "if the UPDATE returns 0 rows changed, skip the audit insert." Implementer might otherwise blindly INSERT.
- `by_review_session_id TEXT` nullable. Phase 1C-driven T1 transitions always have it; CLI-driven T2 promotes don't (no session). Document.
- `recurrence_at_transition INTEGER` nullable. Always non-null for T1; null for T2/T3/T4. Document.

### 6.3 §4.3 `conventions` table — see §7 of this review

Forward-compat `lippa_*` columns are addressed in §7 below.

Otherwise:

- PK on `finding_identity_hash` — one convention per hash. Re-promote uses INSERT OR REPLACE pattern (or DELETE + INSERT inside the txn); not specified. Document.
- `convention_text` 1..4096 — fine; matches `body_snapshot` upper bound from §6.3.
- `conventions_md_block_id TEXT NOT NULL` — 12-hex prefix. Collision rate per §5 Q16.
- `promoted_by_user` best-effort from git config user.email — fine; privacy-friendly fallback to NULL.

### 6.4 §4.4 conventions.md format — solid

The section-fenced, block-keyed format is the right call. Mergeability is genuinely improved by the per-block delimiters. Line-ending preservation (AC 150) is the right discipline for cross-platform repos.

One concern: blocks have a stable `id=` but the BODY can change on every re-promote. If two developers re-promote the same hash with different `--text` bodies, git's line-based merge produces a conflict that the user resolves manually. §4.5 acknowledges this. Fine.

Edge case worth specifying: what if the user manually re-orders the blocks inside the fence? Quorum's promote/demote operates by `id=` match, which is order-independent. So re-ordering is a no-op for Quorum. Worth a sentence in §4.5.

### 6.5 §4.5 merge-conflict discipline — solid

Different-id blocks merge cleanly; same-id blocks produce manual conflicts; promote doesn't auto-commit. All correct.

### 6.6 §4.6 audit-trail leak surface — solid

`state_transitions` carries hash + ts + trigger + recurrence; no titles, no notes, no bodies. `conventions.convention_text` is user-controlled (either `--text` or title-derived); the dismissal `note` is never lifted. This is a clean carry-forward of Phase 1B's privacy discipline.

One adjustment: I'd be more restrictive on the auto-derived placeholder. §4.4 says "the body comes from user-provided `--text` if present, else from the truncated `body_snapshot`." But `body_snapshot` is `Finding.body` — LLM-generated content from the upstream review. Auto-deriving the convention body from LLM output and writing it to a committed file is an LLM-output-to-source path. The bundle delimiter protects against re-interpretation, but the principle is "user-authored conventions, not LLM-authored." Tighter default: title only (no body) when neither `--text` nor `--from-editor` is supplied. Force the user to type or accept a literal title-as-body.

This is a small change with a real principle behind it. Worth doing.

---

## 7. Forward-compat discipline (§7)

§7's mission is to be a seam, not an implementation. Two things in v0.1 push it past seam:

### 7.1 The frozen `lippa_*` columns

§4.3 includes `lippa_memory_item_id TEXT` and `lippa_proposed_at INTEGER` columns. §7.2 justifies them as "frozen as forward-compat for Phase 1D cloud sync." Phase 1C never writes them; tests assert they remain NULL (AC 147, AC 167).

The justification is migration cost: adding columns in 1D needs v2→v3, which is more work than including them now. But the shape isn't adjudicated:

- §7.1 says the Lippa endpoint returns ProposeResponse with `memory_item_id` — but the response shape isn't fully enumerated ("additional fields not enumerated here; Phase 1D recons full shape").
- §7.2 commits Phase 1D to a specific source-field mapping ("source: model_proposed, confidence: 1.0"). But scoping §5.5 explicitly identified this as a fork to be re-evaluated in 1D (option A vs option B). v0.1 silently picks option A for 1D.
- A retry / outbox design might prefer a separate table over columns on `conventions`; freezing columns now commits to the embed-in-conventions design.

The cost of NOT freezing the columns is small: a v2→v3 migration in 1D adds two ALTER TABLE statements. The cost of freezing them now is locking in design assumptions about 1D that 1D might overturn. Trade looks wrong to me.

**Recommendation:** remove `lippa_memory_item_id` and `lippa_proposed_at` from §4.3 in v0.2. Keep §7's prose as documentation of the future shape, but don't bake columns. AC 147 / AC 167 / §7.4 all become unnecessary; drop them.

If the columns stay, then at minimum §7.2's "Phase 1D MUST map `quorum convention promote` writes to `source: 'model_proposed'` with ... `confidence = 1.0`" should be softened: "Phase 1D will adjudicate the source-field mapping; option A (source: model_proposed, confidence: 1.0) and option B (user_promoted via Lippa-side spec) are both viable per scoping §5.5."

### 7.2 §7.4 forward-compat seam test

The test asserts the columns exist and accept NULL. If the columns stay, the test is right. If the columns go, drop the test. Either way the test isn't load-bearing for the seam principle.

### 7.3 §7.1 endpoint schema documentation — fine as-is

Documenting the read-only endpoint shape is fine; that's pure documentation and doesn't constrain 1C behavior. Keep §7.1.

---

## 8. UX coherence

### 8.1 CLI subcommands — mostly good, two gaps

`promote / demote / prune / list / show / history`: clean verb set. `promote ↔ demote` is a clean inverse pair; `prune` is the destructive operation with `--dry-run` as the safe-preview.

Gaps:

- **§5.1 doesn't list `--orphans` on `list`** despite AC 169 referencing it. Add to the table.
- **Undismiss CLI surface is invisible.** T5 is a Phase 1C state-machine transition (§3.2), but the only way to trigger it is through `MemoryStore::delete`. There's no `quorum convention undismiss` or `quorum dismiss --undo` documented anywhere in §5.1. Phase 1B presumably has this somewhere, but v0.1 inherits the gap silently. Mention the existing surface (or the lack of it) explicitly in v0.2's §5.1, even just as a one-liner: "undismiss is exposed via Phase 1B's [existing path]."

### 8.2 TUI keybindings — three issues

- **`x` (delete) is in the status bar but undefined in prose.** §5.2's status bar text reads `H back | p promote | d demote | x delete | / filter | ?  help` but only `p`, `d`, and `H` are described in the prose. What does `x` do? Almost certainly the undismiss path, but it's never spelled out. Either add a "New keybinding `x`" subsection or drop `x` from the status bar.

- **`p` on a `candidate` row has an awkward flow.** AC 160 says "`p` (lowercase) on a `candidate` row rejects with status-bar error." But §5.2 describes `p` as "Opens a single-line modal..." then "On Enter: T2 transition fires (or rejects if state ≠ local_only — error shown in the status bar)." So the modal opens, the user types text, hits Enter, AND THEN gets rejected? Better: reject at keypress time before opening the modal. Status-bar shows "promote requires local_only state (this row is candidate; press Shift+P to force-promote)." AC 160 should pin this.

- **`q` behavior change.** §5.2 says "Exits to main view via `H` (toggle), `Esc`, or `q` (which now quits the TUI from any view)." This is a regression risk for Phase 1B users who muscle-memory `q` to exit the main view. Behavior change is acceptable but call it out as an explicit Phase 1B → 1C UX delta in v0.2.

### 8.3 `quorum link` extension — under-specified

§5.4 mentions `quorum link` is extended to validate the new `[memory]` config keys. Phase 1A's `link` writes `.quorum/config.toml`. Does Phase 1C's `link` write the `[memory]` section with defaults, or only validate it if present? §5.4 says "config.toml is not present by default — `quorum link` writes it" but then "Phase 1C reads it with all-defaults fallback when absent; no warning."

Inconsistent: does `link` write the `[memory]` section or not? My read of intent: `link` writes the section with default values (so the user has an editable starting point); Phase 1C also reads with fallback so config-less repos still work. Both true. Spec wording: "Phase 1C's `quorum link` extension writes the `[memory]` section with default values to `.quorum/config.toml`. Phase 1C's runtime also reads with all-defaults fallback if the section is absent or partial."

---

## 9. Trust model carryover (§8)

§8 is the strongest section of the spec. The carryover is precise; the new rules in §8.3 are minimal; the delimiter discipline is preserved.

### 9.1 What works

- §8.1's restatement of "committed-and-clean" matches CLAUDE.md verbatim. The promotion-without-commit window is acknowledged in §6.2 (modulo the UX-cost framing discussed in §1.5).
- §8.2's delimiter wrap of the new memory subsection is the right call. Local conventions are user-derived but should still be untrusted from Lippa's perspective.
- §8.3's three new rules — no Lippa write, no `note` lift, modifications-detected — are tight.
- §8.3's last rule ("Quorum NEVER edits content outside the fence") is the right boundary.

### 9.2 What I'd tighten

- **`body_snapshot` as placeholder body.** Already covered in §6.6 of this review. Recommend title-only default; require `--text` or `--from-editor` for any body content.

- **The "modifications outside Quorum are detected" rule (§8.3) is correct but the detection trigger is narrow.** Detection only fires on `quorum convention demote/promote` or `convention list --orphans`. It does NOT fire on `quorum review` (the most common entry point). A user who manually edits a managed block won't see a warning until they next touch `convention *`. v0.2 could fire the orphan-detection scan during `quorum review` too — cheap, runs once per review, surfaces drift sooner. Open question rather than recommendation.

- **No mention of `.quorum/conventions.md` size cap.** Conventions.md can grow without bound. The bundle's 10 KB `BUDGET_CONVENTIONS` (`SERVICES.md` §2) handles truncation at bundle time, but is there an upper bound on the file itself (e.g., to prevent a runaway `convention promote` script)? Probably not a real concern — conventions are user-promoted one at a time — but worth a sentence.

### 9.3 No new attack surface from Phase 1C

- `--text` content: user-supplied; trust=user.
- `--from-editor` content: user-supplied via `$EDITOR`; trust=user.
- Title-derived placeholder: `Finding.title` is LLM-generated. Risk = title contains malicious text that the user fails to review before commit. Mitigation: the file is in `.quorum/` and the user commits it explicitly with `git add` + `git commit`. The bundle delimiter wraps the eventual contribution. Acceptable.
- `body_snapshot` placeholder: LLM-generated. Same mitigations + my §6.6 recommendation to drop this default.

No new external network surface, no new keychain reads, no new auth paths. Phase 1C is a pure local-state-machine + filesystem-write milestone. Trust model is correctly minimal.

---

## 10. Recommended changes for v0.2 (prioritized)

In order from "must fix before adjudication" to "nice to have":

### Tier 1 — must fix

1. **Resolve the §3.2 T5 / §4.2 cascade contradiction.** Pick (a), (b), or (c) from §1.1 of this review. Document in §3.2 T5; align CHECK constraints and `to_state`/`trigger` enums in §4.2; add an AC.

2. **Take a position on `--force` against CLAUDE.md.** Either constrain to `recurrence ≥ 2` floor (my lean), remove, or get Rolf to amend CLAUDE.md to explicitly permit user-initiated bypass. Q11 framing needs to be redone against the hard constraint, not against scoping P2.

3. **Resolve the §3.3 timing-paragraph contradiction.** Pick the intended meaning; rewrite both paragraphs to align.

4. **Re-frame §6.2's "small UX cost".** The promoted-but-not-committed window loses ALL committed conventions until commit, not just the new one. Either implement a bridge state, or accept the cost with an explicit strong-warning UX path. Update AC 155 to reflect the chosen behavior.

5. **Pull `lippa_*` columns out of v0.1 (§4.3, §7.2, §7.4).** Defer schema changes to Phase 1D's v2→v3 migration. Keep §7's prose as documentation. Drop AC 147, AC 167.

### Tier 2 — should fix

6. **Spell out the SQLite + filesystem transaction boundary** in §3.2 T2 / T3 / T5. Make the temp-file-rename pattern explicit; document the partial-failure cases.

7. **Make T3 (demote) symmetric to T2 in failure modes.** Currently only T2 has the spelled-out rollback paragraph.

8. **Restrict the auto-derived convention body to title-only.** Drop `body_snapshot` as a placeholder default; require `--text` or `--from-editor` for any body content. (§6.6 of this review.)

9. **Define TUI `x` keybinding** or remove from the status bar. (§8.2.)

10. **Fix `p`-on-candidate flow** — reject at keypress, not after modal. (§8.2.)

11. **AC 144 wording** — replace `tokio::spawn_blocking` with `std::thread::spawn`. (§1.7.)

12. **§5.1 table** — add `--orphans` flag to `convention list` row.

13. **Add missing ACs** for: T2/T3/T5 atomicity, migration failure, `--from-editor`, hook-mode auto-promote suppression, configuration re-validation, `convention show` / `history` output shape. (§4.1 of this review.)

### Tier 3 — nice to have

14. **Close Q9 (migration rollback) — just ship it.** Add `forward_compat_min_version` row to the migration. Add AC. Drop from §10.

15. **Demote Q8 (Lippa wire-format compat) from §10.** It's a 1D recon item, not a 1C question.

16. **Move Q10 (bundle byte budget) from §10 to §11.** Ship the truncation instrumentation in v0.1.

17. **Add the proposed §10 Q12–Q16** for the issues raised above that don't have clean Tier-1 resolutions.

18. **Clarify §3.2 T1's "0 rows changed → skip audit insert" pattern** to prevent the naive implementation from racing the audit insert. (§6.2 of this review.)

19. **Defensive migration: assert `schema_version = 1` before running** rather than silently no-op'ing other versions. (§6.1 of this review.)

20. **Stale-prone "197 tests" count in AC 171.** Replace with "no Phase 1B test fails."

21. **Document the undismiss CLI surface** (or its absence) in §5.1.

22. **`quorum link` behavior on the `[memory]` section** — clarify write-with-defaults vs read-only. (§8.3.)

---

## 11. What v0.1 got right

A short list, because I think it matters to call out:

- **The §5 leans were adopted without silent adjudication on the labeled forks.** The author resisted the temptation to re-litigate scoping decisions in prose; what's left for v0.2 is the unanticipated stuff (`--force`, surface additions), not the main design choices.

- **§2 non-goals are real non-goals.** None of them are "we'll do this later" sleight-of-hand. The Phase 3 fuzzy-hash deferral, the Lippa cloud-sync deferral, the no-LLM-rule-synthesis line — all concrete and load-bearing.

- **§4.4 conventions.md format is well-chosen.** Section-fenced + block-keyed gives genuine git-merge tolerance without inventing a YAML metadata sidecar. The HTML-comment markers are unobtrusive and survive markdown renderers.

- **§4.1 migration discipline matches Phase 1B's** (IF NOT EXISTS, single-transaction, hard-fail on partial). The author internalized the Phase 1B preflight learnings.

- **§8 trust-model carryover is precise.** No new attack surface, no new auth paths, no expansion of the network surface beyond what Phase 1A/1B already shipped.

- **AC numbering continues cleanly from Phase 1B (133 onward, AC 132 having flipped FULL in 0.2.1).** Tracks the spec lifecycle without confusion.

- **§11 "out of scope" distinguishes Phase 1D / Phase 2 / Phase 3 / Phase 1C v2 / "indefinite defer" as separate buckets.** Most specs collapse these into one undifferentiated "future work" section; v0.1 doesn't.

- **The author was honest about §10 Q11 (`--force`) being a scoping-doc deviation.** Surfacing it as an open question (rather than burying it in prose) is the right discipline, even if my position is that the framing needs to be sharper.

The v0.1 → v0.2 path is real but limited. With Tier 1 changes applied, the spec is closing in on implement-ready.

---

*— end peer review (Reviewer A) —*
