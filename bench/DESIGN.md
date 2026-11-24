# Benchmark pilot: frozen design (v0.6 Stage 1)

**Status: frozen at Gate B (Stage 1a), awaiting Rolf's approval of the
design and the budget.** No paid call has been made. Changing anything
this file pins, after the first paid run, restarts the pilot (spec §3).
Every result is labelled **pilot — exploratory** (AC 243).

`bench/tests/freeze.rs` fails if any pinned file's SHA-256 below stops
matching, if `MANIFEST.toml` is stale, or if the frozen settings drift.

## 1. Frozen files

| File | SHA-256 | Version |
|---|---|---|
| `bench/MANIFEST.toml` | c83f1cb10d8f2985f3508f6ddfb78eb2c943be7abb41dbb09910e2f8c4ab7bde | corpus of 40 cases |
| `bench/bench.toml` | da534240084eb57ffa7882582b77ef19e7ac9726af8295de0b5e88cf09beca5e | run settings |
| `bench/prices.toml` | a34ce5840ac2d95d545864f3714c918eb90a518ffc50faee8fdfafb1cf4cd173 | prices read 2026-11-23 |
| `bench/prompts/single-v1.md` | 899b51fdf38830f417e286784c48363518f3c5d1ac6da2d655fe86e2892c04e4 | single-model prompt v1 |
| `bench/RUBRIC.md` | f5ac0550f9cc7bfba78e51de35d76a1f67c0a7582945ebe5574c90aed9c15a32 | judging rubric v1 |
| `bench/rubrics/extraction-v1.md` | ada34c3209da94a0568a459faa39c4f1da6c737781df46674f94f38a19c902e9 | extraction rubric v1 |
| `bench/rubrics/clustering-v1.md` | 5e25edc3e907eb53a01e42a3f7bb44e6d93c8bd0378a126597f3417041ff200b | clustering rubric v1 |

The corpus manifest hash (SHA-256 of the `MANIFEST.toml` text) is the one
in the first row. The harness checks it before a run and records it in
every run manifest.

## 2. Corpus (ACs 242–246)

| Language | Bug | Clean | Total |
|---|---|---|---|
| python | 8 | 3 | 11 |
| rust | 14 | 4 | 18 |
| typescript | 8 | 3 | 11 |
| **all** | **30** | **10** | **40** |

- Reproduced bug cases: 30 of 30.
- Cases with AGENTS.md or CLAUDE.md at the base: 33 (AC 242 needs 8).
- Expected defects: 35 (7 high severity).
- Source repositories (bootstrap clusters): 20.
- Licences: Apache-2.0 6, MIT 21, MIT OR Apache-2.0 13.
- Sources: SWE-bench-Live/MultiLang 22, hand-picked 18.

