# Quorum — v0.4: CI-feedback remediation (bundle, context, honest metadata, identity)

Spec version: v0.1 (draft — for peer review)
Date: 2026-09-15 (revised same day after a pass over `Quorum Docs`: two recon
items resolved from Phase 1A/1B preflight notes, one new recon item added, one
pre-existing live blocker surfaced — see §12)
Predecessor: Phase 1C v1.1 (highest AC in force: 175)
AC range claimed by this milestone: **176–241**
Companion analysis: `specs/Quorum-v0_4-CI-Feedback-Remediation.md`
Amended 2026-09-15 after the Stage 1 close, adjudicating the Stage 1 CC report.
Amendments are marked **[S1-ADJ]** and are binding on Stages 2–4.
Release note 2026-11-21: version 0.4.0 ships Stage 1 (ACs 176–194) plus
AC 211a, together with the SEC-1, DL Part 1, DL Part 2 and two-factor login
bug fixes and crates.io Trusted Publishing. SEC-1 also delivered the
`QUORUM_LIPPA_SESSION` / `QUORUM_LIPPA_BASE_URL` pairing that AC 240's last
sentence names; the rest of AC 240 stays with Stage 4. Stages 2–4 are
retargeted to 0.5.x.
Release note 2026-11-22: 0.5.0 shipped the Consensus tier and two
fail-closed hook fixes instead (no spec gate), so Stages 2–4 are
retargeted again, to 0.6.x.

---

## 1. Mission and scope

An external team evaluated Quorum v0.3.3 as a CI reviewer and declined it for
now. Their eight-point critique (seven raised, one found during analysis)
splits cleanly: four items are Quorum-local defects, two require a Lippa
Consensus schema change, one requires Lippa Bearer auth, and one is an
identity-hash fragility nobody had noticed.

**v0.4 ships every item that does not depend on Lippa.** It does not attempt
to make Quorum a CI reviewer — that is v0.5, gated on `M-ConsensusFindings`.
v0.4's goal is narrower and fully achievable: *make the bundle contain the
right bytes, make the metadata tell the truth, and stop the memory layer from
silently breaking.*

In scope:

| # | Item | Stage |
|---|---|---|
| 4 | File budget favours big docs over code | 1 |
| 2 | Only diff + changed files are visible | 1 |
| 3 | No policy input; AGENTS.md ignored | 2 |
| 5 | "Confidence" is inter-model agreement, misnamed and load-bearing | 3 |
| 6 | Range reviews carry the current branch name | 3 |
| 7 | `--show-session` is CI-hostile (hardening half only) | 4 |
| 8 | Model roster is an input to the identity hash | 4 |

---

## 2. Non-goals (scope discipline)

- **No finding anchors.** `Finding.file` / `line_start` / `line_end` are v0.5,
  gated on the Lippa contract. CC must not invent a client-side heuristic that
  guesses a file from a finding title.
- **No SARIF, no GitHub Action, no inline-comment posting.** v0.5.
- **No Bearer auth work.** v0.4 hardens the cookie surface; it does not
  restructure auth. `AuthMethod::Bearer` stays a stub.
- **No tool-calling / agentic file retrieval.** v0.4 improves *bundling*.
  Mid-review file requests are a Lippa capability, not a client feature.
- **No change to the dismissal state machine** (`candidate` → `local_only` →
  `promoted_convention`). v0.4 changes only the *key*, never the states.
- **No Lippa calls added or changed.** `SessionCreateRequest` keeps its four
  fields. The `model_roles` selector request is filed upstream, not built here.
- **No cloud sync.** Phase 1D remains deferred.
- **[S2-RECON] Quorum never sends `context_text`.** Recon established that the
  field reaches the models verbatim and uncapped, *and* that supplying it
  suppresses Lippa's project-memory seeding pack: both generation guards
  (`consensus.py:494`, `consensus_service.py:214`) are `not seeding_context`,
  so a client-supplied value means `_generate_seeding_context` never runs and
  the confirmed/pinned `MemoryItem` rows are silently dropped for that session.
  Spending that field on a static policy file would trade Quorum's entire
  memory wedge for a placement Quorum already controls. This is a standing
  constraint, not a Stage 2 detail — a future session reaching for
  `context_text` innocently is the failure mode this line exists to prevent.

---

## 3. Stages and halt gates

Four stages, each independently shippable and independently revertable. CC
**halts** at the end of every stage for Rolf's approval before starting the
next. Standard three-category halt rule applies within a stage.

| Stage | Theme | Items | ACs |
|---|---|---|---|
| 1 | Bundle composition | 4, 2 | 176–194 |
| 2 | Context inputs | 3 | 195–211 |
| 3 | Honest metadata | 5, 6 | 212–225 |
| 4 | Identity and auth hygiene | 8, 7 | 226–241 |

Stage 4 contains the only irreversible-by-default operation in the milestone
(a SQLite migration over user data). It is deliberately last and carries a
mandatory backup step.

---

## 4. Stage 1 — Bundle composition

### 4.1 Problem

`bundle.rs::files_section()` sorts eligible files `size_bytes` **descending**
and fills `BUDGET_FILES = 80 * 1024` largest-first. Two large markdown files
consume the whole budget and evict the code under review. Separately,
`files_section()` iterates only `staged.files`, so an unchanged
`eslint.config.js` — the file that decides whether a lint finding is real —
is structurally unreachable.

