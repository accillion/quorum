# Quorum — CI Feedback Remediation Plan (v0.4 / v0.5)

Status: draft for peer review
Date: 2026-09-15
Read against: `accillion/quorum` @ `main`, v0.3.3
Source: external review of Quorum run locally against a TypeScript/eslint repo,
in the context of an "AI review Step 2" CI programme (Codex step 1, Gemini step 2).

---

## 1. Verdict

Seven items were raised. Four are Quorum-local and ship in v0.4. Two —
missing anchors and uncalibrated confidence — are not Quorum bugs; they are
the shape of Lippa's Consensus response. One is auth and waits on Bearer tokens.

| Bucket | Items | Gate |
|---|---|---|
| Quorum-side, no upstream dependency | 2, 3, 4, 6 | none — v0.4 |
| Gated on Consensus finding schema | 1, 5 | Lippa `M-ConsensusFindings` |
| Gated on Lippa auth | 7 | Lippa `M-ExternalAuth` **Phase 2** (Phase 1 is partial — see item 7) |
| Found during this review, not raised | 8 | partly local, partly Lippa naming |

**Root cause of the two blockers.** Quorum surfaces a *debate*, not findings.
`quorum-core/src/review.rs` maps Lippa's `agreement[]` / `divergence[]` /
`assumptions[]` clusters; each cluster carries only `claim_text`, `confidence`
and `supported_by`. The module header already says it: *"There is no per-finding
severity, file, or line range in the wire format."* That single fact produces
item 1 and item 5, and it is the same schema gap already blocking Phase 1D
identity-hash stability in `BACKLOG.md`. One upstream milestone unblocks the CI
story and the memory wedge together.

---

## 2. Item by item

### Item 1 — Findings have no file, line or explanation — BLOCKED (Lippa)

- **Cause.** `quorum-core/src/review.rs`: `Finding` carries no path or range
  field, and `parse_clusters()` synthesises `body` as
  `"Confidence: 0.82. Supported by: gpt-4o, claude-sonnet."` The anchor never
  arrives from the wire; nothing is dropped client-side.
- **Fix (upstream first).** Consensus returns per-finding `file`,
  `line_start`/`line_end`, a real `explanation`, optional `suggested_patch`.
  See §3.
- **Fix (client, after).** Widen `Finding`; thread the anchor through
  `render.rs`, `tui/panels.rs` and the archive schema; add `--format=sarif`
  so GitHub renders findings as inline annotations natively, plus a stable
  `--format=json` for other consumers.
- **Client-side calibration lever.** Once anchors exist, validate them against
  the bundle: a finding pointing at a `file:line` not present in the submitted
  bundle is a hallucination and is dropped or downgraded before render. This
  works before Lippa calibrates anything.

### Item 2 — Only sees the diff and the changed files — v0.4

- **Cause.** `quorum-core/src/bundle.rs::files_section()` iterates
  `staged.files` and nothing else. An unchanged `eslint.config.js` is never a
  candidate. Quorum is a one-shot submit, not an agent loop — there is no grep
  available mid-review.
- **Fix — three layers, all local:**
  1. **Config allowlist.** Always include `eslint.config.*`, `.eslintrc.*`,
     `tsconfig.json`, `package.json`, `Cargo.toml`, `pyproject.toml`,
     `.editorconfig` when the diff touches a matching language. Costs a few KB
     and would have prevented all four reported false findings.
  2. **One-hop resolution.** Parse `import` / `require` / `use` from changed
     files and include direct neighbours; include the sibling test file for
     each changed source file.
  3. **`[bundle] include = [globs]`** in `.quorum/config.toml` for what the
     heuristics miss.
- **Later, not v0.4.** The real version is tool-calling on the Consensus
  session so models can request a file mid-review. Lippa capability; note it,
  do not wait for it.

### Item 3 — No review policy input; CLAUDE.md read, AGENTS.md ignored — v0.4

- **Cause.** Two deliberate decisions, both wrong for this user.
  `quorum-core/src/discovery.rs` is first-match-wins over
  `["CLAUDE.md", "AGENTS.md", ".cursorrules"]`; losers land in `ignored` and are
  dropped. `bundle.rs`: `BUDGET_CONVENTIONS = 10 * 1024` caps
  `.quorum/conventions.md` at 10 KB — a memory budget carrying a policy doc.
- **Fix.**
  1. `Discovery` returns *all* present candidates, concatenated in precedence
     order under the shared memory budget with a per-file header.
  2. First-class policy input: `--policy <path>` plus `[review] policy_file`,
     with its own budget line (~24 KB), deliberately outside the conventions
     trust machinery. Policy is user input, not a promoted convention; it must
     not have to pass the committed-and-clean gate.
  3. Rebalance the 200 KB envelope to absorb it.

### Item 4 — File budget favours big docs over code — v0.4