| Case | Kind | Language | Repository | Licence | Agent file | Reproduced | Defects (high) |
|---|---|---|---|---|---|---|---|
| `clean-py-agentspy-01` | clean | python | openai/openai-agents-python | MIT | yes | - | - |
| `clean-py-mcp-01` | clean | python | modelcontextprotocol/python-sdk | MIT | yes | - | - |
| `clean-py-pydanticai-02` | clean | python | pydantic/pydantic-ai | MIT | yes | - | - |
| `clean-rs-harper-01` | clean | rust | Automattic/harper | Apache-2.0 | - | - | - |
| `clean-rs-harper-02` | clean | rust | Automattic/harper | Apache-2.0 | - | - | - |
| `clean-rs-uv-01` | clean | rust | astral-sh/uv | MIT OR Apache-2.0 | - | - | - |
| `clean-rs-uv-02` | clean | rust | astral-sh/uv | MIT OR Apache-2.0 | - | - | - |
| `clean-ts-excalidraw-02` | clean | typescript | excalidraw/excalidraw | MIT | yes | - | - |
| `clean-ts-workers-01` | clean | typescript | cloudflare/workers-sdk | MIT OR Apache-2.0 | - | - | - |
| `clean-ts-workers-02` | clean | typescript | cloudflare/workers-sdk | MIT OR Apache-2.0 | - | - | - |
| `py-agents-01` | bug | python | openai/openai-agents-python | MIT | yes | yes | 1 (0) |
| `py-agents-02` | bug | python | openai/openai-agents-python | MIT | yes | yes | 1 (0) |
| `py-claudesdk-02` | bug | python | anthropics/claude-agent-sdk-python | MIT | yes | yes | 1 (0) |
| `py-claudesdk-03` | bug | python | anthropics/claude-agent-sdk-python | MIT | yes | yes | 1 (0) |
| `py-mcp-01` | bug | python | modelcontextprotocol/python-sdk | MIT | yes | yes | 2 (0) |
| `py-mcp-02` | bug | python | modelcontextprotocol/python-sdk | MIT | yes | yes | 1 (0) |
| `py-pydanticai-01` | bug | python | pydantic/pydantic-ai | MIT | yes | yes | 1 (0) |
| `py-pydanticai-02` | bug | python | pydantic/pydantic-ai | MIT | yes | yes | 1 (0) |
| `rs-biome-01` | bug | rust | biomejs/biome | MIT OR Apache-2.0 | yes | yes | 2 (0) |
| `rs-biome-02` | bug | rust | biomejs/biome | MIT OR Apache-2.0 | yes | yes | 3 (0) |
| `rs-biome-03` | bug | rust | biomejs/biome | MIT OR Apache-2.0 | yes | yes | 1 (0) |
| `rs-biome-04` | bug | rust | biomejs/biome | MIT OR Apache-2.0 | yes | yes | 1 (0) |
| `rs-biome-05` | bug | rust | biomejs/biome | MIT OR Apache-2.0 | yes | yes | 1 (0) |
| `rs-dioxus-01` | bug | rust | DioxusLabs/dioxus | MIT OR Apache-2.0 | yes | yes | 1 (0) |
| `rs-dioxus-02` | bug | rust | DioxusLabs/dioxus | MIT OR Apache-2.0 | yes | yes | 1 (0) |
| `rs-gitoxide-01` | bug | rust | GitoxideLabs/gitoxide | MIT OR Apache-2.0 | - | yes | 1 (0) |
| `rs-leanctx-01` | bug | rust | yvgude/lean-ctx | Apache-2.0 | yes | yes | 1 (1) |
| `rs-leanctx-02` | bug | rust | yvgude/lean-ctx | Apache-2.0 | yes | yes | 1 (0) |
| `rs-obscura-01` | bug | rust | h4ckf0r0day/obscura | Apache-2.0 | yes | yes | 1 (0) |
| `rs-oxc-02` | bug | rust | oxc-project/oxc | MIT | yes | yes | 1 (0) |
| `rs-oxc-03` | bug | rust | oxc-project/oxc | MIT | yes | yes | 1 (1) |
| `rs-pnpm-01` | bug | rust | pnpm/pnpm | MIT | yes | yes | 1 (0) |
| `ts-agentsjs-01` | bug | typescript | openai/openai-agents-js | MIT | yes | yes | 1 (0) |
| `ts-agentsjs-02` | bug | typescript | openai/openai-agents-js | MIT | yes | yes | 1 (1) |
| `ts-agentsjs-03` | bug | typescript | openai/openai-agents-js | MIT | yes | yes | 1 (0) |
| `ts-defuddle-01` | bug | typescript | kepano/defuddle | MIT | yes | yes | 1 (0) |
| `ts-hydrogen-01` | bug | typescript | Shopify/hydrogen | MIT | yes | yes | 1 (1) |
| `ts-manifest-01` | bug | typescript | mnfst/manifest | MIT | yes | yes | 1 (1) |
| `ts-qwen-01` | bug | typescript | QwenLM/qwen-code | Apache-2.0 | yes | yes | 1 (1) |
| `ts-workers-01` | bug | typescript | cloudflare/workers-sdk | MIT OR Apache-2.0 | yes | yes | 2 (1) |

The 40 cases are the 30 bug and 10 clean cases AC 243 asks for. All 30
bug cases reproduced in Docker: the test fails on the case commit and
passes on the fixed tree. Each case's command, image digest and outputs
are in its `repro/` directory and `case.toml`.

Sources (D3, Gate A ruling 1):
- **SWE-bench-Live/MultiLang** (MIT dataset, tasks from 2025-06 to
  2026-07) supplied the Rust bug cases, and all TypeScript ones except
  where noted.
- **Hand-picked 2026 fixes** supplied the Python bug cases, from
  pydantic-ai, openai-agents-python, the MCP python-sdk and
  claude-agent-sdk-python.
- **Hand-picked commits from before 2025-11-23** supplied the clean
  cases. Each one has a recorded blame and history check (`[clean]`).

Every licence was read at the case base commit.

Only 7 of the 35 expected defects are high severity, so
**recall (high) is very noisy** and is descriptive only (AC 254).

**Case notes:**
- **`rs-pnpm-01`** (Gate B ruling 1: kept as a Rust case). The fix
  changes both the Rust port and one TypeScript resolver file, so
  `diff.patch` includes one `.ts` file. Only the Rust test reproduces
  the bug. This is recorded in the case's `notes`.
- **`rs-gitoxide-01`** (Gate B ruling 3). Its licence, `MIT OR
  Apache-2.0`, is taken from the `license` field of
  `gix-merge/Cargo.toml`, the changed crate's manifest, which matches
  the root manifest. `licence_path` points there. No copyright line is
  needed.