### 4.2 WI-1 — Priority scoring replaces largest-first

Replace `eligible.sort_by_key(|f| Reverse(f.size_bytes))` with a deterministic
priority score. Lower score sorts first.

**[S1-ADJ]** Ordering is **lexicographic over a tuple**, terms
most-significant first — not an integer sum. The v0.1 additive form
(`class_rank * 1000 - min(hunk_count,99) * 10 + size_tiebreak`) was defective:
`size_bytes` is unbounded, so a 60 KB file contributes 61440 and swamps the
1000-wide class bands, reintroducing the exact largest-first eviction WI-1
exists to remove. CC caught this and implemented the tuple. The tuple is
authoritative:

```
(class_rank, is_related, Reverse(min(hunk_count, 99)), size_bytes, path)
```

`is_related` (0 = changed, 1 = unchanged context) sits directly after
`class_rank` to satisfy AC 185 — see that criterion. Within any
(class, origin) group the tuple and the old formula agree, because the hunk
term never crosses a class band.

`class_rank` is derived from the path:

| Rank | Class | Match |
|---|---|---|
| 0 | Source under review | any file that appears in the diff and is not matched below |
| 1 | Config | `eslint.config.*`, `.eslintrc.*` (and bare `.eslintrc`), `tsconfig*.json`, `package.json`, `Cargo.toml`, `pyproject.toml`, `go.mod`, `.editorconfig`, `*.toml` at repo root, **[S1-ADJ]** `*.yml` / `*.yaml` |
| 2 | Test | path contains `/tests/`, `/test/`, `__tests__`, or basename matches `*.test.*`, `*.spec.*`, `test_*.py`, `*_test.go` |
| 3 | Docs / data | `*.md`, `*.mdx`, `*.rst`, `*.txt`, `*.json` not matched above, `*.lock`, `*.snap` |

`size_bytes` is **ascending** (smaller first), so that when class, origin and
hunk count tie, the budget admits more files rather than fewer.

**[S1-ADJ]** `.sql` and `.sh` deliberately fall through to Source (rank 0):
they are executable logic, and when changed they are the subject of review.
`.yml`/`.yaml` are promoted to Config because a changed CI workflow
outranking the changed tests — which the Stage 1 probe surfaced — is wrong.

**AC 176.** Eligible files are ordered by the tuple above; `path` is the final
term, so ordering is total and fully deterministic. **[S1-ADJ]** Satisfied at
Stage 1 close by `Candidate::sort_key`.
**AC 177.** `class_rank` classification is a pure function of the path string,
unit-tested against a table of at least 20 paths covering all four ranks.
**AC 178.** A bundle whose changed set is `{a.md (60KB), b.md (30KB),
src/x.ts (4KB), src/x.test.ts (2KB), types.ts (1KB)}` includes `src/x.ts`,
`src/x.test.ts` and `types.ts` in full, and omits or truncates the markdown —
the exact dogfood shape from the external review, as a regression test.
**AC 179.** `hunk_count` is available per `StagedFile`. `quorum-core::git`
populates it when building `StagedDiff` for both `StagedIndex` and
`CommitRange` sources. Deleted and binary files carry `hunk_count = 0`.
**AC 180.** The bundle emits a `## File inclusion order` block listing each
included path with its class and hunk count, so the ordering is auditable
from the archive without re-running.
**AC 181.** `files_omitted` continues to list every omitted path, and the
`[file omitted: …; budget exhausted]` marker is unchanged in shape.
**AC 182.** No file is included twice, including when a path matches both the
config allowlist (WI-2) and the changed set.
**AC 182a.** Every file body in the changed-file section is emitted with
**1-based line numbers** in a fixed `%6d | ` gutter. The concept spec §6.1
specified "each modified file in full (with line numbers)"; v0.3.3 emits raw
blobs. Without visible line numbers a model cannot cite one even when it knows
the answer — so this is the cheapest thing in the milestone that moves toward
anchors, and it works before `M-ConsensusFindings` exists. The gutter counts
against the section budget; the per-file overhead (~9 bytes/line) is included
in the budget arithmetic of §5.3.
**AC 182b.** Line numbering is applied to related/context files too, and the
`(unchanged, context)` label makes clear the numbers refer to the working tree
at HEAD, not to the diff.

### 4.3 WI-2 — Related-file inclusion

Two sources of unchanged files, both bounded and both reported.

**Config allowlist.** When the changed set contains a file of a given
language, the corresponding repo-root config files are added as candidates at
`class_rank = 1` even though unchanged. Language → config mapping is a
constant table (`.ts`/`.tsx`/`.js`/`.jsx` → eslint + tsconfig + package.json;
`.rs` → Cargo.toml; `.py` → pyproject.toml + setup.cfg; `.go` → go.mod).

**One-hop neighbours.** For each changed source file, parse first-party import
specifiers and add the resolved files as candidates at `class_rank = 0`.
Resolution is textual and conservative: relative specifiers only (`./`, `../`,
`crate::`, `super::`), resolved against the working tree, with the standard
extension candidates tried in a fixed order. A specifier that does not resolve
to a tracked file is skipped silently.

