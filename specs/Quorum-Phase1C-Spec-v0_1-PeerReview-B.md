# Quorum Phase 1C v0.1 — Peer Review

**Reviewer:** B
**Subject:** `specs/Quorum-Phase1C-Spec-v0_1.md`
**Inputs read:** Spec v0.1, scoping notes, `CLAUDE.md`, `SERVICES.md` (§2 & §6
focused), `HISTORY.md` Phase 1B + 0.2.1 entries.
**Session:** independent; no shared context with other reviewers.
**Disposition:** **Ship with revisions.** The shape of 1C is right and the
§5.1–§5.5 leans were implemented cleanly. There are six specific issues I
would block v0.2 on, and ~a dozen smaller pickups. Detail below.

---

## TL;DR — headline positions

1. **Drop `--force` from v0.1.** It silently contradicts the scoping-notes
   P2 principle ("promotion to local_only requires recurrence — fixed").
   Shipping a backdoor in v0.1 and then asking peer review to bless it is
   backwards. Push it to Phase 1C v2 or remove entirely. (§3.2 T2, Q11.)
2. **Reframe T2's transactional story.** "SQLite transaction wrapped around
   the file-write" is not actually atomic across SQLite + filesystem.
   The spec needs to pin which surface is the source of truth and what an
   observer sees on partial failure. (§3.2 T2.)
3. **Resolve the promote-replace contradiction.** §4.4 says "promote-again
   on the same hash REPLACES the block (idempotent re-promote)"; §3.2 T2
   condition rejects promote unless `state == 'local_only'`. Both cannot
   be true. (§3.2 vs §4.4, plus AC 137 / AC 148.)
4. **Drop the `lippa_*` columns** from the `conventions` table in v0.1.
   They are zero-write forward-compat ballast for a Phase 1D that has no
   timeline yet. The migration cost they save in 1D is identical to the
   migration cost they incur today. (§4.3, §7, AC 147, AC 167.)
5. **Pin the `quorum-core → quorum-cli` stderr path.** §5.3's "callback or
   returned event" is a crate-boundary ambiguity. Returned event is the
   only CLAUDE.md-compliant choice; the "or" needs to go. (§5.3.)
6. **Lock the source-field mapping in §7.2.** CLAUDE.md already forbids
   inventing source values; that pre-adjudicates scoping §5.5 option B
   out of existence. The "TBD per Phase 1D" framing is misleading — the
   answer is "`source: model_proposed`, full stop, semantic stretch
   acknowledged". Phase 1C should not leave Phase 1D the false impression
   that B is open.

---

## 1. Internal consistency

**Block-level (must fix for v0.2):**

- **C1. `--force` contradicts the scoping P2 principle.** Scoping note
  P2 explicitly says: "Promotion to local_only requires recurrence, not
  just one dismissal. The threshold value is the open question (§5.1)
  but the principle is fixed." Spec §3.2 T2 ships `--force` that
  bypasses the threshold and goes candidate→promoted_convention in one
  step. The spec acknowledges this in Q11 and ships it pending peer
  challenge. **My challenge: remove from v0.1.** If a peer review
  consensus emerges that --force is wanted, add it in Phase 1C v2 with a
  fresh adjudication. The current path elevates a power-user nicety to
  the level of revising a "fixed" principle. (Affects: §3.2, §5.1 table,
  AC 138, AC 160, Q11, §11 Phase 1C v2 list.)

- **C2. Promote idempotency contradiction.** §4.4 rule: "Promote-again
  on the same hash REPLACES the block (idempotent re-promote)." §3.2 T2
  condition: "row's current `promotion_state == 'local_only'`. Promoting
  from `candidate` directly is rejected." A row already in
  `promoted_convention` is neither `candidate` nor `local_only` — so by
  T2's condition, re-promote is REJECTED. Both rules cannot be true.
  Three plausible resolutions:
  - (a) Drop the §4.4 idempotent re-promote rule; user must `demote`
    then `promote` to change `--text`. Cleaner.
  - (b) Expand T2's condition to "state IN ('local_only',
    'promoted_convention')" and document the conventions.md block
    overwrite path.
  - (c) Add a `quorum convention update <hash> --text <s>` subcommand
    for the text-update use case; keep T2 strict.
  My lean: **(a)** — keeps the state machine simple. Re-promote with a
  different text is rare; demote-then-promote is two extra keystrokes
  and produces a clean two-row audit trail showing the user's intent
  change. (Affects: §3.2 T2, §4.4, AC 137, AC 148.)