- **`rs-dioxus-01`, `rs-dioxus-02`:** the licence files have no copyright
  line. `licence_notice` says so.
- **`rs-leanctx-02`:** the fix commit also updated an unrelated lock
  table, `rust/LOCK_ORDERING.md`. It is kept at its fixed version in
  both trees, so `diff.patch` reverts only the code.
- **`ts-hydrogen-01`:** `npm ci` rejects the lockfile at this commit,
  so its repro uses `npm install`, and its dependencies are not fully
  pinned.
- **Missing test-module files:** some Rust cases keep a `mod tests;` or
  similar line in source that points at a removed test file. Only test
  builds are affected.
- **Symlinks:** in pydantic-ai, openai-agents-python and pnpm,
  `CLAUDE.md` is a symlink to `AGENTS.md`. On a Windows checkout without
  symlink support, it becomes a one-line file, and Quorum reads that
  line.

### Edited cases: the fix's explanatory comments removed (Gate B ruling 2)

A comment the fix added shows up in the reverted diff as a deleted line
explaining the answer. It leaks the answer to every arm and inflates
recall. `verify-corpus` therefore fails any bug case whose `diff.patch`
removes a comment-only line: `//`, `/* */`, `///` and `#` styles, chosen
by file extension.

`quorum-bench strip-comment-hints` deleted those lines from the case
base file, stored as `base/<path>` and listed in `base_overrides`. It
recomputed `case_base_tree` and regenerated `diff.patch` from the edited
base to the unchanged case commit tree. Each case records `edited =
true` and an `edit_reason`. No edit could unbalance a `/* */` block, and
none was refused. The edited lines therefore appear neither in
`diff.patch` nor in the materialized base.

| Case | Lines removed | File(s) |
|---|---|---|
| `py-agents-02` | 1 | `src/agents/sandbox/memory/storage.py` |
| `py-claudesdk-03` | 4 | `src/claude_agent_sdk/_internal/transport/subprocess_cli.py` |
| `py-pydanticai-01` | 1 | `pydantic_ai_slim/pydantic_ai/models/test.py` |
| `rs-biome-01` | 15 | `crates/biome_html_analyze/src/lint/correctness/use_vue_valid_v_bind.rs` (rule-doc prose and the `vue` example) |
| `rs-biome-02` | 26 | `crates/biome_html_analyze/src/lint/correctness/use_vue_valid_v_on.rs` (rule-doc prose and the `vue` example) |
| `rs-biome-04` | 3 | `crates/biome_js_formatter/src/js/expressions/call_arguments.rs` |
| `rs-biome-05` | 2 | `crates/biome_service/src/file_handlers/html/parse_embedded_nodes.rs` |
| `rs-dioxus-01` | 2 | `packages/autofmt/src/writer.rs` |
| `rs-dioxus-02` | 4 | `packages/autofmt/src/prettier_please.rs` |
| `rs-leanctx-01` | 23 | `rust/src/core/shell_allowlist/mod.rs` |
| `rs-obscura-01` | 8 | `crates/obscura-cdp/src/domains/page.rs` |
| `rs-oxc-03` | 1 | `crates/oxc_minifier/src/peephole/remove_unused_expression.rs` |
| `rs-pnpm-01` | 6 | `pnpm/crates/resolving-npm-resolver/src/errors.rs`, `.../fetch_full_metadata.rs` |
| `ts-agentsjs-02` | 19 | `packages/agents-realtime/src/realtimeSession.ts` |
| `ts-defuddle-01` | 1 | `src/standardize.ts` |
| `ts-hydrogen-01` | 2 | `packages/hydrogen-react/src/parse-metafield.ts` |
| `ts-manifest-01` | 4 | `packages/backend/src/routing/proxy/provider-client.ts` |
| `ts-qwen-01` | 6 | `packages/cli/src/ui/hooks/useStatusLine.ts` |
| `ts-workers-01` | 5 | `packages/miniflare/src/workers/d1/dumpSql.ts` |

That is 19 of the 30 bug cases, 133 lines in all.

- **Clean cases are reported, not edited (middle category).** The check
  also lists comment-only removed lines in `clean-py-mcp-01` (1) and
  `clean-rs-harper-01` (4). In a clean case the removed side is the
  original code before an ordinary commit, which the commit itself
  rewrote. It explains no fix and points at no defect, so editing it
  would only change the commit under review. `verify-corpus` prints
  them as `note` lines.
