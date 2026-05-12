# Quorum Phase 1C — Scoping notes

**Status:** scoping pass, pre-v0.1.
**Goal of this doc:** lock down what's already settled, take initial
positions on the easy calls, surface the real open questions for Rolf,
and set up the spec-lifecycle path to v1.0.
**Drafted by:** Claude chat session, May 2026.

---

## 1. Mission

Phase 1C delivers the conventions-promotion state machine — the next
layer on top of Phase 1B's dismissals foundation. Dismissals are local
signals; conventions are durable rules. Phase 1C is the path between
them.

Phase 1B shipped the *signal* layer (one-finding-at-a-time dismissals
with reasons and identity hashes). Phase 1C ships the *pattern* layer:
how recurring dismissals become recognized local rules, and how local
rules get promoted to committed project conventions.

---

## 2. What's already settled (do not re-litigate)

From CLAUDE.md hard constraints, Phase 1B shipped capabilities, and
the Phase 1B preflight recon:

- **State machine spine.** `candidate → local_only → promoted_convention`.
  Three states, two transitions. Names are fixed (already documented
  in Phase 1B TUI and HISTORY.md).
- **Cloud-write triggers** (per CLAUDE.md hard constraint):
  "explicit promote OR repeated dismissals OR admin approval." Three
  legitimate paths to a cloud write. Never one-dismissal-direct-to-cloud.
- **Source allowlist** (per CLAUDE.md). Writes to Lippa memory MUST
  use `source: model_proposed` with `proposed_by_model` + `confidence`
  set. Allowlist is server-enforced; unknown values rejected. This is
  a constraint, not a choice.
- **Trust model carryover.** `.quorum/conventions.md` is trusted only
  when committed AND byte-identical to HEAD. Uncommitted changes are
  ignored. Phase 1A behavior holds.
- **Lippa endpoint.** `POST /api/v1/memory/propose` exists with
  documented schema (Phase 1B preflight recon-1 confirmed). Phase 1C
  consumes it; no Lippa-side work in scope.
- **Identity hash.** Phase 1B's three-input hash (title + source.type
  + sorted models) is the matching primitive. Phase 1C does NOT
  redefine it.
- **SQLite carryover.** `.quorum/dismissals.sqlite` is the existing
  local store. Phase 1C extends the schema (probably adds a `state`
  column + a `conventions` table), does not introduce a parallel
  database.

---

## 3. The state machine

```
[no entry]
     │
     │ dismissal recorded (Phase 1B behavior, unchanged)
     ▼
[candidate]                  - local SQLite row
     │                       - not surfaced in bundle
     │                       - one entry per identity_hash
     │
     │ recurrence threshold met
     │ (OPEN — §5.1)
     ▼
[local_only]                 - applied to local reviews
     │                       - feeds bundle memory context
     │                       - never sent to Lippa
     │
     │ explicit user promote
     │ OR repeated dismissals past threshold
     │ OR admin approval
     │ (OPEN — §5.2)
     ▼
[promoted_convention]        - written to .quorum/conventions.md
     │                       - tracked by state machine
     │                       - eligible for cloud sync
     │
     │ POST /api/v1/memory/propose
     │ (OPEN — §5.5, semantically loaded)
     ▼
[Lippa cloud memory item]    - visibility model is Lippa's concern
```

---

## 4. Initial positions (taken — challenge in peer review if needed)

These are the positions v0.1 will commit to unless §5 adjudication
changes them. They're the "obvious" calls.

- **P1. Candidate state is implicit.** Every dismissal creates a
  candidate row automatically. No user action required. The Phase 1B
  dismissals SQLite already serves this; Phase 1C adds a `state`
  column with default `candidate`. No new gesture for the user.

- **P2. Promotion to local_only requires recurrence, not just one
  dismissal.** A single dismissal is information, not a rule. The
  threshold value is the open question (§5.1) but the principle is
  fixed: one dismissal never makes a local_only rule.