- **C3. T2's "SQLite transaction wrapped around the file-write" is
  not what it claims.** §3.2 T2 says the transaction rolls back on
  IO error from the conventions.md write. SQLite transactions do not
  rollback filesystem operations. The temp-file-rename pattern makes
  the file write effectively atomic against itself, but the
  composition with the SQLite transaction is not atomic across
  surfaces. Three failure windows:
  - File write succeeds, SQLite COMMIT fails: conventions.md has a
    managed block with no SQLite row. AC 169's `--orphans` catches it,
    but only on next `list --orphans`.
  - SQLite COMMIT succeeds, file write fails: SQLite says promoted,
    conventions.md has no block. Next bundle assembly under Phase 1A
    trust model will NOT surface the convention (file not modified
    => committed-and-clean check decides based on committed state,
    not on intended-but-failed state). The `local_only` path also
    won't surface it (state moved past local_only). The rule is
    invisible until the user notices and re-promotes.
  - Either succeeds, process killed mid-rename: temp-file may remain
    on disk; not a correctness issue but a janitorial one.
  v0.2 should pick an ordering (I recommend: write conventions.md
  first via temp-rename; on success, commit SQLite; on SQLite commit
  failure, log the orphan and rely on `list --orphans` for repair) and
  state it explicitly. Also: AC 137 should be split into "happy path"
  and "partial failure" cases with the observable state in each.
  (Affects: §3.2 T2 failure-modes paragraph, AC 137, possibly a new AC.)

**Smaller cross-reference and wording issues:**

- §3.2 T2 step 4 says "appends a managed block (§4.3) within the
  `<!-- quorum:managed-section -->` fence". The block format is in §4.4,
  not §4.3 (which is the `conventions` table). Typo.
- §3.2 T2 `--force` clause: "raises to `promoted_convention` from any
  non-promoted state". Semantically what does `--force` do when state
  is already `local_only`? Per the prose, identical to plain promote
  (the threshold gate is already past). Worth saying so or restricting
  `--force` to candidate-only.
- §3.2 T1 idempotency story conflates two protections. The actual
  serialization comes from SQLite's transaction semantics + the
  `WHERE current_state == 'candidate'` filter (the second writer's
  UPDATE will affect 0 rows and exit before the audit insert).
  `UNIQUE(hash, from_state, to_state, ts)` is a belt-and-suspenders
  guard for the millisecond-collision corner case. The spec presents
  UNIQUE as the primary defense; it isn't. AC 144 conflates this too.