- **What stays.** A comment sharing a line with code is not
  comment-only, and it stays. Python docstrings are strings, not
  comments, and also stay. Both can still carry a hint. A scan of every
  bug case's removed lines for code with a trailing comment found two,
  left as they are because the ruling covers comment-only lines. Rolf
  may rule on them:
  - `rs-leanctx-01`: `b'\\' => i += 1, // escaped brace is data, not a
    group delimiter`. This partly explains the fix.
  - `ts-qwen-01`: `if (gen !== generationRef.current) return; // stale`.
    This is a one-word label.

The AC 244 leakage check and the dry run were re-run on all 40 cases
after the edits (§2).

### Excluded cases (the bundle budget cut them)

AC 243 requires every case to fit Quorum's default bundle budget with no
cut. At Quorum `4f4ec22` that budget is `total_budget_kb = 200`, at v0.3.3
proportions:
- diff 100 KB;
- changed and context files 80 KB, every file sent whole with a
  `{:>6} | ` line-number gutter;
- memory 20 KB;
- conventions 10 KB.

The zero-cost dry run (`quorum-bench verify-corpus --quorum-bin …`)
found eight built cases where Quorum left a changed file out. They were
dropped and replaced by cases that fit, and Rolf approved the deletions
on 2026-11-24.

| Case | Kind | Repository | Source commit | File(s) the budget cut (size at the case commit) | Changed non-test files (total size) |
|---|---|---|---|---|---|
| `clean-py-pydanticai-01` | clean | pydantic/pydantic-ai | `493f42c05832` | `pydantic_ai_slim/pydantic_ai/toolsets/function.py` (16,819 B) | 2 files, 83,930 B |
| `clean-rs-ruff-01` | clean | astral-sh/ruff | `864196b988a4` | `crates/ruff_linter/src/checkers/ast/mod.rs` (133,507 B) | 2 files, 146,440 B |
| `clean-ts-excalidraw-01` | clean | excalidraw/excalidraw | `864353be5ff8` | `packages/element/src/sizeHelpers.ts` (6,851 B; the other changed files took the section) | 3 files, 65,108 B before the gutter |
| `py-claudesdk-01` | bug | anthropics/claude-agent-sdk-python | `6b1927e5126a` | `src/claude_agent_sdk/types.py` (107,482 B) | 2 files, 163,025 B |
| `rs-oxc-01` | bug | oxc-project/oxc | `f9ef1bdb110d` | `crates/oxc_formatter/src/print/call_like_expression/arguments.rs` (48,363 B) | 2 files, 71,013 B before the gutter |
| `rs-oxc-04` | bug | oxc-project/oxc | `1a370a65c8e8` | `crates/oxc_minifier/src/peephole/minimize_statements.rs` (84,316 B) | 1 file, 84,316 B |
| `rs-uv-01` | bug | astral-sh/uv | `d32d51bc2f3b` | `crates/uv/src/commands/pip/operations.rs` (37,381 B); `crates/uv/src/commands/diagnostics.rs` (16,342 B) | 3 files, 102,351 B |
| `ts-excalidraw-01` | bug | excalidraw/excalidraw | `416da62138ff` | `packages/excalidraw/components/App.tsx` (354,540 B); `packages/element/src/linearElementEditor.ts` (57,516 B) | 3 files, 421,534 B |