- **Cause.** `bundle.rs::files_section()`: *"Sort by size descending for
  largest-first inclusion"* against `BUDGET_FILES = 80 * 1024`. Two large
  markdown files win the 80 KB by construction. Same defect as the
  `BACKLOG.md` dogfood entry "Bundle budget rethink for real repos".
- **Fix.** Replace size-descending with a priority score:
  extension class (source > config > test > docs), diff density (more changed
  hunks first), source/test pairing, size as an **ascending** tiebreaker so
  more files fit. Emit inclusion order into the bundle for auditability.
  Regression test with the two-big-markdown-files shape.

### Item 5 — Confidence not calibrated — PART v0.4 / PART BLOCKED

- **Cause.** Not a calibration bug; a naming bug with teeth. The number is
  Lippa's *inter-model agreement* on a debate cluster, not P(finding correct).
  Three models agreeing on the same wrong inference about a config they cannot
  see scores 0.95 by definition. Quorum then amplifies it:
  `AGREEMENT_HIGH_CONFIDENCE = 0.85` promotes high-agreement clusters to
  `Medium`, and every `divergence` cluster becomes `High` — so models
  *disagreeing* is the loudest signal in a CI gate, which is backwards.
- **Fix now (v0.4, client).** Rename the field to `agreement` in output, docs
  and archive schema. Stop deriving severity from it. Show "2 of 3 models"
  rather than a number that reads as confidence and is not.
- **Fix properly (upstream).** A genuine per-finding `confidence` emitted by
  the model, kept separate from `agreement`, plus model-assigned `severity`.
  Pairs with the `BACKLOG.md` item that every dogfood finding returned
  `severity: medium`.

### Item 6 — Range reviews use the current branch name — v0.4

- **Cause.** Confirmed. `quorum-core/src/git.rs::repo_metadata()` takes
  `head.shorthand()` from repository HEAD unconditionally;
  `bundle.rs::envelope_header()` writes it into the prompt as `branch:`. Under
  `--range a..b` the models are told a branch unrelated to the commits.
- **Fix.** Derive facts from the range head commit for
  `DiffSource::CommitRange`. Make `branch` an `Option<String>`; omit the line
  when the range head is not a branch tip — an absent field beats a wrong one.
  Same treatment for `head_sha`. Cover in `crates/quorum-cli/tests/range_diff.rs`.
- **Effort.** ~2 hours. Cheapest real bug in the set.

### Item 7 — Session cookie printable; manual CI rotation — PART v0.4 / PART BLOCKED

- **Cause.** `crates/quorum-cli/src/commands/auth.rs:105` —
  `println!("{}", cookie.expose())`, behind `auth --show-session`. It is gated
  (TTY prompt, or `-y` with an explicit "acknowledge the secret leak" message),
  and `Secret` correctly redacts `Debug`/`Display`. But in CI the `-y` path
  pipes a live cookie through a runner that logs, and the value then lives in
  `QUORUM_LIPPA_SESSION`. The gate assumed a human at a terminal.
- **Fix now (v0.4).** Refuse `--show-session` when `CI` is set unless an
  additional explicit flag is passed; emit a GitHub Actions `::add-mask::`
  directive alongside; document that it must never run in a workflow.
- **Fix properly — corrected after reading the Lippa spec.**
  `M-ExternalAuth-Phase1-Spec-v0_2.md` (draft; shipped status unconfirmed) does
  **not** deliver a scoped token. Its non-goals rule out a new token format, a
  revocation list, scopes (`("all",)` hardcoded) and hashed-at-rest storage.
  The token is `User.extension_token` — a plaintext UUID that already
  authenticates `/mcp/*` and the browser extension.
  So Phase 1 is a trade: it removes the fortnightly manual rotation, and it
  makes a leak *worse* — no expiry instead of 14 days, and the same credential
  now covers the user's MCP surface. Worth taking, but the CI guard matters
  more afterwards, not less.
  The reviewer's actual ask — a scoped API token — is Lippa **Phase 2**:
  `personal_access_tokens`, `lpa_` prefix, hashed at rest, real scopes.
  Quorum keeps both auth paths and sends exactly one (that spec's §6.1
  criterion 6: cookie + malformed `Authorization` → 401 even with a good
  cookie), and implements the rotation UX its CC-Recon-6 already specifies:
  401 after rotation → prompt `quorum auth login`.

### Item 8 — Model roster is an invisible input (NOT raised by the reviewer)