**AC 183.** Config-allowlist files are read from the **working tree at HEAD**,
not the index, and are labelled `(unchanged, context)` in their section header
so a model cannot mistake them for part of the diff.
**AC 184.** One-hop resolution is **one hop only** — neighbours of neighbours
are never added. **[S1-ADJ]** The Rust specifier set includes `mod x;`
declarations alongside `crate::` / `super::`; without it one-hop finds almost
nothing in a Rust repo, which is the repo Quorum is dogfooded on.
**AC 185.** Related files never displace a changed file: the scorer places all
changed files ahead of all unchanged context files of the same class.
**[S1-ADJ]** This requires an explicit changed/context term — a changed file
with `hunk_count == 0` would otherwise tie with an unchanged one. It is the
`is_related` term in the AC 176 tuple, not an emergent property of the score.
**AC 186.** Related-file discovery adds at most `related_file_max` files
(default 12, range 0..=64); when the cap binds, the omission is reported on
stderr and in the inclusion-order block.
**AC 187.** Deny-list rules apply identically to related files — a related
file matching a deny-list pattern is excluded with the same marker.
**AC 188.** Related files respect `MAX_FILE_BYTES` and the binary check.
**AC 189.** Related-file discovery never leaves the repository root; a
specifier resolving outside it is skipped.
**AC 190.** Setting `related_files = false` in config restores exactly the
v0.3.3 candidate set (changed files only), byte-for-byte, **[S1-ADJ] when
`include` is empty**. `related_files` disables automatic *discovery*; an
explicit `include` glob is a direct user instruction and is honoured
regardless. CC's reading is confirmed.

### 4.4 WI-3 — `[bundle]` config section

```toml
[bundle]                      # v0.4 — all keys optional
total_budget_kb = 200         # 100..=1024; sections scale proportionally
related_files = true          # false restores v0.3.3 behaviour
related_file_max = 12         # 0..=64
include = []                  # extra globs, always offered as candidates
```

**AC 191.** All four keys are optional; an absent `[bundle]` section yields
the documented defaults with no warning, matching the `[memory]` precedent.
**AC 192.** Out-of-range values produce `ConfigError::OutOfRange` and exit 2 —
they are never clamped.
**AC 193.** `include` globs are matched against repo-relative paths; matched
files enter as candidates at the class their path implies, not at a privileged
rank. **[S1-ADJ]** `include` is evaluated independently of `related_files`
(see AC 190).
**AC 194.** Raising `total_budget_kb` scales every section budget by the same
ratio, rounded down, with the 2 KB envelope overhead held constant.
**AC 194a.** Because Lippa applies no maximum prompt size (recon-v04-1), an
oversized bundle is not rejected — it reserves credits, runs, and comes back
`failed`. So when `total_budget_kb` is set above 200, a one-line stderr warning
names the risk and the fact that a failed session still spends credits. The
warning is suppressed under `--hook-mode=*`. **[S1-ADJ]** It fires at
config-read time rather than bundle-assembly time — it is a statement about
configuration, and it is testable there without a mock Lippa.

> **Note for review.** The v0.3.3 section budgets sum to 210 KB against a
> 200 KB total cap — they are maxima, not reservations, and `assemble()`
> errors only on the total. v0.4 keeps that model but makes the arithmetic
> explicit (§5.3).

---

## 5. Stage 2 — Context inputs

### 5.1 WI-4 — Discovery returns every candidate

`discovery.rs` is first-match-wins: `CLAUDE.md` present means `AGENTS.md` is
dropped into `ignored` and never reaches the bundle. Teams that maintain both
silently lose half their context.

This is a **regression against the original design**, not a considered
trade-off. Concept spec §6.1 lists the context bundle as containing
"Repo-level conventions: `CLAUDE.md`, `AGENTS.md`, `.cursorrules`,
`.quorum/conventions.md`" — all four, together. First-match-wins was a Phase 1A
simplification that was never revisited. WI-4 restores the intent.

**AC 195.** `Discovery` gains `chosen_all: Vec<(String, PathBuf)>` in
precedence order (`CLAUDE.md`, `AGENTS.md`, `.cursorrules`). `chosen` and
`chosen_path` are retained as the first element for call-site compatibility.
**AC 196.** `build_memory_section` renders every discovered file, each under
its own `### <basename>` sub-header inside the `## Repo memory` block.
**AC 197.** The files share `BUDGET_MEMORY`, consumed in precedence order; the
existing `[memory truncated: …]` marker fires on overflow and names the file
that was cut.
**AC 198.** `Discovery.ignored` remains populated for files that exist but did
not fit, so the stderr note keeps working, with its wording changed from
"ignored" to "not included (budget)".
**AC 198a. [S2-RECON]** The stderr line `quorum: using repo memory: {chosen}`
(`review.rs:172-173`) names **every** file rendered into the memory section,
not just the precedence winner. Left as-is it compiles and the gate still
fires, but it reports "using repo memory: CLAUDE.md" while the bundle also
carries `AGENTS.md` — a silent user-visible regression introduced by WI-4
itself, and the exact confusion item 3 exists to remove.
**AC 198b. [S2-RECON]** The `## Memory context\n[not loaded: {chosen}]`
fallback (`bundle.rs:388-391`) represents partial failure. With N discovered
files it is currently all-or-nothing on element 0, so "two read, one failed"
renders as though nothing failed. Each discovered file that exists but could
not be read is named individually; `review.rs:185` must stop swallowing the
read error into `None` without recording which file it was.
**AC 199.** A repo with only `AGENTS.md` behaves exactly as v0.3.3 did.
**AC 200.** The auto-derived local-conventions subsection is unaffected —
it still follows the repo-memory block and shares the same budget.
**AC 200a. [S2-RECON]** `Discovery` gains `#[non_exhaustive]`. It is a public
`quorum-core` struct with no `Default` and no `#[non_exhaustive]`, so adding
`chosen_all` is a breaking change for any external constructor — the test
fixture `bundle_assembly.rs:15-20` builds one by bare struct literal today,
which means external construction is currently sanctioned. Mark it in the
same commit that adds the field, and note the break in `HISTORY.md`.
**AC 200b. [S2-RECON]** Both stderr lines get end-to-end coverage. Nothing in
`crates/quorum-cli/tests/` currently matches `using repo memory` or
`ignored:`, so AC 198's wording change and AC 198a's regression would both
ship uncaught. Also mark `Quorum-Phase1A-Spec-v1_0.md` AC 25 (line 718) and
§507 as superseded: they document the multi-candidate `ignored:` stderr as
required behaviour, and WI-4 makes that false.