**This biases the corpus toward small changes.** Every included case's
changed files fit together in the 80 KB file section. Fixes and changes
inside large files (for example App.tsx at 355 KB, ruff's AST checker at
134 KB, oxc's minifier at 84 KB) cannot appear. The pilot therefore says
nothing about reviews of large files, and `RESULTS.md` must say so. The
replacement briefs added a size rule, so the replacements were chosen
small on purpose: changed files at most 55,000 bytes in total and
40,000 bytes each.

The Stage 2 budget rebalance (spec §5.3: 80/72/16/24/6) changes the
section sizes. The Stage 2 and 3 re-runs must repeat the dry run, and any
case cut then is reported as a drift caveat, not silently scored.

## 3. Arms (ACs 242a, 248, 248a, 249; [S1a-ADJ])

All arms in one case-repeat are fed the same bundle bytes. Each arm runs
every case three times (AC 249).

| Arm | What runs | Findings |
|---|---|---|
| `Q-high`, `Q-medium`, `Q-low` | `quorum review --json --range <case-base>..<case-commit> --tier <t>`. The binary is built from Quorum `4f4ec225c8b3` and runs through the recording proxy. | The archive's findings, in order |
| `S-gpt-6.2-sol`, `S-claude-opus-5-5`, `S-gemini-4-carbon` | The exact bundle bytes of the same repeat's `Q-high` session, sent through each vendor's API inside `single-v1.md` | The claims block: at most 5 claims of at most 20 words. Without a block, the extraction rubric |
| `E-union`, `E-majority` | Each seat's Round 0 `raw_response` from the same `Q-high` session, after extraction and clustering ([S1a-ADJ]) | Every cluster, or only the clusters raised by two or more seats |

- **The panel** is fixed by the run's first `Q-high` session. It is the
  seat profile id mapped to the observed model: `answered_model_id` when
  a fallback answered, else the profile's model. The archive records
  vendors only, which is a BACKLOG item. A `Q-high` session reporting
  another panel makes that case-repeat **uncontrolled**. It is excluded
  from C1 and C2 and reported separately (AC 248).
- **The `S-` vendor ids** are pinned to the ids `Q-high` observes (Gate A
  ruling 4). The `[s_arms.models]` table in `bench.toml` maps each Lippa
  id to the identical vendor id. Run 1b's first step confirms each id
  with one minimal call.
- **`S-` settings** are the panel seats' high-tier settings, from Lippa
  `effort_tiers.py` at `3cd425b8`:
  - `gpt-6.2-sol`: effort `medium`;
  - `claude-opus-5-5`: effort `low`;
  - `gemini-4-carbon`: effort `medium`;
  - a 16,384-token output ceiling for all three.

  **C1 is therefore scoped to "Quorum vs. the best single panel model at
  panel settings"**, and `RESULTS.md` will say so.
- **`E-` arms** need all three Round 0 seats healthy (no error text and a
  non-empty answer). Otherwise the case-repeat is excluded from C2 and
  reported. With [S1a-ADJ], a model without an `S-` mapping affects only
  C1.
- **Retries and errors.** A `Q-` review is retried once on error, and so
  is every paid direct call (AC 250). A review still in error after its
  retry scores zero findings: every expected defect is a miss, and
  errors are counted per arm (AC 250a). A spend refusal, meaning
  Lippa's 402 or Quorum's future spend guard, halts the run unscored
  (AC 242a). Quorum 0.5.x has no spend guard; when Stage 6 adds one,
  `quorum_run::classify` needs its refusal line.

### The single-model prompt (Gate A ruling 4)

`bench/prompts/single-v1.md` is Lippa's Round 0 seat wrapper, copied from
Lippa `3cd425b898a24f70b05b087759dd423f98196a37`. It has three parts:
- the safety block `2026-10-30.1`;
- the standard-mode `base_system`;
- `claims_format`.

The user message is `Question:\n<bundle>`, exactly as Lippa builds it.
The strings were extracted with `ast.literal_eval`, not retyped. The only
change is removing the sentences that tell the model it is one of
several:

```diff
-You are participating in a structured multi-model consensus process. Multiple independent AI models are answering the same question separately. Do not assume what other models will say. Provide your independent assessment.
+Provide your independent assessment.
```

Quorum's own bundle never refers to other reviewers (Recon-v06-1,
`bundle.rs:180`).

## 4. Stripped format (AC 252)

Every arm's findings become `{title, severity, body}` with:
- the title cut to **20 words**;
- `severity = "unrated"`;
- `body = ""`.

**Decision (halt rule, middle category):** AC 252 lists severity in the
stripped format and also requires agreement to be removed. Quorum's
severity is a function of agreement: a divergence is `high`, strong
agreement is `medium`, the rest `low`. Its body is only "Confidence: …
Supported by: …". Showing either would reveal agreement, and no arm has
a model-assigned severity before Lippa's `M-ConsensusFindings`. So the
field is kept with one value for every arm.

`recall (high)` (AC 253) uses the **expected defect's** severity from
`expected.toml`.

## 5. Judge (AC 252, Gate A ruling 2)

**Chosen: xAI `grok-4.7`**, with strict JSON schema, `reasoning_effort =
high`, `temperature = 0` and `seed = 7`, through
`https://api.x.ai/v1/chat/completions` with key `XAI_API_KEY`.

Mistral `mistral-large-4-0` was checked first, against Mistral's own
documentation. It failed:

| Check | Result | Source |
|---|---|---|
| `-0` is a pinned snapshot | **Pass on naming**: `model-name-major-minor` is "Fixed major and minor version", and GA models get no silent updates. **Conflict**: the changelog's last entry calls Large 4 "Public Preview", where silent updates are allowed, while the model page says GA | docs.mistral.ai/inference/model-lifecycle, /models/mistral-large-4-0, /resources/changelogs |
| Strict structured outputs | **Not verified**: `json_schema` mode is documented, but what `strict` does is not described | docs.mistral.ai/openapi.yaml (`ResponseFormat`, `JsonSchema`) |
| API data not used for training | **Fail**: Commercial Terms §4.2 allow training unless the customer opts out, and opt-out does not apply to Preview models | legal.mistral.ai/terms/commercial-terms-of-service §4.2–4.3; help.mistral.ai article 347617 |

