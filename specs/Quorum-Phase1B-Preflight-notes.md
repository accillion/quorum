# Quorum Phase 1B — Preflight Notes

**Status:** Recon complete; **HALT before Stage 1**. Awaiting Rolf's review of divergences.
**Date:** 2026-05-11.
**Spec under recon:** `specs/Quorum-Phase1B-Spec-v1_0.md`.
**Companion driver prompt:** `specs/Quorum-Phase1B-Drafting-Handoff.md` + the Phase 1B session-driver prompt.

This file follows the Phase 1A pattern (`specs/Quorum-Phase1A-Preflight-notes.md`): per-recon-item outcome, then a consolidated **Divergences** section CC will not silently absorb without Rolf's sign-off.

Probe artifacts (not committed; under `%TEMP%\quorum-1b-preflight\`):
- `recon-3-rusqlite\` — bundled-vs-baseline size + PE-imports
- `recon-8-panic\` — Drop-runs-during-unwind test
- `recon-12-13-drift-rs\` — 5 live Lippa session details + drift report
- `recon-4-tui\` — TUI dep build + TestBackend render log
- `recon-2-11-hooks\` — scratch repo + invocation log
- `recon-12-13-rs\` — Rust probes (`drift`, `login_probe`, `feedback_probe`)

Lippa repo verified untouched at session start AND session end (`git -C ../lippa status --short` returns empty).

---

## Per-item outcomes

### CC-Recon-1B-3 — rusqlite bundled — **PASS-WITH-DIVERGENCE (D1)**

Built two scratch crates under the workspace release profile (`lto = "thin"`, `codegen-units = 1`, `strip = "symbols"`, `opt-level = 3`):
- baseline (println-only): 125,440 bytes / 122.5 KB
- bundled (`rusqlite = { version = "0.32", features = ["bundled"] }`): 1,767,936 bytes / 1726.5 KB
- **delta: 1,642,496 bytes ≈ 1.60 MB** on `x86_64-pc-windows-msvc`

Static-link verification via PE-import-table parse (no `dumpbin` available on this host; parsed the import directory directly):
- DLL imports: `KERNEL32.dll`, `ntdll.dll`, `VCRUNTIME140.dll`, `dbghelp.dll`. No `sqlite3.dll` import; **bundled feature confirmed static**.
- Internal SQLite strings (`sqlite_master`, `sqlite_schema`, `sqlite_sequence`, `sqlite_stat1`, etc.) present in the binary — as expected from a static SQLite blob.

**D1 — Bundled rusqlite size delta is ~1.60 MB on Windows MSVC release, vs spec §4.0's "~700KB" estimate (≈2.3x larger).** This appears to be a Windows linker/strip artifact (Windows `strip = "symbols"` is less aggressive than Linux strip + `--gc-sections`). Linux release-stripped should land closer to the spec target; we'll know in Stage 5 when cargo-dist cross-compiles. **No action — single-binary preservation property holds; just an estimate update.** Suggest §4.0 footnote: "delta ≈700KB on Linux release-stripped; ≈1.6MB on Windows MSVC release per preflight measurement."

`rusqlite v0.32.1` was resolved by cargo (latest available is v0.39.0). The spec doesn't pin a version; Stage 1 should pin `rusqlite = "0.32"` in `quorum-core/Cargo.toml`. (0.39 may require a newer rust-version; we hold at 0.32 for predictability.)

### CC-Recon-1B-8 — panic=unwind for test profile — **PASS**

Built scratch crate (no explicit `[profile.test]`) with a `Drop`-guard-inside-`catch_unwind` test. Drop ran during unwind; the assertion held. Cargo's default `[profile.test]` is `panic = "unwind"`, which the workspace doesn't override — the spec §4.9 `tui_smoke.rs` terminal-restoration test is reachable as designed.

**Defensive Stage-2 add (no divergence):** before the terminal-restoration test is written, add an explicit `[profile.test] panic = "unwind"` to workspace `Cargo.toml` so a future `[profile.release] panic = "abort"` switch can't accidentally regress this. ~1 LOC.

### CC-Recon-1B-12 — Lippa stable finding IDs / file refs — **PASS-WITH-DIVERGENCE (D2)** — *P39 does NOT fire*

Ran 5 live sessions against the same trivial diff (one-hunk `print(1)→print(2)` patch). 5/5 converged. Captured cluster_ids across runs:

| Run | Cluster IDs |
|---|---|
| 1 | `351f5fb0`, `8207436f` |
| 2 | `93fe3d10` |
| 3 | `1dc95c42`, `57f0541b` |
| 4 | `4d04feb2`, `8028f473` |
| 5 | `83a2647c`, `c3d8e527`, `d52bc5ec`, `ecc6085b` |

- **Intersection across all runs: 0 ids.** **Union: 11 ids.** **Stability ratio: 0%.**
- `cluster_id` is **not a content-derived anchor** — appears to be session-scoped random 8-hex-char.
- The wire format exposes **no `file` field on clusters, no `line_range`, no `code_region`** (confirmed by Phase 1A fixture inspection AND by the 5 fresh detail responses).

**D2 — P39 conditional does NOT fire.** Neither anchor (`stable cluster_id` nor `file`) is available. The §4.2.4 design **stays with `body_opener` as the fourth hash input**, exactly as written. The "if yes, use the id directly" / "if file is non-None, file replaces body_opener" forks are both dead branches. Spec wording should drop the P39 conditional and lock in body_opener-based hashing.

**Additional D2 sub-finding (more consequential than the P39 question itself):** Phase 1A's `Finding.body` is **synthesized metadata**, not rich body prose. Specifically, `parse_clusters` in `crates/quorum-core/src/review.rs:206` constructs `body = "Confidence: 0.XX. Supported by: <models>."` — that's ~70-100 bytes of derived text. The spec §4.2.4 line 491 says "Phase 1A `Finding.body` already holds the markdown body, so no wire-format change is needed." **That sentence is factually wrong.** Lippa's wire format has `claim_text` (title) + `confidence` + `supported_by` + `cluster_id` per cluster; there is no rich cluster body in the response. The body_opener heuristic was designed to anchor on body prose; that prose doesn't exist.

Two ways forward, both substantial:
- **D2-A (recommended):** retarget `body_opener` to the first 280 normalized bytes of `claim_text` itself. This is essentially "hash the title twice with different normalization windows," which is weaker than designed but matches what's actually available. **Drift consequences in recon-13 below show even this is fragile.**
- **D2-B (alternative):** drop `body_opener` from the hash entirely and hash on `title` + `source.type` + `source.models` only. Same drift characteristics, fewer moving parts; documents the strict-identity-as-strict-title trade-off honestly.

Either way, §4.2.4 needs a rewrite. The current text describes a design that doesn't survive contact with the actual wire format.

### CC-Recon-1B-13 — title/body-opener drift across 5 reviews — **PASS-WITH-DIVERGENCE (D3)** — *drift is far higher than spec assumed*

Same 5-run probe as recon-12. Grouped clusters by `(kind, sorted supported_by)` — the closest analog to "same logical finding across runs":

| Group | Occurrences | Unique `claim_text` | Unique 280B opener | Unique 200B opener | Unique 150B opener |
|---|---|---|---|---|---|
| agreement, [claude-sonnet, gemini-pro] | 4 | **4** | 4 | 4 | 4 |
| agreement, [claude-sonnet, gemini-pro, gpt-4o] | 5 | **5** | 5 | 5 | 5 |
| agreement, [claude-sonnet, gpt-4o] | 2 | **2** | 2 | 2 | 2 |
| assumptions, [] | 5 | **5** | 5 | 5 | 5 |

**Every occurrence had a unique `claim_text`. Drift is 100% at every cutoff (150B, 200B, 280B).** Tightening the body-opener cutoff would not help — the drift is in the title text itself, not past byte 280.

Sample titles for the same logical "looks-fine" agreement cluster across runs:
- "The change is syntactically correct and does not introduce errors."
- "The change introduces no syntax errors or technical risks."
- "The change updates the output to '2' for a potential functional adjustment."
- "The test output changes from 1 to 2 as a direct result of this modification."
- "Understanding the intent behind the change requires additional context or explanation."

Confidence drift was also visible: assumption cluster ranged 0.85 → 0.99 (δ=0.14). v1.0's choice to exclude confidence from the hash was correct, but the drift in the *fields that ARE hashed* is severe.

**D3 — strict-identity hashing on `claim_text` will produce `recurrence_count = 1` for nearly every dismissal on low-signal diffs.** The intended Phase 1C signal (recurrence > N → promote to convention) is effectively unobtainable from this aggregator's output via strict identity.

**Caveat:** the probe used a trivial one-line print diff. Real-world diffs with sharp-edged bugs may produce *less* phrasing variance — multiple models latching onto "null pointer dereference on line 47" is more constrained than five models inventing prose about a trivial change. **Phase 1A's saved real-bug fixture (`auth.py` admin-bypass)** clustered as "Trusting client-controlled 'admin' for authentication creates a severe vulnerability" — sharp content, would likely cluster more stably. We have no live data on that case (the auth.py fixture is from a prior session, not reproducible without 5 fresh runs of the same dangerous diff).

**Recommended D3 response:** acknowledge in §4.2.4 + §3 "Architectural principles" that strict-identity is a **conservative under-counting** approach. Dismissals will work (the user can dismiss the same finding multiple times across sessions and each dismissal is recorded). The *recurrence_count Phase 1C signal* will under-fire on low-signal diffs but should still fire on high-signal diffs where model phrasing converges. The 365-day default expiry (P26) limits the cost of an over-broad dismissal even if drift means it doesn't auto-apply on the next run.

Alternative: explicitly fold a "fuzzy" recurrence layer into Phase 1B (token-overlap similarity over `claim_text`). This is what v0.1 §10 deferred to Phase 1C as "loose-identity." Bringing it forward to 1B would require new design work — not in scope for the current preflight gate.

### CC-Recon-1B-4 — TUI input model — **PASS (Windows-only)**

Built scratch crate with `ratatui = "0.29"` + `crossterm = "0.28"`. Both compile clean on `x86_64-pc-windows-msvc`. TestBackend rendered 3 frames (including multibyte content `café — naïve façade`) without error. Spec key-binding matrix (j/k/g/G/PgDn/PgUp/d/Enter/u/q/Esc/?) implemented in the probe; ctrl+c branch present.

**Interactive Windows Terminal verification deferred to pre-Stage-2** — interactive TUI verification requires a user-facing terminal session, not a tool-harness invocation. The probe binary lives at `%TEMP%\quorum-1b-preflight\recon-4-tui\target\release\tui-probe.exe`; Rolf to run it in a real Windows Terminal pane and confirm:
- j/k navigation moves selection
- d/Enter opens note prompt; multibyte chars accepted (try "café")
- Esc cancels note, Enter submits
- ctrl+c restores cooked-mode (no garbled terminal after exit)
- q exits cleanly

Mac iTerm2 + Linux gnome-terminal coverage **DEFERRED per Rolf's preflight scope decision** (Windows-only verified; cross-platform deferred to pre-Stage-2 or CI runner spike).

### CC-Recon-1B-2 — Git hook activation on Git for Windows — **PASS**

Drove pre-commit + pre-push hooks against Git for Windows 2.53.0.windows.1 with a fake `quorum` shim on PATH. Five scenarios:
1. Plain commit → fake quorum invoked with `review --hook-mode=pre-commit`; exit 0; commit proceeds ✓
2. `QUORUM_SKIP=1` → hook prints skip-stderr, exits 0, fake quorum NOT invoked ✓
3. `quorum` absent from PATH → hook prints "binary not on PATH; skipping" stderr, exits 0 (fail-open) ✓
4. fake exit 1 (high severity) → hook prints "commit blocked", exits 1; commit BLOCKED ✓
5. `QUORUM_HOOK_POLICY=warn` + fake exit 1 → hook exits 0 (warn override); commit proceeds ✓

Marker scan (first-5-line `# quorum-managed-hook` substring per §4.5.4) verified on both managed and non-managed scratch hooks — correctly distinguishes the two.

### CC-Recon-1B-11 — pre-push stdin ref-tuple edge cases (Win) — **PASS-WITH-DIVERGENCE (D4, D5)**

Drove pre-push through six scenarios against Git for Windows 2.53. Captured exact stdin shapes:

| # | Scenario | stdin tuple shape |
|---|---|---|
| P1 | Standard push | `refs/heads/main <new-sha> refs/heads/main <old-sha>` |
| P2 | New-branch push | `refs/heads/feature/x <sha> refs/heads/feature/x 0000000000000000000000000000000000000000` |
| P3 | Force push | `refs/heads/main <new-sha> refs/heads/main <old-sha>` (no special marker) |
| P4 | **Branch deletion** | `(delete) 0000000000000000000000000000000000000000 refs/heads/feature/x <sha>` |
| P5 | Atomic multi-ref | TWO lines, one per ref, in one stdin |
| P6 | **Tag push** | `refs/tags/v0.0-preflight <sha> refs/tags/v0.0-preflight 0000000000000000000000000000000000000000` |

Spec §4.5.5 covers P1, P2, P3, P5 correctly. Two addenda needed:

**D4 — branch-delete tuple's local-ref is literal `(delete)`, not a ref name.** Spec §4.5.5 says "If `local-sha` is all zeros → branch deletion; skip with stderr note." Parser must rely on local-sha=zeros, NOT field 1 being a valid ref. A naive parser that splits on whitespace and uses field 1 as `refs/...` will break. Add to §4.5.5: "Note that on Git for Windows 2.53, the local-ref field for a delete tuple is literally `(delete)` — do not assume field 1 is parseable as a ref when local-sha is all zeros."

**D5 — tag pushes (`refs/tags/*`) come through pre-push with the new-branch-shape tuple (remote-sha = zeros).** Spec §4.5.5 mentions "signed tags ... defer to CC-Recon-1B-11" but doesn't say how to handle them. **Recommend: skip tuples whose `local-ref` starts with `refs/tags/`** with stderr note "quorum: skipping tag push (no commit-range semantics for review)". Tag pushes don't represent a code-change interval that maps to `DiffSource::CommitRange`. Add to §4.5.5.

P3 (force push) looks identical to a standard push from stdin's perspective — there is no special "force" marker in the tuple. The differentiator is the SHA relationship (rewound history). §4.5.5 says "git's range semantics handle force-push correctly" — confirmed at the stdin level; the actual review may surface an empty or rewritten range depending on the rewind, which is fine.

### CC-Recon-1B-5 — crates.io publish dry-run + namespace — **PASS-WITH-DIVERGENCE (D6)**

`cargo search` confirms all three names available:
- `quorum-cli` — no match
- `quorum-core` — only `keyquorum-core` (different package; namespaces don't collide)
- `quorum-lippa-client` — no match

`cargo publish --dry-run --allow-dirty --no-verify`:
- `quorum-core` → packaged 13 files, 75.2KiB; would upload OK ✓
- `quorum-lippa-client` → packaged 11 files, 82.5KiB; would upload OK ✓
- `quorum-cli` → **error: "all dependencies must have a version requirement specified when publishing. dependency `quorum-core` does not specify a version"**

**D6 — `quorum-cli/Cargo.toml` path-deps need `version = "0.2.0"` added before publish.** Current shape `quorum-core = { path = "../quorum-core" }` doesn't satisfy crates.io's publishability requirement. Stage 5 must add `version = "0.2.0"` to both path-deps. ~2 LOC.

Plus Stage 5 metadata gaps (spec §4.8.1 lists, current Cargo.tomls miss):
- `readme = "README.md"` missing in all three crates
- `keywords = [...]` missing in all three
- `categories = [...]` missing in `quorum-cli` (the only one called out by spec)

Plus a license divergence (independent finding): workspace `Cargo.toml` declares `license = "MIT"`, but spec §4.8.1 says `license = "Apache-2.0"`. Phase 1A shipped under MIT (committed). **Rolf to decide:** keep MIT and update §4.8.1, or relicense at Phase 1B publish (irreversible for already-published code).

### CC-Recon-1B-6 — cargo-dist version pin + dry-run — **DEFERRED to pre-Stage-5**

`cargo search cargo-dist` returns `cargo-dist = "0.31.0"` as latest. **Did not run `cargo dist init`** — that command mutates the workspace's `Cargo.toml` and creates `dist-workspace.toml`; running it in-place during preflight would inject Stage-5 artifacts into the tree. Stage 5 should:
1. Pin `cargo-dist = "0.31.x"` (whatever's current at Stage 5 entry).
2. Run `cargo dist init` in a clean worktree.
3. Inspect generated `.github/workflows/release.yml`.
4. Confirm OIDC attestation step present.

**Status: DEFERRED with concrete Stage-5 task list. No spec divergence; just timing.**

### CC-Recon-1B-10 — aarch64 C toolchain in cargo-dist generated workflow — **DEFERRED to pre-Stage-5**

Same deferral basis as -6: depends on `cargo dist init` output. Stage 5 to verify the generated workflow includes an `aarch64-linux-gnu-gcc` (or equivalent cross-toolchain) installation step before the aarch64 `rusqlite bundled` build. If missing, add manually to `release.yml`.

**Status: DEFERRED with concrete Stage-5 task. No spec divergence.**

### CC-Recon-1B-1 — Lippa-side feedback endpoint — **PASS-WITH-FINDING**

Probed eight candidate paths against live Lippa. Notable results:
- `GET /api/v1/feedback` → 404
- `POST /api/v1/feedback` → 404
- `GET /api/v1/memory/propose` → 405 Method Not Allowed (endpoint exists, GET disallowed)
- **`POST /api/v1/memory/propose` → 422 Unprocessable Entity** with validation-error body enumerating required fields

`/api/v1/memory/propose` accepts POST and requires the following fields (from the 422 response):
- `scope` (string)
- `item_type` (string)
- `content` (string)
- `source` (object — per CLAUDE.md hard constraint, `source: "model_proposed"` with `proposed_by_model` and `confidence`)
- `conversation_id` (string)
- `turn_id` (string)
- `excerpt` (string)

**Finding:** the Phase 2 outbox endpoint already exists. Phase 1B remains local-only by design (§2 Non-Goals); this finding is purely forward-compat — Phase 2's outbox can target this endpoint directly without further server-side work. No divergence to v1.0.

`/api/v1/openapi.json` returns HTML (the docs viewer, not the spec). Real OpenAPI spec location not located in this probe; defer to Phase 2 entry.

### CC-Recon-1B-7 — env var naming collision — **PASS**

Confirmed no `LIPPA_*` env vars are consumed anywhere in `crates/`. The Phase 1A binary only reads `APPDATA`, `XDG_CONFIG_HOME`, `HOME`. Phase 1B adds `QUORUM_LIPPA_SESSION`, `QUORUM_LIPPA_EMAIL`, `QUORUM_LIPPA_PASSWORD`, `QUORUM_LIPPA_URL`, `QUORUM_HOOK_POLICY`, `QUORUM_SKIP` — all `QUORUM_`-prefixed; no collision with Lippa's own server-side env vars on the same host. Spec §7 row -7 wording stands as written.

**Documentation nit (no divergence):** the repo's `.env` file uses unprefixed `LIPPA_URL`, `LIPPA_EMAIL`, etc. for preflight scripts. README should clarify that Quorum reads `QUORUM_LIPPA_*` env vars, not `LIPPA_*`, so a future maintainer reading `.env` doesn't conflate them.

### CC-Recon-1B-9 — SQLite WAL on NFS/SMB — **PASS (on spec rationale)**

Per Rolf's preflight scope decision: no NFS/SMB mount available for live spike. Spec §4.2.2 P37 already specifies the fallback (try WAL; if `journal_mode != 'wal'` after PRAGMA, fall back to `journal_mode = DELETE` with stderr warning). This is the design; the supportable-filesystem matrix is to be validated post-ship if a user reports a problem.

**No spec change.** Stage 1 implementation must include the fallback path per P37; the integration test should mock a non-WAL-supporting `Connection::open` by other means.

---

## Side-findings discovered en route (not in v1.0 §7 recon list)

### D7 — `quorum-lippa-client::auth_apply` manual-Cookie-header path returns 403 from Lippa edge

Discovered while building the recon-12/-13 live probe. Two parallel reqwest clients tested with the same valid session cookie:
- `reqwest::Client::builder().cookie_store(true)` + cookie jar auto-replay → `POST /api/v1/consensus/sessions` returns 201 ✓
- Manual header `.header(COOKIE, format!("session={}", value))` (what Phase 1A `crates/quorum-lippa-client/src/client.rs:91-95` does) → `POST /api/v1/consensus/sessions` returns **403 Forbidden** with HTML body

Same session, same prompt, same time of day. The only difference is how the Cookie header is set. Most likely an edge-tier change at Lippa (Cloud Armor / CSP / cookie attribute matching) tightened since Phase 1A close on 2026-05-11 morning. The Phase 1A release binary at `target/release/quorum.exe` is built against this path and **will fail end-to-end against current Lippa.** All ACs that need a live Lippa interaction (smoke-test ACs from Phase 1A close, plus all 1B happy-path ACs) are blocked until this is resolved.

**D7 — CRITICAL Stage-4 blocker (not in v1.0 §7).** Three options:
- **D7-A (recommended):** Switch `LippaClient::new` to `reqwest::Client::builder().cookie_store(true)` and use a `cookie::Jar` populated from the secret at construction. ~30 LOC; the `Secret` newtype contract stays; the manual `auth_apply` Cookie-header injection goes away. Stage 4 work; should land before Stage 3 hooks since pre-commit / pre-push depend on `quorum review` actually reaching Lippa.
- **D7-B:** Reverse-engineer the exact header(s) Lippa now requires (Origin, Referer, secondary cookie, CSRF token, sec-fetch-* family) and add them to `auth_apply`. Brittle; Lippa edge config is opaque to us.
- **D7-C:** Reach out to Lippa team to confirm intended client-auth pattern (especially if they support a Bearer or API-key path now, in which case M-ExternalAuth landed early).

Recommend **D7-A** — it's narrow, reuses reqwest infrastructure, doesn't expose internal cookie details, and matches what their own SDK examples likely do.

### D8 — `project_id` in session-create payload triggers 403 at Lippa edge

Discovered as the failure mode of the recon-12/-13 probe initial run. Same login session, same prompt, same model_roles → `project_id=<UUID>` field present in multipart returns 403; field absent returns 201 and the session converges.

This is **different from Phase 1A's reported failure** (CLAUDE.md says "sessions submitted with project_id consistently fail in ~5s due to a server-side seeding-context exception"). Phase 1A saw a delayed application-layer error; we're now seeing an immediate edge 403. The trigger (project_id presence) matches; the response shape changed.

**D8 — blocks any Phase 1B AC that requires live verification with a project context.** Specifically: ACs 52, 53, 67, 79, 96 (Phase 1B numbering) cannot be live-verified until this is resolved Lippa-side. **Same status as Phase 1A's ACs 17/22/25/30 deferral** — they're MOCK-covered comprehensively in the integration test suites; they're not blocked at the CC level; live verification is deferred and tracked.

**This contradicts Rolf's pre-preflight answer "Fixed — sessions converge."** The fix may have been a frontend/account thing; the WAF / app-layer 403 on project_id submission persists from API-direct clients. Worth a separate Lippa-side issue if not already filed.

Drift probe ran without `project_id` and converged 5/5; the recon-12 and recon-13 verdicts above hold (cluster aggregation does not depend on project context). But Stage-1's live happy-path AC verification will fail the same way unless D8 is resolved.

---

## Consolidated divergences

| ID | Severity | Item | Spec section to amend | Stage affected |
|---|---|---|---|---|
| D1 | low | rusqlite bundled size ~1.6MB on Windows MSVC (not ~700KB) | §4.0 footnote | Stage 1 |
| D2 | **high** | P39 conditional doesn't fire; spec §4.2.4 inaccurately describes `Finding.body` as rich prose; body_opener needs retarget or removal | §4.2.4 rewrite | Stage 1 |
| D3 | **high** | claim_text drift is ~100% on low-signal diffs; cutoff doesn't help; strict-identity under-counts recurrence | §3 principle + §4.2.4 note + §10 open question | Stage 1 + future |
| D4 | low | pre-push branch-delete tuple has local-ref = `(delete)` not a ref name | §4.5.5 addendum | Stage 3 |
| D5 | low | pre-push tag pushes need explicit skip rule | §4.5.5 addendum | Stage 3 |
| D6 | low | quorum-cli path-deps need `version = "0.2.0"` for publish; per-crate metadata (readme/keywords/categories) gaps; license MIT-vs-Apache-2.0 mismatch | §4.8.1 + path-dep declarations | Stage 5 |
| D7 | **critical** | `auth_apply` manual-Cookie-header path returns 403 from current Lippa edge; only `cookie_store(true)` works | new §4.6.x or §4.4 amendment | Stage 4 — but blocks ALL live verification |
| D8 | **high** | `project_id` in session-create triggers edge 403 (Lippa-side; different shape than Phase 1A's seeding-context error) | external Lippa issue | live verification of any AC needing project context |

D7 and D8 are **Lippa-side** problems we discovered as side-effects; the others are spec/code adjustments inside Quorum's surface.

The Stage-1 code freeze should not start until D2 and D3 are settled. D7 should not block Stage 1 starting (the bug is Stage-4 surface), but **D7 blocks every live verification step**, so mock coverage will be the only QA path until it's fixed.

---

## Adjudication

Divergence gate cleared by Rolf 2026-05-11. Directives below are verbatim; CC proceeds in the stated commit order.

**D1 (binary size):** Noted; cosmetic. Update preflight figure to ~1.6MB Windows MSVC (smaller after Linux strip).

**D2/D3 (identity hash):** Revert to 3-input hash: `(title, source.type, sorted source.models)`. Drop `body_opener`. Drop the P39 conditional from §4.2.4 entirely — no `file` to swap in. Keep the `body_snapshot` schema column (harmless; Phase 1C may use it). Length-prefix concatenation no longer needed since the model set is the only multi-value field; join models with `0x1F` between elements and use `0x1F` as field separator. Document the residual cross-finding collision risk in the preflight notes + at Phase 1B close in `SERVICES.md` §6.

**D4 (pre-push branch deletion):** Detect via `local_ref == "(delete)"` literal, not zero-sha. Update §4.5.5 mental model accordingly; reflect in `SERVICES.md` §8 at Phase 1B close.

**D5 (pre-push tag push):** Tag pushes (`local_ref` prefix `refs/tags/`) skip with stderr note `quorum: tag push detected; skipping review (code review not applicable to refs/tags/*)`. Same `SERVICES.md` §8 reflection.

**D6 (metadata):** Workspace license is Apache-2.0 across the board — matches v1.0 spec, matches Rust-tools-ecosystem norm. Update workspace `LICENSE` file, every crate's `Cargo.toml`, and any source-file SPDX headers. Add `version = "0.2.0"` to path-deps. Fill `readme`, `keywords`, `categories` per the gaps identified above.

**D7 (cookie auth):** Approved as a standalone Phase 1A fix; see commit 2.

**D8 (project_id 403):** Noted; no spec change. Same workaround as Phase 1A close — mock-heavy live verification; project-bound ACs deferred. Continue.

**Recon-1 forward-compat:** `/api/v1/memory/propose` schema documented; append the schema to preflight notes (below) for Phase 2 reference. At Phase 1B close, update v1.0 §11 Phase 2 plan with the concrete contract.

**Recon-6/-10 (cargo-dist):** Deferred to Stage 5 entry per the existing task list.

### Commit order

1. **`recon: preflight notes + divergence-gate adjudication`** — this file.
2. **`fix(client): use reqwest cookie_store for session lifecycle`** — Phase 1A regression caused by upstream Lippa edge changes. Switch `LippaClient::new` to `reqwest::Client::builder().cookie_store(true)`. Drop the manual `Cookie: session=...` header injection in `auth_apply`. Bump `quorum-lippa-client` version to 0.1.1 (patch — Phase 1A bug fix, not Phase 1B feature). Update `SERVICES.md` §3 (Cookie Lifecycle) to reflect cookie-jar mechanics — capture, use, renewal, persistence still apply but now flow through the jar. Verify against live Lippa that `/sessions` returns the expected create response, not 403.
3. **`chore: license to Apache-2.0; publish metadata gaps`** — D6 changes only. Workspace + per-crate.

Then begin Stage 1 per v1.0 spec §8 with these deltas:
- Identity hash is 3-input per D2/D3 adjudication. `finding_identity_hash` doc comment in `crates/quorum-core/src/memory/identity.rs` reflects actual inputs (no `body_opener`, no P39 conditional).
- Add `dismissals_filter.rs::cross_finding_collision_rate` test: run ~20 varied diffs against the mock fixture, count distinct `(title, source.type, sorted_models)` triples per logical finding. Assert collision rate ≤2%. If higher, file `BACKLOG.md` ticket for loose-identity exploration and proceed — do not block Stage 1 on it.
- Other Stage 1 deliverables unchanged.

Halt at end of Stage 1 per session-driver standing instruction.

### Recon-1 contract: `POST /api/v1/memory/propose`

Captured from a 422 validation response (empty-body POST after authenticated login). Required fields:

| Field | Type (inferred) | Notes |
|---|---|---|
| `scope` | string | Likely enum (workspace / project / global) — exact set not enumerable from a single 422 |
| `item_type` | string | Memory category (likely convention / pattern / preference) |
| `content` | string | The memory body (proposed convention text) |
| `source` | object | Per CLAUDE.md hard constraint, must be `{ "type": "model_proposed", "proposed_by_model": "<id>", "confidence": 0.0..1.0 }` |
| `conversation_id` | string | Lippa session id that produced the proposal |
| `turn_id` | string | Round/turn id within the session |
| `excerpt` | string | Snippet from the originating cluster — likely what Phase 1B's `body_snapshot` will feed |

Phase 2 outbox can target this endpoint directly. Phase 1B writes nothing here.

---

## Status

**Adjudication received; CC proceeding per the directive list above.** Phase 1B is no longer at the HALT gate.

`../lippa` verified untouched at session end:
```
$ git -C ../lippa status --short
(empty)
```

Phase 1B is otherwise mock-test-ready: all 13 recon items have outcomes; nothing is open-ended. The biggest unknowns are not technical — they're product decisions on the identity-hash design (D2/D3) and the Lippa-side regressions (D7/D8).