### 5.2 WI-5 — First-class review policy

A review policy is *user input*, not a promoted convention. It must not have
to pass the `.quorum/conventions.md` committed-and-clean trust gate, and it
must not compete with a 10 KB memory budget.

**[S2-RECON] CC-Recon-v04-6 resolved: §5.2 stands, the policy stays in
`prompt`.** The recon HALT trigger was drafted as a binary — verbatim delivery
means move the policy, an 8000-char pack cap means keep it — and neither is
what Lippa does. `context_text` *is* verbatim and uncapped (no route check, no
truncation, `Text` column, `body_cap.py` not wired to this route), so the
letter of the trigger was met; but it is the same field as the memory pack and
setting it turns the pack off. Beyond that it buys nothing §5.2 needs:
`context_text` arrives unlabelled and bare-prefixed ahead of `Question:`
(`consensus_service.py:580-582`, repeated per round at `730-732`), so AC 202's
delimiters and attribution would have to be embedded in it anyway; AC 201's
ordering is a `format!` argument order, not an API capability; and both fields
are concatenated into one user message, so no budget is gained. Decision:
keep the policy in `prompt`; do not redesign.

```toml
[review]                          # v0.4
policy_file = ".quorum/policy.md" # optional; path relative to repo root
fail_on = "divergence"            # see §6.2
```

Plus a CLI flag `--policy <path>` which overrides the config key.

**AC 201.** The policy is rendered in its own `## Review policy` section,
placed **before** the diff so it frames the review rather than trailing it.
**AC 202.** The policy section is wrapped in the standard
`QUORUM_REPO_CONTENT` delimiters like every other repo-sourced block, and is
described to the model as instructions *from the repository owner*, still
never as instructions to override Quorum's own prompt.
**AC 203.** `BUDGET_POLICY = 24 * 1024`. Overflow emits
`[policy truncated: N bytes exceeded 24KB]`.
**AC 204.** A `policy_file` path that does not exist is a hard error at exit 2
for an explicit `--policy`, and a stderr warning with the review proceeding
for a config-supplied path that has gone missing.
**AC 205.** The policy file is read from the working tree and requires **no**
git tracking, no commit, and no cleanliness check. This is a deliberate
divergence from the conventions-file trust rule and is documented as such in
`SERVICES.md`.
**AC 206.** The policy is never written to, never promoted, and never enters
the dismissal state machine.
**AC 207.** `--policy` is accepted by `quorum review` in all modes including
`--hook-mode=*` and `--tui`.
**AC 208.** With no policy configured, the `## Review policy` section is
absent entirely — not an empty header.

### 5.3 Budget rebalance

Default section budgets at `total_budget_kb = 200`:

| Section | v0.3.3 | v0.4 |
|---|---|---|
| Diff body | 100 KB | 80 KB |
| Changed-file + related contents | 80 KB | 72 KB |
| Repo memory | 20 KB | 16 KB |
| Review policy | — | 24 KB |
| Conventions | 10 KB | 6 KB |
| Envelope overhead | ~2 KB | ~2 KB |
| **Total cap** | **200 KB** | **200 KB** |

**AC 209.** Section constants are derived from `total_budget_kb` at runtime,
not hard-coded, so AC 194's proportional scaling is structural. **[S1-ADJ]**
Stage 1 shipped the derivation *mechanism* with v0.3.3 proportions preserved;
the rebalance table above (80/72/16/24/6) is a Stage 2 behaviour change and
lands with the policy section, not before it. CC was right to refuse to ship
it under a Stage 1 commit.
**AC 210.** `assemble()` still returns `BundleError::BundleTooLarge` when the
assembled prompt exceeds the total, and the error names the section that
overflowed.
**AC 211.** A repo with no policy file yields a bundle at least as large as
v0.3.3 would have produced for the same diff — the policy budget is not
reserved when unused.
**AC 211a. [S1-ADJ] — promoted from BACKLOG, do not defer again.**
`bundle.rs::truncate_with_marker` slices `text[..budget_for_text]` on a byte
index and panics when that index lands mid-codepoint. Route it through
`truncate_at_codepoint_boundary`, which is already imported into `bundle.rs`
and already used at the local-conventions cap. Reproduced, not assumed: on a
diff of accented source comments the shipped function panics for 292 of 4000
budget values sampled in 4000..8000, and 600 of 4000 with an emoji in a
comment. For pure-ASCII input the fixed version is byte-identical to the old
one, so the "it shifts marker byte counts several tests pin" concern does not
apply to ASCII fixtures — add one non-ASCII test rather than deferring.
Severity: `quorum review` panics rather than failing open, which under a
pre-push hook is the one failure mode `Fail-open at hooks` exists to prevent.
Stage 1's line-number gutter (AC 182a) adds ~9 bytes per line and therefore
pushes more bundles into truncation, so the probability of hitting this went
**up** at Stage 1 close.