The ruling said any failure moves the judge to xAI.

On xAI:
- **Structured outputs:** "the response is guaranteed to match your
  schema" for supported schema features
  (docs.x.ai/docs/guides/structured-outputs).
- **Data policy:** "never trains on your API inputs or outputs without
  your explicit permission" (docs.x.ai/developers/faq/security).

`grok-4.7` is an alias with no dated snapshot. So every judge,
extraction and clustering response records its `model` string and
`system_fingerprint` in the run manifest (`judge_models_returned`). A
change between runs is flagged as drift. The same model does extraction
and clustering (AC 248a). It is outside the observed panel, which is
OpenAI, Anthropic and Google.

## 6. Rubrics and classes

| Rubric | Job | Sees `expected.toml` |
|---|---|---|
| `bench/rubrics/extraction-v1.md` | Round 0 prose to at most 5 claims of at most 20 words ([S1a-ADJ]) | no |
| `bench/rubrics/clustering-v1.md` | Three seats' findings grouped by underlying concern (AC 248a) | no |
| `bench/RUBRIC.md` | Every unique stripped finding of a case against its expected defects (AC 252) | yes |

All case content reaches the model between `<<<BENCH_*_BEGIN>>>` and
`<<<BENCH_*_END>>>` markers, with an instruction never to follow it.
Findings appear in a seeded shuffle under opaque ids.

**Classes:** `match`, `false_alarm`, `pre_existing`, `unexpected_tp`,
`unresolved`, and `duplicate`. Scoring derives `duplicate`, one-to-one
per expected defect within an arm and repeat.

**Proposed, needs Rolf's ruling — [S1a-ADJ-2], a `no_issue` class.**
Quorum's findings are Lippa claims, and claims include positions such as
"The change correctly handles empty input". The `S-` and `E-` arms use
the same claims format. Under AC 252 as written, every unmatched finding
that is not a duplicate, pre-existing or an unexpected true positive is
a false alarm. A correct approval of a clean case would then count as an
FP for every arm, and "false alarms per clean case" would mostly count
approvals.