**Quorum does not choose its models at all.** `SessionCreateRequest` carries
`prompt`, `project_id`, `debate_mode`, `idempotency_key` — no model parameter.
`SERVICES.md` §3 records the decision: *"Phase 1A does NOT send `model_roles`
(preflight D1: it's display metadata, not a selector)."* The roster is whatever
Lippa's Consensus configures for the project at request time; Quorum learns it
afterwards by reading `models[].vendor` off the detail response in `review.rs`.
The names in the repo (`gpt-4o`, `claude-sonnet`, `gemini-pro`) are test
fixtures and doc comments frozen at Phase 1A preflight, not configuration.

- **Cause.** `quorum-core/src/memory/identity.rs` hashes
  `title || source_type || sorted_models`. The model list is *part of the key*.
  When Lippa swaps a model — or a vendor merely renames one
  (`claude-sonnet` → `claude-sonnet-4-5`) — every stored dismissal hash
  silently stops matching. Dismissals resurface, promoted conventions stop
  firing, and nothing in the output explains why.
- **Severity.** Same wound as the known Phase 1D identity-hash instability,
  from a second direction: titles drift *and* the roster drifts. Silent
  failure — memory simply ceases to work.
- **Fix (local, v0.4).** Remove `sorted_models` from the identity hash; keep it
  alongside the row as a `roster_fingerprint` for forensics. A finding's
  identity is what it says about which code, not which models happened to say
  it. This repairs half the Phase 1D blocker with no upstream dependency.
- **Fix (upstream).** Canonical, versioned model identifiers in one namespace
  (`anthropic/claude-sonnet-4-5`), matching between session-level `models[]`
  and per-finding `supported_by`. Also closes the `BACKLOG.md` item that those
  two fields cannot be joined by a consumer of the archive JSON.
- **Ask for.** Make `model_roles` a real selector. Without it Quorum cannot pin
  a roster, cannot reproduce a review, and cannot be a CI gate whose verdict is
  stable across a week — a large part of the open non-determinism blocker.
- **To find out what actually runs today.** `quorum review --json` writes
  `.quorum/reviews/<ISO>.json`; `model_names` there is the roster Lippa used,
  and `supported_by` per finding is the attribution. There are no archives in
  the working copy, so the honest answer is: run one review and read the file.

---

## 3. Upstream contract — `M-ConsensusFindings`

To be filed as a spec in the Lippa repo. The never-touch-Lippa rule holds:
this is a contract request, not code.

Proposed `findings[]` on `GET /api/v1/consensus/sessions/{id}`, alongside the
existing debate clusters rather than replacing them:

```json
{
  "id": "f_01",
  "file": "src/lint/rules.ts",
  "line_start": 42,
  "line_end": 47,
  "severity": "high",
  "title": "...",
  "explanation": "...",
  "evidence": ["eslint.config.js:12"],
  "suggested_patch": "@@ -42,3 +42,3 @@ ...",
  "supported_by": ["claude-sonnet", "gpt-4o"],
  "dissent": [{"model": "gemini-pro", "reason": "..."}],
  "agreement": 0.67,
  "confidence": 0.41
}
```

Contract notes:

- `agreement` and `confidence` are two numbers and are never conflated.
  `agreement` is a count of supporting models; `confidence` is the model's own
  belief in the finding.
- `file` is repo-relative and MUST exist in the submitted bundle. Quorum
  validates this client-side and drops findings that fail.
- `evidence` closes the eslint-config class of error: a model required to cite
  what it read stops inferring.
- `severity` is model-assigned, not derived from agreement.
- A stable `id` anchored to a path is the identity hash Phase 1D needs.
- Per item 8: `supported_by` and `dissent[].model` carry canonical versioned
  identifiers in a single namespace, matching session-level `models[]` so the
  two can be joined; and the create call accepts `model_roles` as a real
  selector so a CI gate can pin its roster.
- `M-ConsensusFindings` should also carry the open non-determinism blocker
  (four runs on identical input returning disjoint finding sets) — anchored
  findings make that measurable for the first time.

---

## 4. Sequence

1. **v0.4 — "pre-push grade".** Items 2, 3, 4, 6, the confidence rename from 5,
   the CI guard from 7, the identity-hash change from 8. ≈8 working days. No upstream dependency. Every item is
   prerequisite work for anchors anyway: anchored findings are worthless if the
   bundle never contained the file they point at.
2. **Gate: Lippa `M-ConsensusFindings`.** Unblocks items 1 and 5 and Phase 1D
   identity hashes.
3. **v0.5 — "CI grade".** Anchored findings end to end: widened `Finding`,
   SARIF + JSON output, client-side anchor validation, a GitHub Action that
   posts inline. Starts the day the schema lands.
4. **Parallel: Lippa `M-ExternalAuth` Phase 1.** Bearer tokens. Independent of
   the schema work. Until it lands, Quorum should not run in anyone's CI
   regardless of the other items.

---

## 5. Response to the reviewer

Agree with their call. Step 2 with Gemini is the right move now — Quorum-as-CI
is one client milestone plus two upstream milestones away, and promising
otherwise would be selling a date we do not control. The local pre-push gate is
the position Quorum holds today, and v0.4 improves it at exactly the points
they hit.

Worth reflecting back: their Step 2 design — an independent pass, then a
cross-check against the first reviewer, with anything both flagged tagged
"flagged by both" — is multi-model consensus, hand-rolled in CI across two
vendors. That is Quorum's thesis, reached independently. When the schema lands,
they already have the shape.