---

## 6. Stage 3 — Honest metadata

### 6.1 WI-6 — `confidence` is agreement, and stops driving severity

Lippa's cluster `confidence` is inter-model agreement, not a probability that
the finding is correct. The external review scored incorrect findings at
0.82–0.95 because three models agreeing on the same wrong inference is a
high-agreement event by construction. Quorum then amplifies the mistake:
`AGREEMENT_HIGH_CONFIDENCE = 0.85` promotes high-agreement clusters to
`Medium`, and every `divergence` cluster becomes `High` — so *disagreement*
is the loudest signal Quorum can emit, which is backwards for a gate.

**AC 212.** `Finding.confidence` is renamed `Finding.agreement`. The wire
field read in `parse_clusters` is unchanged (`confidence` on the Lippa side);
only Quorum's own vocabulary changes.
**AC 213.** `AGREEMENT_HIGH_CONFIDENCE` and the confidence→severity promotion
are deleted. Severity mapping becomes purely structural:
`Divergence → Medium`, `Agreement → Medium`, `Assumption → Info`.
**AC 214.** No finding is assigned `Severity::High` in v0.4. `High` remains in
the enum, reserved for model-assigned severity in v0.5.
**AC 215.** Finding body text renders agreement as a count, not a float:
`Agreement: 2 of 3 models (claude-sonnet, gpt-4o).` The denominator is
`Review.model_names.len()`; when it is 0 or smaller than `supported_by.len()`,
the body falls back to `Supported by: …` with no denominator.
**AC 216.** The float is still carried in the archive JSON under the key
`agreement`, and the key `confidence` no longer appears in Quorum output.
**AC 217.** The archive JSON carries an explicit schema version, bumped by
this change (see CC-Recon-v04-3).
**AC 218.** TUI panels, markdown render, and `--json` all use the same
formatter — the string is composed in exactly one place.
**AC 219.** `[review] fail_on` controls the exit code: `"never"` (always 0),
`"divergence"` (exit 1 when any `Divergence` finding survives dismissal
filtering — the v0.3.3 effective behaviour, now named), `"any"` (exit 1 when
any finding survives). Default `"divergence"`. `Review::has_high_severity()`
is replaced by `Review::should_fail(policy)`.

### 6.2 WI-7 — Range-aware repo facts

`git.rs::repo_metadata()` reads `head.shorthand()` from the repository HEAD
unconditionally, and `envelope_header()` writes it into the prompt as
`branch:`. Under `quorum review --range a..b` from a different branch, the
models are told a branch that has nothing to do with the commits they read.

**AC 220.** `repo_metadata` takes the `DiffSource` and, for
`CommitRange { base, head }`, derives `head_sha` from the resolved range head
commit rather than repository HEAD.
**AC 221.** `RepoFacts.branch` becomes `Option<String>`, populated only when
the range head resolves to a branch tip (or, for `StagedIndex`, from HEAD as
today). Detached HEAD yields `None` rather than the literal `"HEAD"`.
**AC 222.** `envelope_header()` omits the `branch:` line entirely when
`branch` is `None`. An absent field beats a wrong one.
**AC 223.** The envelope gains a `range: <base>..<head>` line for
`CommitRange` reviews, so the model knows what it is looking at.
**AC 224.** The archive JSON and the SQLite `branch_snapshot` column accept
the absent case without becoming `"HEAD"` or `""`; `branch_snapshot` records
`(detached)` for the null case to satisfy its `NOT NULL` constraint.
**AC 225.** `crates/quorum-cli/tests/range_diff.rs` gains a negative-control
test: a range reviewed from a *different* branch must not emit the checked-out
branch's name anywhere in the bundle.

---

## 7. Stage 4 — Identity and auth hygiene

### 7.1 WI-8 — The model roster leaves the identity hash

`memory/identity.rs` hashes `title || source_type || sorted_models`. The model
roster is part of the key, and Quorum does not choose it — `SessionCreateRequest`
has no model field, and `SERVICES.md` §3 records that `model_roles` is not sent
because preflight found it to be display metadata rather than a selector. So the
roster is Lippa's, it can change without warning, and when it does every stored
dismissal hash silently stops matching: dismissals resurface, promoted
conventions stop firing, nothing explains why.

A finding's identity is what it says about which code — not which models
happened to say it.