- **P3. Promotion to promoted_convention requires explicit user
  action.** The "repeated dismissals" trigger from CLAUDE.md is the
  candidate-to-local_only path, NOT the local_only-to-promoted path.
  Cloud writes (when they happen — see §5.5) are always intentional.

- **P4. local_only entries flow into the bundle as memory context.**
  They share the §4.3.2 memory budget with CLAUDE.md / AGENTS.md /
  .cursorrules. Phase 1C does NOT introduce a new budget section;
  the 200 KB total stays.

- **P5. promoted_convention writes back to `.quorum/conventions.md`.**
  Single source of truth for committed conventions; the state machine
  doesn't introduce a parallel display surface. The state machine
  tracks transitions and history; the committed file is the result.
  This preserves the Phase 1A trust model: the file in the bundle is
  the file in the repo at HEAD.

---

## 5. Open questions (Rolf input needed before v0.1 drafting)

### 5.1 Recurrence threshold (candidate → local_only)

The trigger for moving a candidate to local_only.

**Options:**
- **A. Exact-hash recurrence.** N dismissals of the same identity_hash.
  Simple, deterministic. Risk: hash is title-sensitive, so semantically
  identical findings under different titles count separately.
- **B. Fuzzy recurrence (embedding-based).** Dismissals across
  identity hashes that match by embedding distance. Powerful but
  introduces a vector dependency — that's Phase 3 territory
  (sqlite-vec).
- **C. Hybrid.** Exact-hash counts double; near-hash (same
  source.type, same sorted_models, similar title via cheap
  edit-distance like normalized Levenshtein) counts single. No
  embedding dependency.

**Threshold value:** 2? 3? 5? Configurable?

**Time window:** all-time, or trailing 90/180 days?

**My lean:** **A with N=3, all-time, configurable via
`.quorum/config.toml`** (`[memory] candidate_threshold = 3`).
Defers embedding work to Phase 3; threshold high enough to filter
noise, low enough to surface real patterns; all-time is simpler
than time-window and the user can prune manually.

**Why not C:** edit-distance feels like an arbitrary middle ground.
Either commit to exact (cheap, brittle) or go full semantic (Phase 3
work). Hybrid adds code complexity for marginal accuracy gain.

### 5.2 Promotion mechanism (local_only → promoted_convention)

How the user explicitly promotes.

**Options:**
- **A. CLI only.** `quorum convention promote <hash>` or
  `quorum convention promote --interactive`. Scriptable, scriptable,
  no TUI work.
- **B. TUI only.** From a new dismissal-history view inside the TUI,
  press `p` to promote. In-flow, discoverable.
- **C. Both.** CLI for scriptability + CI; TUI for in-flow promotion.
- **D. Surface as a side-effect of `quorum review`.** When a
  local_only entry is matched again, prompt "promote to convention?"
  y/n. Friction inside a hook (which conflicts with fail-open
  discipline if user input blocks).

**My lean:** **C.** CLI as the primary scriptable surface;
TUI as the discoverable one. **D is deferred** — interactive prompts
mid-`review` break hook fail-open. The TUI history view is new
work, ~Phase 1B-Stage-2 scale, but reuses the Phase 1B ratatui
foundation.

### 5.3 Expiry policy

How long candidates / local_only entries persist without re-trigger.

**Options:**
- **A. No expiry.** Manual cleanup via `quorum convention prune
  [--state candidate]`.