I added `no_issue` ("the finding asserts no problem: an approval, a
description, a question"). It is neither TP nor FP and is reported
separately. If Rolf prefers AC 252 as written, remove the class from
`RUBRIC.md` and the schema before the freeze. That needs no paid
re-run.

## 7. Primary endpoint and statistics (AC 253, AC 254)

- **Micro F1** = 2·TP / (2·TP + FP + FN). TP, FP and FN are averaged per
  case over its repeats, then summed over cases. FP includes every
  adjudicated false alarm, in bug and clean cases. Each arm averages
  over the repeats every compared arm has.
- **Bootstrap.** Paired, 10,000 draws, seed `20261124`. Each draw
  resamples source repositories with replacement, then the cases within
  each drawn repository. The best `S-` arm is re-chosen in every draw.
  The interval is the 2.5th–97.5th percentile, with linear interpolation.
- **Labels:**
  - *supported*: the lower bound is above 0;
  - *contradicted*: the upper bound is below 0;
  - *inconclusive*: the interval contains 0;
  - *not tested*: fewer than 2 repositories, or F1 undefined in more
    than 5% of draws.
- **Undefined draws (middle category).** In a draw where some arm has
  2·TP + FP + FN = 0, F1 is undefined; a clean-only resample is the
  typical case. That draw's difference is counted and left out.
- **C1** = F1(`Q-high`) − F1(best `S-`), on controlled case-repeats.
- **C2** compares `Q-high` with `E-majority` and with `E-union`, on
  controlled case-repeats with three healthy Round 0 seats. It is
  supported only if both differences are supported. It is a **direct**
  test ([S1a-ADJ], D7).
- **Descriptive only:**
  - recall, overall and for high-severity defects;
  - precision;
  - false alarms per clean case;
  - stability (defects found in all three repeats);
  - median latency;
  - cost per review;
  - error counts;
  - duplicates, `pre_existing`, `unexpected_tp` and `no_issue` counts.
- **Re-runs.** The `Q-high` re-runs after Stages 2 and 3 are one repeat
  each. They are reported only against `Q-high`'s Stage 1 baseline, with
  drift caveats (AC 251, 253).

## 8. Output caps

| Call | Cap (tokens, reasoning included) |
|---|---|
| `S-` arms | 16,384 (the panel seats' ceiling) |
| Extraction | 8,192 |
| Clustering | 16,384 |
| Judging (batches of 40 findings) | 16,384 |

## 9. Price table (`bench/prices.toml`, read 2026-11-23)

The table lists USD per 1M tokens. Reasoning and thinking tokens bill as
output. Cached-input discounts are ignored, so estimates can only
overstate.

| Model | Input | Output | Note |
|---|---|---|---|
| `gpt-6.2-sol` | 2.00 | 10.00 | 4.00 / 15.00 above 272K input |
| `claude-opus-5-5` | 4.00 | 20.00 | |
| `gemini-4-carbon` | 4.00 | 20.00 | Google's price from 2026-12-01 (it doubles); the pilot runs after that date |
| `grok-4.7` (judge) | 2.00 | 6.00 | Every token doubles at 200K+ prompt tokens |
| Fallbacks: `gpt-6-luna`, `claude-sonnet-5-5`, `gemini-4-hydrogen` | 0.10 / 2.00 / 1.50 | 0.50 / 10.00 / 7.50 | So that any observed id has a price |
| `mistral-large-4-0` | 0.70 | 2.10 | Not used (§5) |

## 10. Budget for approval (AC 250)

**Lippa:** 360 sessions for the base run (40 cases × 3 tiers × 3
repeats) at 3 credits each is **1,080 credits**. The two `Q-high`
re-runs (40 sessions each) add **240 credits**. **Total ≈ 1,320
credits.** `--max-credits 1100` covers the base run with room for
retried sessions; each re-run gets its own cap.

**Direct API** (the `S-` arms, extraction, clustering and judging),
estimated from the real dry-run bundle sizes:

The estimate uses these assumptions:
- the mean dry-run bundle is 56,296 (largest 90,995) bytes, about 15k tokens at 3.5
  bytes per token;
- a typical call produces 4k output tokens for `S-`, 2.5k for
  extraction, 4k for clustering, and 6k per judging batch of 40, all
  including reasoning;
- the worst case bills every reservation in full: every prompt byte as
  one token, plus the full output cap.

| Item | Calls | Typical | Worst case |
|---|---|---|---|
| `S-` arms (3 models × 40 cases × 3 repeats) | 360 | $44.64 | $170.85 |
| Round 0 extraction (3 seats × 120 case-repeats) | 360 | $6.84 | $22.92 |
| Clustering (one per case-repeat) | 120 | $3.43 | $13.78 |
| Judging (about 2 batches per case, plus 1 per case for each of the 2 re-runs) | 160 | $6.86 | $19.65 |
| **Total** | 1,000 | **about $62** | **about $227** |

**Requested cap: `--max-usd 300`** (Gate A ruling 7). It covers the
worst case, in which every reservation is billed in full.

## 11. Human adjudication workload (AC 252, AC 248a, [S1a-ADJ])

Everything is blind: no arm, model, seat or agreement is shown.
Identical stripped findings of a case share one decision, and the copy
count is never shown. Estimate:

| Queue | How chosen | Expected items | Time each | Time |
|---|---|---|---|---|
| Findings: uncertain or unresolved judge proposals, plus every `unexpected_tp` | all | 400–600 | about 20 s | 2–3.5 h |
| Findings: the rest | a seeded random 20% | 650–700 | about 20 s | 3.5–4 h |
| Clusterings (one per case-repeat) | 20% of 120 | 24 | about 1 min | 0.4 h |
| Extractions (one per Round 0 seat) | 20% of about 360 | about 72 | about 1.5 min | 1.8 h |
| **Total** | | **about 1,150–1,400** | | **about 8–10 h** |

The basis is about 41 findings per case-repeat:
- `Q-` about 6 × 3 tiers;
- `S-` about 4 × 3 models;
- `E-union` about 8 and `E-majority` about 3.

That is about 4,900 raw findings, of which about 80% are unique per
case after de-duplication, so about 3,900 judge decisions. The
mandatory share is a guess; the run reports the real count before
adjudication starts. The tool resumes where it stopped, so the work can
be split across sessions.

## 12. Safety and release (AC 247)

- **Keys** come from the environment or the gitignored `bench/.env.local`.
  The Lippa cookie comes only from `QUORUM_LIPPA_SESSION`. The key guard
  runs before every command (`keyguard.rs`, with a negative control on
  this repository).
- **The recording proxy** binds to `127.0.0.1` only and records bodies,
  never headers. Its recordings never contain the cookie: the test
  `proxy::tests::recordings_hold_bodies_only_and_never_the_cookie`.
- **One scrubber** handles manifests, recordings and the archive:
  cookies, tokens, presigned URLs and `Authorization` headers.
- **The bench project must stay empty.** Lippa seeds every
  project-linked session with the project's confirmed and pinned
  memory. `GET /api/v1/projects/{id}/memory` exists (Lippa
  `routes/api_v1/project_detail.py:349`), and its `total` counts
  confirmed and pinned, active or challenged, unarchived items, a
  superset of what seeding reads. **The 1b preflight refuses to run
  unless `total` is 0** (`preflight.rs`). No manual check is needed.
- **Release trigger.** `release.yml`'s only tag filter is
  `'**[0-9]+.[0-9]+.[0-9]+*'`. In GitHub filter syntax `.` is a literal,
  so a tag matches only if it holds two literal dots between digit runs.
  Run ids are `YYYYMMDDTHHMMSSZ-<6 hex>`, with no dot, so no
  `bench-<run-id>` tag can match. `runid::tests::release_tag_filter_never_matches_a_bench_tag`
  reads the real workflow file and checks this. Its positive controls,
  `v0.5.1` and `quorum-cli-v0.5.1`, match; a dotted `bench-2026.11.24`
  matches too, which is why ids are dot-free. `publish-crates.yml`
  publishes three named crates. `bench` is `publish = false` and
  `dist = false`.
- **Dependencies.** `hyper`, `hyper-util`, `http-body-util`, `flate2` and
  `tar` are bench-only. The `cargo tree` output for `quorum-cli`,
  `quorum-core` and `quorum-lippa-client` is unchanged.

## 13. Run 1b procedure

1. Rolf creates the bench Lippa project, leaves it empty, and puts its
   id in `bench.toml` `[lippa] project_id`. That is the one edit allowed
   after the freeze.
2. Build the binary under test:
   `cargo install --path crates/quorum-cli --locked --root bench/.cache/quorum-4f4ec22`
   from a checkout of `4f4ec225c8b3`.
3. Set `QUORUM_LIPPA_SESSION`, `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`,
   `GEMINI_API_KEY` and `XAI_API_KEY` in the environment or in
   `bench/.env.local`. The cookie can only come from the environment.
4. Smoke-check each provider with one minimal call. The request shapes
   in `vendors.rs` follow the vendors' references as read on 2026-11-23
   and have not been exercised live.
5. Run `quorum-bench run --max-credits 1100 --max-usd 300 --quorum-bin <bin> --confirm-paid`.
   The preflight checks the keys, the empty project and a fresh
   manifest. The run writes `bench/runs/<run-id>/` (gitignored) and the
   blind queue.
6. Rolf runs `quorum-bench adjudicate --run <run-id>` until no item is
   left, then `quorum-bench results --run <run-id>`, which writes
   `results.json`. `RESULTS.md`, with the per-case scores, is committed
   whatever the results show (AC 255).
7. Optionally `quorum-bench archive --run <run-id>` (scrubbed, SHA-256
   printed). Rolf tags and releases by hand.

## 14. Decisions taken under the halt rule's middle category

Each was decided and documented here rather than halted on. Rolf may
overrule any before the freeze.

1. **Uniform severity in the stripped format** (§4).
2. **`duplicate` is derived, not judged.** The judge classifies each
   unique finding by its meaning. Scoring marks the second and later
   matches of one defect, within an arm and repeat, as duplicates, so
   "one-to-one per expected defect" (AC 252) holds by construction.
3. **Undefined F1 draws are excluded and counted.** Above 5%, the claim
   is labelled *not tested* (§7).
4. **The interval** uses linear interpolation between order statistics.
5. **The judge sees every unique finding of a case in one blind pool**,
   batched 40 per call and mixed across arms and repeats, rather than
   one arm at a time. Each finding gets one decision, which also does
   the de-duplication (Gate A ruling 7).
6. **Clustering items**, for the 20% human check, are whole case-repeat
   groupings. Extraction items are one seat's answer with its claims.
7. **What counts as a cut** (AC 243): a truncated diff, a changed file
   omitted for budget, or a changed file withheld by the deny list.
   Unchanged context files left out by the budget are allowed and
   counted (`verify-corpus` prints them).
8. **What counts as a leak** (AC 244):
   - a removed path, or a repro test file at its own path, in either
     case tree or the working directory;
   - an object no tree reaches;
   - a whole bundle line, at least 30 characters, that occurs in a repro
     test and in neither case tree.

   The same bytes at another, unremoved path, such as a fixture copied
   from an older one, are ordinary repository content.
9. **Corpus rule against hints:**
   - a fix whose test sits inside the fixed source file is not used;
   - changelog and changeset edits never appear in `diff.patch`.

   `bench/CORPUS.md` has the full rules.
10. **Case sources are fetched as GitHub tarballs**, proven by the pinned
    tree hashes, with the API's tree listing as the fallback. Quorum's
    git2 has no TLS transport, and its tech stack rules out shelling out
    to git (BACKLOG).
11. **The `no_issue` class**, [S1a-ADJ-2], is built in and proposed
    (§6). It needs Rolf's ruling. It adds to AC 252's class list, so it
    is the one item here that changes an AC rather than reading one.