**AC 226.** `finding_identity_hash` becomes a two-input hash:
`normalize_title(title) || 0x1F || source_kind`. The models input is removed.
**AC 227.** `models_snapshot` is retained on the row, unchanged, as forensic
provenance. No column is added or dropped.
**AC 228.** Schema v2 → v3 migration recomputes every `dismissals` row's
`finding_identity_hash` from its stored `title_snapshot` and
`source_type_snapshot`. No data is lost and no user re-entry is required.
**AC 229.** The migration writes `.quorum/dismissals.sqlite.v2.bak` (a
byte-copy of the pre-migration file) before any DDL or DML, and aborts the
whole migration if the backup cannot be written.
**AC 230.** Hash collisions created by the migration — rows that differed only
by model set — are **merged**, not dropped, into a single surviving row:
`dismissed_at` = earliest, `last_seen_at` = latest, `recurrence_count` = sum,
`promotion_state` = most advanced (`promoted_convention` > `local_only` >
`candidate`), `models_snapshot` = union, `reason`/`note` from the row with the
earliest `dismissed_at`.
**AC 231.** Every row in `state_transitions` and every row in `conventions`
referencing a merged-away hash is repointed to the surviving hash **before**
the losing rows are deleted, so `ON DELETE CASCADE` never fires during
migration (see CC-Recon-v04-4 for the full reference inventory).
**AC 232.** The migration appends one synthetic `state_transitions` row per
merge, recording the merge with a reason string, so the audit trail explains
the count change.
**AC 233.** `CURRENT_VERSION = 3` and `forward_compat_min_version = '3'`.
**AC 234.** The migration is idempotent: re-running against a v3 DB is a no-op
and `schema_version` still holds exactly one row.
**AC 235.** The migration runs inside a single transaction; any failure rolls
back and leaves a v2 DB plus the backup file, with a stderr message naming
the backup path.
**AC 236.** A negative-control test constructs a v2 DB with two rows that
differ only in `models_snapshot`, migrates, and asserts exactly one surviving
row with the summed `recurrence_count` and the advanced `promotion_state`.

### 7.2 WI-9 — `--show-session` stops being CI-shaped

`commands/auth.rs` prints the live cookie to stdout behind a TTY prompt or
`-y`. The gate assumed a human at a terminal; in CI the `-y` path pipes a live
credential through a runner that logs.

**AC 237.** `auth --show-session` refuses to run when `CI` is set in the
environment, regardless of `-y`, unless the new explicit flag
`--i-understand-this-leaks` is also passed.
**AC 238.** When the command does proceed and `GITHUB_ACTIONS` is set, it
emits `::add-mask::<value>` on stdout **before** the value itself.
**AC 239.** The refusal message names the alternative
(`QUORUM_LIPPA_SESSION` as a masked repository secret, set out of band) rather
than only saying no.
**AC 240.** `README.md` and `SERVICES.md` state that `--show-session` must
never appear in a workflow file, and that cookie auth is an interim mechanism
pending Lippa `M-ExternalAuth`. The docs must **not** promise that Bearer
solves the leak problem: per §11.4, the Phase 1 Bearer token is the same
plaintext credential as the user's MCP and extension token, so a leak is worse,
not better. Say that plainly rather than deferring to a future milestone.
The refusal message names `QUORUM_LIPPA_SESSION` and `QUORUM_LIPPA_BASE_URL`
as the pair CI must set, since neither works without the other after SEC-1.
**AC 241.** No other code path prints, logs, or writes a cookie value. A test
asserts that the only `expose()` call reaching stdout is the one in
`--show-session`.

---

## 8. Recon items (CC resolves before implementing the owning stage)

- **~~CC-Recon-v04-1~~ — RESOLVED, no CC work needed.** Phase 1A preflight
  (Blocker 3) already established it: `consensus.py` enforces a **minimum** of
  20 characters and **no maximum** on `prompt`. Oversized prompts pass through
  to the model providers and fail as session status `failed`. So the 200 KB cap
  is entirely Quorum's own invention, and the real ceiling is the narrowest
  context window in a roster Quorum cannot see or choose. This changes the
  shape of the risk rather than removing it — see AC 194a.
- **CC-Recon-v04-2 (Stage 1).** Whether `git2`'s diff API exposes per-file hunk
  counts directly, or whether `hunk_count` must be counted from the unified
  patch text. Prefer the API; fall back to counting `@@` headers per file
  section only if the API does not provide it.
- **CC-Recon-v04-3 (Stage 3).** Whether `archive.rs` already emits a schema
  version field. If yes, bump it. If no, add one and record the addition in
  `DATABASE.md` — the archive is a consumed contract and renaming
  `confidence` → `agreement` without a version marker breaks silent consumers.
- **CC-Recon-v04-4 (Stage 4).** The complete inventory of tables and columns
  referencing `finding_identity_hash` (at minimum `dismissals`,
  `state_transitions`, `conventions`). The v3 migration must repoint every one
  of them. **HALT** if any reference is found that this spec does not name.
- **CC-Recon-v04-5 (Stage 2).** Whether any existing call site treats
  `Discovery.ignored` as "these files were rejected" in a way that AC 198's
  semantic change would break.