- **B. Asymmetric TTL.** Candidates expire after N days without
  re-dismissal; local_only never expires (it's already "real").
- **C. Symmetric TTL.** Both expire on different schedules. Phase 1B's
  per-dismissal `--no-expire` flag (already shipped) maps onto this:
  365-day default for dismissals already exists.

**My lean:** **B with N=90 days for candidates.** local_only and
promoted_convention never auto-expire (manual prune or explicit
demote). Candidates are noisy by design — auto-cleanup keeps the
SQLite from ballooning. Phase 1B's `--no-expire` translates to
"this candidate is permanent" cleanly.

### 5.4 Bundle assembly — where do local_only entries live?

Phase 1A's bundle has a 20 KB memory context section. local_only
entries need to go somewhere.

**Options:**
- **A. Concatenate into the existing memory section** after
  CLAUDE.md/AGENTS.md, with a `## Local conventions (auto-derived)`
  header. Shares the 20 KB budget.
- **B. New bundle section with its own budget.** Bumps the 200 KB
  total or steals from another section.
- **C. Inline via Lippa-side memory query at session-create time.**
  Quorum sends entry IDs; Lippa fetches bodies. Lippa-side work,
  out of scope.

**My lean:** **A.** No new budget allocation, no Lippa-side
dependency, fits the existing trust model (memory context is data,
never instructions — Phase 1A's `<<<QUORUM_REPO_CONTENT_BEGIN>>>`
delimiters apply).

**Side effect to flag:** §4.3.2 memory budget gets tighter for users
with many local_only entries. v0.1 spec needs a per-entry size cap
(suggest 500 bytes max per local_only entry, configurable) so a few
verbose dismissals don't squeeze out CLAUDE.md.

### 5.5 The source-field allowlist — the consequential fork

CLAUDE.md says writes to Lippa must use `source: model_proposed` with
`proposed_by_model` + `confidence` set. But human-promoted conventions
aren't model-proposed.

**Options:**

- **A. Stretch `model_proposed` semantics.** Use `source:
  model_proposed`, `proposed_by_model: "<the model that flagged
  the underlying finding>"`, `confidence: 1.0` to mark "user-affirmed."
  Stays within the existing allowlist; cloud sync works in Phase 1C.
  Risk: future readers of Lippa memory may treat `model_proposed`
  with confidence=1.0 as a special case; the convention semantic is
  fudged but recoverable.

- **B. Push for Lippa-side `source: user_promoted` allowlist
  addition.** Cleaner semantics; cloud sync works after Lippa
  milestone X. Blocks Phase 1C on Lippa-side work — adds a Lippa-side
  spec to the dependency tree.

- **C. Defer cloud sync entirely to a future phase.** Phase 1C
  ships the state machine and local rules; the
  promoted_convention → Lippa write is the *next* phase's
  scope. `/api/v1/memory/propose` stays unused but its forward-compat
  seam (the source-field shape) is preserved. local_only and
  promoted_convention both stay local; promoted_convention's only
  Phase-1C-visible effect is writing to `.quorum/conventions.md`
  and getting tracked in SQLite history.

**My lean:** **C.** Cleanest split:
- 1C is the state machine + local rules + the `.quorum/conventions.md`
  write-back.
- 1D (or whatever) is cloud sync + Lippa-side spec + `/memory/propose`
  consumption.
- Phase 1C ships faster, smaller surface, smaller blast radius.
- Lippa cloud sync was a "directionally important" capability but not
  the user-facing value of Phase 1C. The user-facing value is "the
  same dismissed thing stops bothering me, and I can promote it
  into a real rule." That ships under C without cloud.

**This is the fork most worth challenging.** Going with A gets cloud
sync now at a small semantic cost. Going with B is the principled
move if the Lippa-side spec is fast. Going with C is the smallest,
fastest 1C — but defers a CLAUDE.md-hinted capability ("cloud writes
only on...") to a later phase.

---

## 6. Out of scope (Phase 1C does NOT cover)

- Embedding-based fuzzy hash matching (Phase 3 — sqlite-vec).
- Cross-repo convention sharing (Phase 2+ if at all).
- LLM-generated rule synthesis ("here's a draft convention from these
  dismissals" — interesting but a separate spec).
- Convention versioning / history beyond the SQLite state log.
- Per-team or per-org convention scoping (Lippa-side concern).
- Cloud sync conflict resolution (deferred per §5.5 lean).
- TUI dismissal-history view UX details (drafted in v0.1, expanded
  pre-impl).

---

## 7. Spec lifecycle plan

1. **This session.** Scoping notes (this doc) → Rolf adjudication
   on §5.1-§5.5 → my v0.1 spec draft.
2. **CC v0.1 drafting session.** Takes the adjudicated scoping notes
   + writes `specs/Quorum-Phase1C-Spec-v0_1.md` with full §1-§9
   detail.
3. **Peer review sessions.** 2–3 separate Claude instances (fresh
   chats, no shared context). Each produces an independent review
   document.
4. **Adjudication v2.** Rolf reviews divergences in chat with me;
   we decide.
5. **v0.2.** Drafted with peer-review feedback applied.
6. **Re-review.** Lighter pass on v0.2.
7. **v1.0 commit.** Spec committed to `specs/`. Phase 1C
   implementation can begin via a separate CC session.
8. **SERVICES.md.** Phase 1C will add a new §-numbered section.
   Drafted alongside v0.2.

---

## 8. Acceptance criteria sketch (for v1.0 to define rigorously)

Direction-setting only; v1.0 will tighten and number these:

- AC: Phase 1B dismissals behavior preserved (regression).
- AC: A candidate row is created on every dismissal automatically.
- AC: Hitting the §5.1 recurrence threshold transitions a candidate
  to local_only without user action; the transition is logged.
- AC: `quorum convention promote <hash>` writes a rule to
  `.quorum/conventions.md` and updates the SQLite state to
  `promoted_convention`.
- AC: `quorum convention list [--state X]` shows entries with counts
  per state.
- AC: `quorum convention demote <hash>` and `quorum convention
  prune` are available for cleanup.
- AC: `.quorum/conventions.md` modifications outside Quorum are
  detected — the Phase 1A committed-and-clean check holds.
- AC: local_only entries appear in the bundle memory section under
  the `## Local conventions (auto-derived)` header.
- AC: local_only entries DO NOT cause Lippa-side memory writes (per
  §5.5 lean).
- AC: bundle byte budgets still enforced; oversize produces visible
  markers.
- AC: per-entry size cap (§5.4 side-effect) enforced with marker.
- AC: state-machine transition audit log captures
  (hash, from_state, to_state, trigger, timestamp).
- AC: TUI dismissal-history view shows state per entry and supports
  promote/demote in-flow.

---

## 9. Things needing Lippa-side confirmation (if §5.5 = A or B)

(N/A if §5.5 = C, which is my lean.)

- Exact request schema for `POST /api/v1/memory/propose` —
  recon-1 confirmed existence; v0.1 spec needs the field list.
- Behavior of the source allowlist on the proposal endpoint
  specifically (does it reject anything other than `model_proposed`?).
- 4xx error surface (Phase 1B-style typed terminal errors).
- Idempotency semantics — what happens on retry with the same
  proposal body?
- Visibility model — per-user? Per-project? Per-workspace? Affects
  whether the user can preview what they'll publish.

---

## 10. Risks and pre-emptive flags

- **Identity hash brittleness.** Phase 1B's three-input hash treats
  different titles as different findings. §5.1-A means the candidate
  threshold may take longer to hit in practice than the number "3"
  suggests. v0.1 spec should set N=3 but also instrument the SQLite
  to measure "near-miss" rates (hashes with N-1 hits but no Nth) so
  Phase 3's fuzzy work is data-informed.

- **`.quorum/conventions.md` merge conflicts.** Two developers
  promoting different findings on the same day produce conflicting
  diffs. Phase 1C should append-only or section-keyed to minimize
  merge pain. v0.1 spec needs to take a position on the file format.

- **State-machine clock skew.** "All-time" recurrence (§5.1 lean)
  is robust to clock skew. Time-window options would need attention
  to timezone / monotonic time. Sticking with all-time avoids this.

- **Phase 1B regression surface.** Adding a state column to an
  existing SQLite table needs a v2 → v3 migration. v0.1 spec defines
  the migration; impl session will need a corresponding integration
  test.

---

*— end scoping notes —*