- §3.4's config validation timing ("Out-of-range values produce a CLI
  error on the next `quorum review` or `quorum convention *`") doesn't
  quite match §5.4 ("surfaced at the `link` site AND on the next
  `quorum review`") or AC 164 ("on the first read after config
  change"). Three slightly different framings of the same intent. Pick
  one and apply consistently.
- §4.4 last bullet: the `<!-- quorum-managed-conventions-md v=1 -->`
  first-line marker conflicts with the example layout above it (which
  shows `<existing user content, untouched>` as the first line). Is
  this marker only for Quorum-created files, displaced by user content
  on append? The conditional is unclear. The marker also has no AC
  asserting its presence/absence behavior.
- §5.3 "callback or returned event". Pick one. See C5 in §3 below.

## 2. Coherence with scoping notes (silent adjudications)

The five §5 leans (A / C / B / A / C) are implemented cleanly. But v0.1
prose silently adjudicates several things the scoping notes did not put
on the §5 list:

- **SA1. `--force` carve-out.** Largest silent adjudication. Scoping P2
  is contradicted. Already covered as C1 above. The right place for
  this was a Q in the scoping doc, not a Q in v0.1's §10.

- **SA2. T4 (prune) and T5 (undismiss) trigger taxonomy.** Scoping doc
  contemplated three transitions; v0.1 ships five. T4 (prune) is "any
  state → tombstone" but has NO entry in the `state_transitions.trigger`
  CHECK list — `auto_recurrence | explicit_promote | explicit_demote |
  explicit_undismiss | force_promote`. So prune leaves no audit row.
  Combined with ON DELETE CASCADE removing prior audit rows, prune
  produces a complete forensic blackout for the row. Worth being
  intentional: either (a) accept the blackout and call it out in §4.6
  audit-trail-leak-surface notes (current behavior, just undocumented),
  or (b) write a prune-audit JSONL outside SQLite (re-introduces leak
  surface; not great), or (c) keep a row in `state_transitions` with
  `to_state='deleted'` (already legal per CHECK) and trigger='prune'
  (would need adding to the CHECK list), then break the FK cascade so
  the audit row survives. My lean: **(a)** — accept the blackout but
  state it explicitly. Q2 in §10 partially touches this but only asks
  about archive JSONL; the prune-leaves-no-trace fact deserves its own
  call-out.

- **SA3. `state_transitions.to_state` virtual `'deleted'`.** New design
  choice in v0.1 to capture undismissal in the audit log. Reasonable
  but unannounced. The asymmetry is also a smell: `from_state` is
  restricted to `(candidate, local_only, promoted_convention)` but
  `to_state` includes `'deleted'`. So a row transitioning to deleted
  leaves a row with `from_state='X'` and `to_state='deleted'`. Fine,
  but if FK CASCADE is firing on the dismissal delete, the audit row
  is also cascade-deleted in the same transaction — so the row is
  written and then immediately deleted. Pointless work. Unless the
  intent is that T5 explicitly inserts an audit row BEFORE the cascade
  fires. The spec doesn't pin the ordering. Either (a) drop the
  `'deleted'` virtual state and accept that undismiss is unaudited, or
  (b) insert audit-before-delete in T5 and document the "audit row
  survives the cascade because it was written in the same tx" claim,
  which requires the audit row to live in a different table NOT
  cascaded — i.e. not the current `state_transitions` schema. As
  currently specified, the `'deleted'` virtual state is dead code.

- **SA4. The forward-compat `lippa_*` columns.** Scoping notes do not
  authorize introducing columns in 1C that 1C never reads or writes.
  v0.1 introduces them on the rationale that 1D would otherwise need a
  v2→v3 migration. See §7 below — I'm against keeping these.

- **SA5. Auto-promote runs unconditionally inside `record_seen`, even
  under `--hook-mode=*`.** §5.3 says the stderr note is suppressed
  under hook mode but says nothing about suppressing the transition
  itself. So a `git push` can mutate the user's local SQLite into the
  `local_only` state silently from the user's perspective (they get
  no stderr note because hook mode suppresses it). This is a legitimate
  call but unannounced. Worth promoting to an §10 Q.

## 3. CLAUDE.md compliance

- **Three-crate boundary.** §5.3's "the auto-promote stderr informational
  note (§3.2 T1) fires from `quorum-core::memory::record_seen` via a
  callback or returned event; the CLI prints to stderr." Callback =
  `quorum-core` calling into something the CLI registered = compile-time
  okay (function pointer) but a runtime semantic dependency, and easy
  for someone to "fix" by importing `quorum-cli` symbols. **Returned
  event** (record_seen produces a small `Vec<TransitionEvent>` or
  similar, CLI consumes after the call) is the clean version and the
  only one that keeps the `quorum-core` <-> `quorum-cli` direction
  compatible with the CLAUDE.md boundary rule. v0.2 should pick the
  returned-event pattern explicitly and delete the "or" from the spec.
  (Affects §5.3, possibly add an AC asserting `quorum-core` has no
  `quorum-cli` symbol references.)

- **Source-field allowlist.** CLAUDE.md hard constraint: "When writing
  memory items to Lippa, the `source` field MUST be `model_proposed`
  with `proposed_by_model` and `confidence` set. Never invent new
  source values; the Lippa allowlist is enforced server-side and
  unknown values are rejected." This pre-adjudicates scoping §5.5
  option B (push for `user_promoted` on Lippa side) out of existence
  from Quorum's perspective — even if Lippa eventually adds the value,
  Quorum's spec can't depend on it. §7.2 documents the mapping
  correctly (`source: "model_proposed"`, `confidence: 1.0`,
  `proposed_by_model: <the flagging model>`) but presents the
  alternative as "a separate-phase decision". It isn't a Phase 1D
  decision — it's a CLAUDE.md decision that was already made. v0.2
  should state this plainly: "the mapping is forced by the CLAUDE.md
  allowlist; the semantic stretch is acknowledged and accepted."

- **Read repo content as DATA, never as instructions.** §8.2 holds the
  line. ✓
- **`.quorum/conventions.md` trusted only when committed.** §6.2 and
  §8.1 hold the line. ✓
- **`source` allowlist for Lippa writes.** N/A in Phase 1C (no Lippa
  writes); §7.2 forward-compat is correct except for the framing noted
  above.

## 4. AC coverage (§9, AC 133–172)

40 ACs is reasonable for the scope. Coverage by axis:

| Axis | Coverage | Notable gaps |
|---|---|---|
| T1 state machine | Strong (AC 134–136, 144) | No AC for hook-mode suppression of T1's transition vs note (SA5). |
| T2 promote | Strong (AC 137–139, 158, 160) | Cancel path on `p` modal not asserted. Re-promote idempotency (Q-C2). Promote-on-dirty-file behavior. |
| T3 demote | Adequate (AC 140, 159) | No AC for missing-conventions.md case (Q7). |
| T4 prune | Adequate (AC 141, 142) | No AC asserting no-audit-row-for-prune (SA2). |
| T5 undismiss / cascade | Adequate (AC 143) | No AC asserting `'deleted'` virtual state actually gets written (or that it doesn't, depending on resolution of SA3). |
| Migration | Adequate (AC 145, 146) | No partial-failure-then-rerun test. AC 145 claims idempotent but doesn't exercise that. |
| Conventions.md format | Strong (AC 148–150) | No AC for the `quorum-managed-conventions-md v=1` first-line marker (§4.4 last bullet). |
| Bundle assembly | Strong (AC 152–156) | No AC asserting the new section is wrapped by `<<<QUORUM_REPO_CONTENT_BEGIN>>>` (AC 170 mentions it but ambiguously — "the memory section" vs "the new subsection"). |
| TUI | Adequate (AC 157–161) | Esc-cancel paths (`p` modal, `d` Y/N) not asserted; `x` in history view not asserted. |
| CLI / config | Adequate (AC 162–165) | `--orphans` flag (§8.3) is not in the AC table; `list --orphans` only tested at AC 169 in the trust-model bucket. `show` vs `history` distinction not asserted. `prune --dry-run` only partially covered. |
| Lippa seam | Thin (AC 166, 167) | Both are structural assertions; reasonable for a no-network milestone. |
| Trust model | Adequate (AC 168–170) | AC 168's "operation proceeds" semantics not fully tested (what if the section fence is missing entirely?). |
| Regression | Present (AC 171, 172) | ✓ |

**Specific AC fixes:**

- **AC 137** should split into happy-path and partial-failure cases.
  Currently asserts the end state without addressing the IO-error
  rollback claim from §3.2 T2.
- **AC 144** test description: "two `tokio::spawn_blocking` workers
  writing the same hash". The protection mechanism in SQLite is the
  serialization of writers in WAL mode + the WHERE-clause-zero-rows
  case. The test is asserting SQLite's behavior. v0.2 should make
  explicit what's being tested (and note that it's not exercising
  Quorum logic so much as the SQL-level idempotency claim).
- **AC 147** asserts the columns are nullable and never non-NULL-written
  in 1C. **Drop entirely if §7 ballast columns are dropped per Issue
  C4 below.** Otherwise good.
- **AC 170** rewrite for clarity: "The `<<<QUORUM_REPO_CONTENT_BEGIN>>>`
  delimiter wraps the entire memory section, including the new
  `## Local conventions (auto-derived)` subsection. No Quorum-side code
  path emits the delimiter inside the local-conventions text."
- **AC 161** ("TUI exit leaves terminal restored... A panic during T2's
  file write also restores"). This is hard to unit-test without a PTY
  harness. v0.2 should say "verified manually + via panic-injection
  unit test against the restoration RAII guard, not via PTY".

**Missing ACs worth adding:**

- AC-α: T1 transition under `--hook-mode=*` — does it fire or skip?
- AC-β: T1 stderr note suppressed under `--hook-mode=*` (already in
  §5.3 prose; should be tested).
- AC-γ: Promote-on-dirty-conventions.md behavior (warn? refuse?
  overwrite? See Q15 proposal below).
- AC-δ: Migration partial-failure idempotency (run migration, kill mid-
  transaction, re-run, assert clean state).
- AC-ε: Esc cancels the `p` modal in TUI with no SQLite write.
- AC-ζ: Auto-promote inside `quorum review` includes the new
  `local_only` rule in the SAME review's bundle when fired
  in-record_seen-pre-bundle-assembly. §3.3 makes a positive claim
  about this; it should be tested.

  Wait — re-reading §3.3: "the transition therefore happens BEFORE the
  bundle is sent to Lippa. A newly-promoted `local_only` entry is
  included in the SAME review's memory context that triggered its
  promotion." Then immediately below: "The auto-promote does NOT
  re-run `quorum review` to apply the new local rule to the current
  findings; the bundle is built once. The local convention takes
  effect on the NEXT review." These two statements appear to
  contradict. Which is it? The first says the same review; the
  second says the next review. I think the resolution is: the rule
  is INCLUDED in the bundle's memory section (because record_seen
  fires before bundle assembly finalizes), but the Lippa-side
  Consensus run does NOT apply the rule to the findings it's
  about to surface (because the findings were already generated
  before the review started). That's a subtle distinction worth
  spelling out explicitly. Right now the spec reads contradictory
  on a first pass.