- **CC-Recon-v04-6 (Stage 2) — NEW, potentially design-changing.** The
  session-create endpoint accepts two multipart fields Quorum has never used:
  `context_text` and `document` (recon-v0 §endpoint 4). If `context_text` is
  delivered to the models verbatim, the review policy belongs there rather than
  inside `prompt`, and §5.2 should be redesigned around it. If instead it feeds
  the server-side context pack — `consensus_service.CONSENSUS_CONTEXT_PACK_MAX_CHARS
  = 8000` — then it is far too small for a policy and §5.2 stands as written.
  Read `../lippa` and determine which. **HALT** if it is the former: that is a
  spec-level design decision, not CC's call.

---

## 9. Rollback

Each stage is a separate commit series behind no feature flag; rollback is
`git revert` of the stage's commits, with these exceptions:

- **Stage 1.** Setting `related_files = false` and `total_budget_kb = 200`
  restores v0.3.3 *candidate selection* but not the ordering. Full rollback is
  a revert.
- **Stage 3.** Reverting restores the `confidence` key in archive JSON.
  Archives written by a v0.4 binary keep the `agreement` key and their bumped
  schema version; consumers must tolerate both. This is the one
  forward-incompatible artefact in the milestone and is why AC 217 exists.
- **Stage 4.** Reverting the binary does **not** revert the database.
  Recovery is: stop, restore `.quorum/dismissals.sqlite` from the
  `.v2.bak` written by AC 229, then run the reverted binary. This must be
  documented in `HISTORY.md` at stage close, not just in this spec.

---

## 10. Definition of done

- All ACs 176–241 verified, each with a named test or a named manual check.
- `cargo test --workspace`, `cargo clippy --workspace --all-targets -D warnings`,
  `cargo fmt --check --all` clean.
- `SERVICES.md` updated: policy-file trust divergence (AC 205), bundle
  composition rules, `--show-session` CI rule.
- `DATABASE.md` created or updated: schema v3, the merge semantics, the backup
  and recovery procedure, archive schema version.
- `BACKLOG.md`: the "Bundle budget rethink for real repos" entry closed;
  "Archive schema: model name normalization" updated to note it now depends
  only on the upstream naming request.
- `CLAUDE.md` milestone status updated; hard cap ≤ 280 lines re-verified.
- `HISTORY.md` v0.4 entry with the Stage 4 recovery procedure.

---

## 11. Deliberately upstream (not built here)

Filed against Lippa, tracked in the Lippa repo, referenced here so reviewers
can see what v0.4 is *not* solving:

1. **`M-ConsensusFindings`** — per-finding `file`, `line_start`/`line_end`,
   `explanation`, `evidence`, `suggested_patch`, model-assigned `severity`,
   and `confidence` kept separate from `agreement`. Unblocks items 1 and 5's
   real fix, and Phase 1D identity stability.
2. **Canonical versioned model identifiers** in one namespace, joinable
   between session-level `models[]` and per-finding `supported_by`.
3. **`model_roles` as a real selector.** Today it is
   `Optional[dict[model_uuid → role_label]]`, consumed only at the GET-detail
   handler for display; the roster is selected server-side from
   `ConsensusModelProfile WHERE is_default=True AND health_state='active'`,
   with `len(profiles) >= 2` enforced (Phase 1A preflight D1; a live probe
   returned `models_active: 3`). So a Lippa workspace admin editing a model
   profile silently changes every Quorum review and — until AC 226 lands —
   every stored dismissal key. A real selector would let a review pin its
   roster, be reproduced, and bound its own context budget. Bears directly on
   the open Consensus non-determinism blocker.
4. **[S2-RECON] A base64 sibling field on the Consensus create route**, so a
   diff that trips the Cloud Armor OWASP packs can be submitted without being
   mistaken for the attack it describes. Lippa has applied exactly this remedy
   on three prior routes. Until it exists, Quorum cannot review security fixes
   reliably — see §12.1.

5. **[S2-RECON] `document` upload failures are swallowed silently.**
   `consensus.py:545-562` wraps the whole upload in
   `except Exception … logger.warning`: a bad extension, an empty extraction or
   an R2 failure produces no client error, and the session is created, charged
   and run with no document. A `document_critique` session then critiques
   nothing, at full credit cost. Not Quorum's surface — Quorum sends no
   `document` — but it is a paying-user-visible defect found while reading, and
   filing it is free.

6. **Bearer auth — and a correction to how this milestone has been describing
   it.** the milestone exists but stalled, and reading it
   changes the plan. The full chain is now accounted for: **v0.1** (draft) →
   **v0.1 adjudication** (3 peer reviews; 2× NEEDS REWORK, 1× ACCEPT WITH
   CHANGES; five Critical findings, chiefly that v0.1 mutated
   `request.session` on Bearer success) → **v0.2** (the rework, draft, spec
   home `packages/shared_specs/`). The adjudication's own next step was "one
   re-review by Lippa Doc Reviewer, then finalize as **v1.0**" — and that v1.0
   is what `Quorum-Phase1B-Spec-v1_0.md` cites. **It was never produced.** The
   milestone is parked at a reworked draft awaiting a single review pass, and
   nothing suggests it has been implemented. Reading v0.2:

   **Phase 1 does not deliver a scoped token.** Its own §2 non-goals are
   explicit: no new token format, no revocation list, no scope-restricted
   tokens (`AuthPrincipal.scopes` is hardcoded `("all",)`), no hashed-at-rest
   storage. The token *is* `User.extension_token` — a plaintext UUID that
   already authenticates `/mcp/*` and `/api/extension/*`. Any valid one
   authenticates as the full user.

   So swapping cookie → Bearer at Phase 1 is a **trade, not a fix**:

   | | Cookie (today) | Bearer Phase 1 |
   |---|---|---|
   | Manual rotation cadence | every ~2 weeks (`Max-Age=1209600`) | never — solves the reviewer's complaint |
   | Privilege if leaked | that user's session | that user's session **and** their MCP surface and browser extension — same credential |
   | Expiry if leaked | 14 days | none, until manually rotated |
   | Revocable | logout | rotation only — which breaks every other client sharing the token |

   The blast radius gets **worse**, and the CI-logging guard of WI-9 therefore
   matters more after Phase 1, not less. The reviewer's actual ask — "a scoped
   API token" — is Lippa **Phase 2**: `personal_access_tokens`, typed `lpa_`
   prefix, hashed at rest with `hmac.compare_digest`, meaningful scopes.

   Consequences for Quorum:
   - Item 7's "real fix" is retargeted from Phase 1 to **Phase 2**. Phase 1 is
     a partial improvement worth taking, not the end state. Amend the companion
     analysis accordingly.
   - Phase 1 ships behind `EXTERNAL_AUTH_BEARER_ENABLED`, default `False`,
     flipped per workspace. Quorum cannot depend on it being on.
   - Quorum must keep **both** auth paths and send exactly one. That spec's
     §6.1 criterion 6: cookie present + malformed `Authorization` header → 401,
     even with a valid cookie. A client that sets both is a bug.
   - That spec's CC-Recon-6 already names Quorum's side of rotation: *"CLI
     receives 401 after rotation, prompts user to update token via `quorum auth
     login`."* Whatever milestone does the Bearer swap must implement that.

   **Unblocking it is cheap.** v0.2 needs one Lippa Doc Reviewer pass and a
   promotion to v1.0 — the adjudication estimated 2–3 days wall-clock to
   CC-ready, and CC work at 2–3 days. That is a small, well-specified piece of
   upstream work standing between Quorum and the end of manual cookie
   rotation. Worth scheduling regardless of v0.4.

   Not copied into `specs/` — these are Lippa-side documents and the hard
   constraint routes Lippa specs to the Lippa repo. Referenced by path only.

---

## 12. Pre-existing live blocker, surfaced during this pass (not v0.4 scope)

**D8 — `project_id` in the session-create multipart triggers 403 at the Lippa
edge.** Phase 1B preflight: same session, same prompt, `project_id` present →
403; field absent → 201 and the session converges. Phase 1A saw a related but
differently-shaped failure (a ~5s server-side seeding-context exception).

This still matters at v0.3.3: `client.rs` sends `project_id` whenever config
carries one, and `quorum link --project <id>` is the documented first-run step.
Any user who follows the README and then hits an edge that still enforces this
gets 403s on every review.

Not a v0.4 work item — it is Lippa-side and cannot be fixed from this repo. But
it should be re-probed before v0.4 ships, because v0.4's whole premise is that
the local pre-push gate is the position Quorum holds today. Recommend a
standalone probe (create one session with `project_id`, one without, same
prompt) and, if D8 persists, a Lippa issue plus a README caveat on `link`.

**[S2-RECON] Nothing in Lippa's source can produce a `project_id`-correlated
403.** The two route-level 403s are `CONSENSUS_ENABLED` off and `assert_tier`,
both field-blind; a bad or cross-tenant `project_id` returns 404 by an
explicitly documented tenant-non-leakage rule. `CsrfOriginMiddleware` is
field-blind and postdates D8. That leaves Cloud Armor, whose policy lives in
`lippa-infra` and not in either repo. **Cheap discriminator when re-probing:**
an edge 403 is `Content-Type: text/html` with no `server: Google Frontend`
header; an app 403 is JSON. HTML means Cloud Armor and a Lippa-side fix. JSON
means the handler did something the source says it cannot, which is worth
escalating.

### 12.1 [S2-RECON] Two hazards found while looking for D8

**`assert_tier` gates every Consensus create, and it is newer than the
preflight.** It landed on this route on 2026-07-09, after the Phase 1B
preflight was written, and returns 403 `plan_not_eligible` for a workspace
that is not on standard/max/team. Two consequences: any live verification in
Stages 2–4 fails on a Free-plan account for reasons unrelated to the change
under test, so confirm the test account's tier first; and Quorum now has a
server-enforced paid-tier dependency that neither `README.md` nor
`SERVICES.md` states. Worth documenting for users before v0.4 ships.

**The OWASP packs sit in front of the diff Quorum submits.** `api.lippa.ai` is
grey-clouded past Cloudflare with Cloud Armor as the real WAF, and only
`POST ^/mcp$` and `/mcp/messages` are bypassed —
`/api/v1/consensus/sessions` is inside the packs. A diff containing
`<script`, `onerror=` or `../../etc/passwd` is the documented trip pattern.
This is a structural problem for a *code reviewer*: the changes most worth
reviewing are the ones most likely to look like an attack. Today's diffs get
through, which is evidence of tolerance, not a guarantee. It applies equally
to `prompt` and `context_text`, so it is not an argument either way for §5.2.
Lippa's own remedy for three prior occurrences was a per-route base64 sibling
field — a Lippa-side change, filed upstream in §11, not buildable here.