## 5. Open-question quality (§10)

Q1–Q11 are mostly genuine. My takes:

- **Q1 (promote-conflict-with-merge):** genuine. Lean is reasonable.
  Easy v0.2 add.
- **Q2 (audit log retention on prune):** genuine but partially overlaps
  with SA2 above. Resolution should be combined.
- **Q3 (promote inside a hook):** genuine. Lean correct (no).
- **Q4 (per-source-type threshold):** genuine. Lean reasonable; my
  preference is v0.1 keeps single int, but instrument something so
  Phase 1C v2 can data-inform the decision (counts per source.type of
  rows-at-threshold-N-1).
- **Q5 (sync-check shape):** **not a genuine open question.** v0.1
  ships `--orphans` flag only; the standalone `sync-check` is a Phase
  1C v2 item. Move to §11.
- **Q6 (auto-promote stderr verbosity):** genuine. Lean (one line per
  transition) is correct for v0.1.
- **Q7 (demote when conventions.md missing):** genuine. Lean
  (no-op-with-warning) reasonable. **Should be promoted to an AC**,
  not left as Q. Cheap to test.
- **Q8 (local convention rendering vs Lippa wire format):** **not a
  genuine 1C question.** Can only be answered with Lippa-side recon
  during Phase 1D. Move to §11 / Phase 1D scoping prep.
- **Q9 (migration rollback path / `forward_compat_min_version`):** the
  lean says "yes; cheap to add". Then ship it in v0.2. Don't defer a
  cheap-and-yes item to a v0.3 it doesn't have. Either include in
  v0.2 or explain why it isn't trivial.
- **Q10 (bundle byte budget revisit):** genuine. Lean (instrument
  truncation rates) is the right call.
- **Q11 (--force / Shift+P):** covered as C1 above. **My position:
  remove from v0.1, not "v0.2 picks (a)/(b)/(c)".** The decision is
  whether scoping P2 stands. If yes, --force goes. If no, scoping
  needs revisiting and we're in a different milestone.

### Q12+ I would add

- **Q12 — Atomicity boundary between SQLite and conventions.md.**
  T2 writes both. The current "transaction wrapped around file write"
  framing isn't accurate. v0.2 should pick: (a) file-first, SQLite-
  second, log orphan on SQLite-commit-failure; or (b) SQLite-first,
  file-second, accept invisible-promotion-window on file-write-failure.
  Mine: (a). (Covered as C3 above; flagging here for §10 inclusion.)
- **Q13 — Re-dismiss after promotion.** Phase 1B's `dismiss()` is
  INSERT-only; returns `AlreadyDismissed` on duplicates. After
  promotion the dismissal row exists. Does a new `dismiss()` attempt
  on the same hash return AlreadyDismissed? Probably yes. What if the
  user demoted first? Still AlreadyDismissed (the dismissal row was
  never deleted). What about `record_seen` post-promotion — does it
  bump `recurrence_count` on a `promoted_convention` row? Spec is
  silent. My lean: yes, bump (the counter is observability-only at
  that point, useful for "this rule is still catching things") and
  do NOT fire T1 again. Worth pinning.
- **Q14 — Auto-promote under `--hook-mode=*`.** Per SA5: does T1 fire
  during a pre-push hook? Spec is ambiguous. My lean: fire, because
  the state is local-only and the hook fail-open contract is about
  network errors, not about local SQLite writes. But the user gets
  no stderr note (per §5.3 suppression), so the transition is
  invisible at the moment it happens. Worth a §10 entry.
- **Q15 — Promote when conventions.md has uncommitted user edits.**
  T2 modifies the working-tree file via temp-rename. If the user has
  uncommitted edits to conventions.md (outside the managed section),
  the temp-rename preserves them (the rename writes the whole file
  with the user's content intact). But: what if the user has
  uncommitted edits INSIDE the managed section? Or what if they've
  edited the same managed block we're about to replace? Spec doesn't
  address. My lean: warn-but-proceed if any managed-block bytes
  differ from what SQLite expects; refuse if the section fence is
  missing or malformed. Worth a §10 entry.
- **Q16 — Update-text without demote-repromote.** Per C2 above; if
  resolution (a) is taken (require demote-repromote), this is a
  closed question; if (b) or (c), it's an open one.

## 6. Data model rigor

**Migration:**

- §4.1 single-transaction migration with `IF NOT EXISTS` guards. ✓
- `UPDATE schema_version SET version = 2 WHERE version = 1` as the
  last step. ✓
- No backfill of audit for pre-v2 rows. Documented and tested. ✓
- Forward-incompatibility (a v1.0 1B binary opening a v2 DB) is
  flagged in Q9; ship in v0.2 per the lean.

**`state_transitions` table:**

- `BLOB NOT NULL REFERENCES dismissals(finding_identity_hash) ON DELETE
  CASCADE` — fine; the `BLOB` matches the Phase 1B SHA-256 storage.
- `UNIQUE(hash, from_state, to_state, ts)` — see SA1 commentary; the
  spec overstates its protection. Keep the constraint, but reframe.
- Index on `(finding_identity_hash, ts)`. ✓ Covers `convention history
  <hash>` and `convention show <hash>` queries.
- **No index on `to_state` or `trigger`.** Probably fine; analytical
  queries (e.g. "how many auto-promotes this month?") are not on the
  Phase 1C path. Worth noting in §11 for instrumentation work.

**`conventions` table:**

- `finding_identity_hash BLOB PRIMARY KEY REFERENCES
  dismissals(finding_identity_hash) ON DELETE CASCADE`. ✓
- `convention_text TEXT NOT NULL CHECK (length(convention_text)
  BETWEEN 1 AND 4096)`. The 4096 cap is 8x the 500-byte bundle cap;
  rationale (`BUDGET_CONVENTIONS` = 10 KB so larger blocks are valid
  in conventions.md) is reasonable. ✓
- `promoted_by_user TEXT` — git config user.email. **Privacy concern:**
  §4.6 audit-trail-leak-surface analysis covers `note` and
  `body_snapshot` but doesn't mention `promoted_by_user`. The
  dismissals SQLite is gitignored, but if it leaks (debug bundle,
  inadvertent copy), git emails are tied to specific promotions. My
  lean: either omit the column (the conventions.md commit attribution
  is the canonical record anyway — git blame on conventions.md tells
  you who promoted), or extend §4.6 to acknowledge it. v0.2 question:
  **is `promoted_by_user` worth keeping at all** given that the
  committed conventions.md commit already carries the email via git
  blame? My lean: drop it.
- `conventions_md_block_id TEXT NOT NULL` derived as "first 12 chars of
  finding_identity_hash". **This is just a deterministic prefix of the
  PK.** Storing it is redundant unless it can ever diverge — and the
  spec doesn't allow that. Drop the column; compute on read. The
  `UNIQUE (conventions_md_block_id)` constraint becomes vacuous when
  the PK is the source of truth. Drop both.
- `lippa_memory_item_id TEXT`, `lippa_proposed_at INTEGER` — see §7
  below. **Drop these from v0.1.**

**§4.4 conventions.md format:**

- Section-fenced, block-keyed by 12-char hex prefix. ✓
- `v=1` on both the section and each block. ✓ (forward-compat for
  Phase 2+ format migrations).
- Line-ending preservation. ✓ (tested via AC 150).
- The `quorum-managed-conventions-md v=1` first-line marker — see
  the §1 list above. Conflicts with example; semantics underspecified.

**§4.5 merge-conflict discipline:**

- Different `id=` blocks don't conflict structurally. ✓ Reasonable.
- Same `id=` with different bodies = manual semantic resolution. ✓
- No auto-commit on promote. ✓ (preserves user agency over the
  commit graph; matches CLAUDE.md spirit).

**§4.6 audit-trail leak surface:**

- `state_transitions` carries hash + ts + trigger + recurrence. No
  titles, no notes, no bodies. ✓
- `conventions.convention_text` IS readable (intentional — it lands in
  conventions.md, which is committed). ✓
- Phase 1B `note` never auto-copied into `convention_text`. ✓
- **Gap:** `promoted_by_user` not analyzed. (See above.)

## 7. Forward-compat discipline (§7)

The most contestable part of v0.1. My position:

**Drop the `lippa_*` columns from v0.1. Keep §7 as architectural prose.**

Reasoning:

- The columns save Phase 1D from needing a `v2→v3` migration that adds
  two nullable columns. That migration is ~10 lines of Rust and one AC.
- The columns cost Phase 1C: two columns, AC 167 (a test that asserts
  a non-feature works), §7.2 prose that locks in semantics that may
  drift before 1D actually ships.
- Worse, the columns sit in v1.0 SQLite forever if 1D is reframed or
  delayed. If 1D ends up using a different shape (e.g. an outbox
  table referencing convention hashes rather than columns on the
  conventions row), the columns are dead weight permanently.
- The CLAUDE.md "cloud writes only on explicit promote OR repeated
  dismissals OR admin approval" hint does NOT mandate the
  outbox-on-conventions design. Multiple valid shapes exist.

What §7 should keep:
- §7.1 endpoint schema documentation (read-only orientation; useful
  for Phase 1D scoping).
- §7.2 source-field mapping (per CLAUDE.md compliance discussion above).
- §7.3 explicit non-goals.

What §7 should drop:
- §7.2 paragraph 2 ("Phase 1C bakes this into the schema").
- §7.4 forward-compat seam test.
- AC 167.
- `lippa_*` columns in §4.3.

**If you keep the columns**, then at minimum:
- §7.2's "1D will truncate or refuse at 500 — TBD per Phase 1D spec"
  needs resolution NOW. Either Phase 1C truncates at write time (and
  records the full text in `convention_text` for the user-facing file
  while presenting the truncated version to Lippa later), or Phase 1C
  enforces ≤500 directly on `convention_text` (changes the CHECK from
  4096 to 500, which contradicts the §4.3 rationale that 4096 is
  intentional for the conventions.md file). Punting this to 1D
  guarantees the columns will be wrong-shaped when 1D lands.

## 8. UX coherence

**CLI (§5.1):**

- `quorum convention promote/demote/list/show/prune/history`. Six
  subcommands; reasonable surface.
- **`list --orphans` is missing from the §5.1 table** but referenced in
  §8.3 and AC 169. Add to the table.
- **`show` vs `history` overlap.** Spec is ambiguous on what `show`
  prints beyond "transition history". My read: `show` is a per-row
  summary (current state + body + recent 5 transitions), `history` is
  the full audit. Should be pinned.
- **No `--dry-run` on `promote` / `demote`.** `prune` has one. For
  `demote`, which mutates the committed conventions.md (well, the
  working tree of it), a `--dry-run` showing the diff would be
  user-friendly. v0.2 question, not a blocker.
- **No `--text` update path post-promotion.** See C2 (re-promote
  contradiction). Resolution depends.

**TUI (§5.2):**

- `H` toggles dismissal-history view from main list. ✓
- `p` (lowercase) = promote local_only.
- `Shift+P` = force-promote candidate. **Goes away if --force is
  removed per C1.**
- `d` = demote promoted_convention. **Conflict:** Phase 1B's main list
  view uses `d` for dismiss. Same key, different semantics across
  views. Acceptable if the status bar makes it obvious which view
  you're in, but a `D` (capital) for demote symmetric with `Shift+P`
  for promote would be cleaner.
- `x delete` — Phase 1B undismiss. In the history view, if the row is
  in `promoted_convention`, `x` cascades through to conventions.md
  block removal (AC 143). That's a heavy effect for one keystroke. A
  confirmation modal `Y/N` (like `d` has) would be appropriate.
  v0.2 question.
- "Shift+P" wording: ratatui-side this is just an uppercase `P` event;
  the modifier-key phrasing implies modifier tracking which is
  inconsistent across terminals. Recommend phrasing as "`P` (uppercase)"
  for clarity.

**Bundle (§6):**

- Per-entry cap default 500 bytes. ✓
- Sort by `recurrence_count DESC, last_seen_at DESC`. Reasonable.
  Most-noisy-rules-first is the right value for the bundle.
- The §6.2 invisible-promoted-but-uncommitted window is a real UX
  cost. Spec accepts it. I'd flag for end-user docs ("after
  `quorum convention promote`, commit conventions.md to apply"); the
  stderr reminder at promote-time helps.

**Config (§3.4 / §5.4):**

- Three keys, all bounded, all default-when-absent. ✓
- Out-of-range exits 2. ✓
- Validation timing inconsistency noted above.

## 9. Trust model carryover

§8 is mostly correct.

- **Carryover from Phase 1A** (committed-and-clean for conventions.md):
  preserved. ✓
- **Carryover from Phase 1B** (`<<<QUORUM_REPO_CONTENT_BEGIN>>>`
  wrapping repo content as data): preserved; wraps the new local
  conventions subsection per AC 170.
- **New rules:**
  - No Lippa writes in 1C. ✓
  - `note` never auto-copied. ✓
  - Out-of-sync detection on read. Adequate but lazy — the bundle
    can ship a malformed managed-block-section through before the
    next promote/demote/`list --orphans` detects it. The malformed
    block is wrapped in delimiters and treated as data by Lippa, so
    no instruction-injection surface, but the rule the user thought
    they had isn't there. v0.2 should consider adding a parse-and-
    validate pass at bundle-assembly time for the conventions section
    (cheap, runs in `quorum review` anyway).
  - **`promoted_by_user` not analyzed** in §4.6. See data-model
    discussion above.

**No new attack surface I can identify.** The local-only entry text
comes from the user's own typed `note` and the dismissal's
`body_snapshot` (which itself was the bundled-to-Lippa finding body
text). Both are already in the trust pipeline.

**One small concern:** the §4.4 `quorum-managed-conventions-md v=1`
first-line marker is presented as a detection signal but isn't
authoritative. A user (or a malicious teammate) who removes the marker
keeps the section fence and gets the section parsed as if untouched.
Detection of "modified outside Quorum" then misses cases where the
fence is intact but content inside it has been edited. The
section-content hash would be the rigorous signal; a single marker
isn't enough. v0.2 question.

## 10. Recommended v0.2 actions (prioritized)

**Blockers (must address):**

1. Remove `--force` / `Shift+P` from v0.1 OR formally revise the
   scoping P2 principle. Currently a silent adjudication that
   contradicts a "fixed" principle. **(C1.)**
2. Resolve the re-promote contradiction (§4.4 vs §3.2 T2 condition).
   My lean: drop §4.4's idempotent re-promote rule; require demote
   first. **(C2.)**
3. Rewrite §3.2 T2 failure-modes to pin the SQLite/file ordering and
   the observable state on each partial-failure window. Add a
   partial-failure AC. **(C3.)**
4. Decide on the `lippa_*` columns. My lean: drop. **(§7.)**
5. Pin §5.3 "callback or returned event" to returned event.
   Add an AC asserting `quorum-core` has no `quorum-cli` symbol
   imports. **(CLAUDE.md crate-boundary.)**
6. Restate §7.2 source-field mapping as forced-by-CLAUDE.md, not as
   a Phase 1D open question.

**Strong recommendations:**

7. Resolve §3.3's same-review-vs-next-review contradiction about
   when an auto-promoted local rule first takes effect.
8. Add ACs for: T1 under `--hook-mode=*`, missing-conventions.md at
   demote, partial-failure-migration, `p` modal Esc-cancel.
9. Drop `conventions_md_block_id` and `UNIQUE` constraint on it
   (redundant with PK).
10. Drop `promoted_by_user` from `conventions` schema (privacy
    concern; git blame on the committed file is the canonical record).
11. Add new §10 Qs: Q12 (atomicity), Q13 (re-dismiss post-promotion),
    Q14 (auto-promote under hook mode), Q15 (promote-on-dirty-file).
12. Reclassify §10 Q5 and Q8 to §11 (they're deferrals, not
    questions).
13. Ship Q9 in v0.2 (the lean is "yes, cheap" — do it).
14. §5.1 table: add `list --orphans`; clarify `show` vs `history`.
15. Fix the §3.2 T2 cross-reference to §4.4 (currently says §4.3).
16. Reframe §3.2 T1's idempotency story (WHERE clause is the primary
    protection; UNIQUE is the secondary guard).
17. Resolve `state_transitions.to_state='deleted'` semantics (SA3).
    Either drop the value or pin ordering against CASCADE.

**Smaller pickups:**

18. Pin the §4.4 `quorum-managed-conventions-md` first-line marker
    semantics, or drop it in favor of section-content hashing.
19. Pin config-validation timing across §3.4 / §5.4 / AC 164 to one
    consistent framing.
20. Add `D` (capital) for demote in the TUI to avoid the `d` overlap
    with main-list dismiss.
21. Consider `--dry-run` on `demote` (mutates committed file).
22. §6.1 bundle-section sort order: confirm tie-breaking is stable
    (`hash ASC` after `last_seen_at DESC`?).

---

## Closing

The 1C scope as drafted is the right scope: state machine + local rules
+ conventions.md write-back, no cloud. The shape of the spec is solid;
most of my notes above are about tightening rather than redesign. The
two issues I'd hold v0.2 on are **--force** and **the SQLite/file
atomicity story**. Everything else is editable in a single pass.

The spec also reads as one that's been written confidently, which I
mean as praise — it's easier to find precise issues in a precise
document. Phase 1B's spec set this bar; v0.1 of 1C clears it.

*— end peer review B —*
