# Quorum Phase 1A — Preflight notes

**Date:** 2026-05-10
**Verifier:** Claude Code session-driver run.
**Account:** rolf@lippa.com on https://app.lippa.ai (workspace `2438e10f-a558-427b-a8bd-645304049456`, project `a52a0e9f-938c-4f85-a2a1-b24d8646e2ce`).
**Outcome:** 4/4 blockers pass; Blocker 4 is reduced-scope. Proceed to implementation.

---

## Blocker matrix

| Blocker | Result | Evidence | Notes |
|---|---|---|---|
| 1 — Login endpoint | **PASS** | `apps/api/app/routes/api_v1/auth.py:104-192`; `apps/api/app/main.py:419-424` (SessionMiddleware config); live `POST /api/v1/auth/login` returned 200 with `Set-Cookie: session=...; path=/; Max-Age=1209600; httponly; samesite=lax; secure`. | Body: **JSON** (Pydantic `LoginRequest`). CSRF: **none** (no middleware, no token check). 2FA: **not enforced** (no TOTP field or gate). Failure modes: 401 generic, 403 `email_not_verified`, 403 `suspended`. Account lockout: 5 failed attempts → 15-min `locked_until`. |
| 2 — Single-model Consensus | **PASS w/ divergence** | `apps/api/app/routes/api_v1/consensus.py:327-475`; live create-session with `model_roles=["reviewer"]` accepted, returned 201 with `models_active: 3`. | **`model_roles` is metadata, not selection.** Server enforces `len(profiles) >= 2` against workspace's `is_default=True, health_state="active"` profiles (line 391-401). `model_roles` is `Optional[dict[model_uuid → role_label]]` consumed only at the GET-detail handler (line 253) for display. **Single-model is impossible on this endpoint.** Phase 1A submits `model_roles=null`, gets N-model behind the scenes, aggregates findings — see §"Spec divergences" below. |
| 3 — Diff size / token budget cap | **PASS** | `apps/api/app/routes/api_v1/consensus.py:351-353` (min check only); `apps/api/app/services/consensus_service.py:33` (`CONSENSUS_CONTEXT_PACK_MAX_CHARS = 8000` is server-side seeding context, not user prompt). | **Min prompt: 20 chars** (400 reject). **No max prompt cap at the API level.** Oversized prompts pass through to model providers; failure surfaces as session status `failed` (Phase 1A's existing exit-2 path). Quorum's 200KB local default stands. |
| 4 — Detail response schema | **REDUCED-SCOPE PASS** | `apps/api/app/routes/api_v1/consensus.py:200-320`; live converged detail at `/tmp/quorum-preflight/blocker4-detail-converged.json` (28,877 bytes, session `cc36bbe2-41cb-4ae1-8a21-456307969008`). | **No typed findings.** Response is debate-shaped: `summary_text` (prose), `agreement[]`, `divergence[]`, `assumptions[]` (claim clusters), `rounds[]` (per-(round × model) raw text). Cluster shape: `{cluster_id, claim_text, confidence, supported_by[vendor], has_memory_match}`. **No severity, file, line_range, or suggestion fields.** Phase 1A `Finding` collapses to `{title=claim_text, body, severity=synthesized}` with `severity` mapped from cluster type (`divergence→High`, `agreement(conf≥0.85)→Medium`, `agreement→Low`, `assumptions→Info`). |

---

## Spec divergences applied during implementation

These are inline patches, captured here for v1.1 spec revision after the session.

### D1 — `model_roles` is not a model selector

**Spec assumption (§4.4, non-goal §2):** Phase 1A is "single-model Consensus, `model_roles` carries exactly one entry."

**Reality:** `model_roles` is a `dict[model_uuid → role_label]` for display purposes only. Models are selected server-side from `ConsensusModelProfile WHERE is_default=True AND health_state='active'`. There is no API to constrain to one model.

**Phase 1A implementation:**
- `SessionCreateRequest.model_roles: Option<HashMap<String, String>>` (Rust). Phase 1A sends `None` (omits field) since we have no UUIDs to label.
- `wire::SessionDetail.models: Vec<Model>` carries the actual model list — Phase 1A renders the model count and vendor names rather than a single "model_name."
- `Review.model_name` becomes `Review.model_names: Vec<String>` (e.g., `["claude-sonnet", "gpt-4o", "gemini-pro"]`). The constant in `quorum-cli/src/commands/review.rs` for "the chosen role" is dropped — there's nothing to choose.
- Acceptance criterion 17's stdout rendering shows `Reviewed by claude-sonnet, gpt-4o, gemini-pro` instead of `Reviewed by <single-model>`.

### D2 — Detail response is debate-shaped, not findings-shaped

**Spec assumption (§4.3.1):** typed `Finding { severity, file, line_range, title, body, suggestion }`.

**Reality:** see Blocker 4 row. No severity/file/line. Structure is claim clusters by agreement/divergence/assumptions plus per-round raw model text.

**Phase 1A `Finding` shape:**
```rust
pub struct Finding {
    pub severity: Severity,
    pub title: String,           // = claim_text
    pub body: String,            // = supporting context (models, confidence, summary excerpt)
    pub source: FindingSource,   // Agreement | Divergence | Assumption | Summary
    // file / line_range / suggestion REMOVED — Lippa does not return them.
}

pub enum FindingSource {
    Summary,      // synthesized from summary_text
    Agreement,    // from agreement[]
    Divergence,   // from divergence[]
    Assumption,   // from assumptions[]
}
```

**Severity mapping in `review_from_json`:**
- `divergence[]` clusters → `Severity::High` (areas of model disagreement = areas of code uncertainty)
- `agreement[]` clusters with `confidence >= 0.85` → `Severity::Medium`
- `agreement[]` clusters with `confidence < 0.85` → `Severity::Low`
- `assumptions[]` clusters → `Severity::Info`
- Synthesized summary entry from `summary_text` → `Severity::Info`, `FindingSource::Summary` (single entry)

**Markdown renderer changes (§4.5):**
- Drop "src/lib.rs:42-58 — Title" pattern; render `## Severity — title` without file refs.
- Add a top-level `## Summary` block from `summary_text` before the per-severity sections.
- Add "Supported by: gemini-pro, claude-sonnet, gpt-4o" after each finding body if `supported_by` is non-empty.

### D3 — `create_session` is multipart, not JSON

Already documented in `Quorum-Recon-v0_findings.md` §1.1 note. Implementation uses `reqwest::multipart::Form` rather than `.json(...)`.

### D4 — `models_active` may be 0 in a `pending` session, ramping up

Live observation: a fresh session begins `models_active: 3, rounds=0/3`, transitions through `running` state for ~80s before reaching `converged`. Polling cadence (1s → 8s with jitter, §4.2.4) is appropriate for this convergence time.

---

## Operational observation (not a blocker, but flag for live verification)

**Sessions submitted with `project_id` consistently failed within ~5s on this account/project.** Two preflight attempts (initial trivial-diff prompt, then substantive-prompt with `project_id`) both terminated with `status: failed` in poll #2. The session submitted **without** `project_id` converged normally in 80s. Failure cause is almost certainly an exception in `_generate_seeding_context(project_id, db)` (`consensus_service.py:143-144`).

**Implications for Phase 1A:**
- The `failed`/`exit 2` code path is correct and exercised. No code change needed.
- **Live verification (acceptance criteria 17, 22, 25, 30) requires care:** running `quorum review` after `quorum link --project <id>` on this account will likely return `Consensus session failed: <reason>` and exit 2. Workaround for live verification: submit without project_id (i.e., test the unlinked path), or have Rolf identify a healthy project_id, or temporarily comment out project_id submission in `quorum review` for verification only.
- **Not a Phase 1A blocker** — it's a Lippa-side data/setup issue specific to this project, not a contract issue.

---

## Concept-spec re-validation

Lippa's always-multi-model architecture with agreement-cluster output matches the **original Quorum concept spec** ("Consensus across frontier models") more directly than the Phase 1A spec did. Phase 1A's "single-model" framing was a self-imposed scope reduction — an artifact of pre-recon assumption, not a Lippa constraint. Now that we know the API always returns multi-model output, the v1.1 roadmap should drop **Phase 1C (multi-model Consensus)** as a separate milestone: Phase 1A already delivers it. Phase 1B-and-after can focus on interactive dismissal, hook installation, CI auth, and the dismissals store without the "still single-model" caveat.

## Determinism for stable rendering

`Review.model_names: Vec<String>` MUST be **alphabetically sorted** (case-insensitive). Lippa's `models[]` array ordering is not guaranteed stable across sessions, so without sorting AC 17's stdout and `tests/render.rs` snapshot tests would flake on order shuffles. The sort happens in `quorum-core::review_from_json` at the mapping boundary, not at render time. This is the same pattern applied to JSON archive serialization (deterministic key ordering, §4.3.3) — single fixpoint per pipeline.

---

## Recommended v1.1 patches (post-session, for Rolf)

| ID | Section | Patch |
|---|---|---|
| V1.1-1 | §2 non-goals, §4.4 | Remove "No multi-model Consensus / `model_roles` carries exactly one entry." Replace with "Quorum submits no `model_roles` and aggregates whatever models Lippa runs; Phase 1A's UX is single-session, model count is server-controlled." |
| V1.1-2 | §4.3.1, §4.5 | Update `Finding` struct to drop `file`/`line_range`/`suggestion`; add `FindingSource` enum; document severity-synthesis rules. Update markdown renderer to drop file refs and add `## Summary` + supported_by lines. |
| V1.1-3 | §4.2.4 | Add note: `model_roles` field is `Option<HashMap<String, String>>` (display labels keyed by model UUID), not a selection mechanism. Phase 1A sends `None`. |
| V1.1-4 | §4.6 tests | Update `bundle_assembly.rs` fixtures and `render.rs` snapshots to reflect new `Finding` shape. Drop file/line assertions; add `FindingSource` assertions. |
| V1.1-5 | §6.1 ACs | Acceptance criterion 17: rendered output mentions vendor list rather than single `<model_name>`; vendors alphabetized for stability. AC 20 message remains correct. AC 22 truncation markers remain correct. |
| V1.1-6 | §4.5 | If `assumptions[]` is empty and `divergence[]` is empty (high agreement session), renderer emits only `## Summary` + `## Agreement (N)` sections; no zero-count headers. |
| V1.1-7 | §11 roadmap | Drop Phase 1C as a separate milestone. Multi-model is Phase 1A. |
| V1.1-8 | §4.3.1 | `Review.model_names: Vec<String>` is alphabetically sorted (case-insensitive) at the `review_from_json` mapping boundary. |

---

## Cookie semantics (additional confirmation for §4.7 and CC-Recon-6)

Live `Set-Cookie` attributes from `POST /api/v1/auth/login`:
- `path=/`
- `Max-Age=1209600` (= 14 days, fixed-window from login)
- `httponly`
- `samesite=lax`
- `secure`

`Max-Age` is fixed; no rolling renewal was observed across the 16 polls of the converged session (no `Set-Cookie` re-issuance on `/status` or `/sessions` GETs). Phase 1A's auto-renewal capture code in §4.2.4 remains correct under this regime — it activates if/when Lippa flips to rolling sessions; today the capture is a no-op in practice.

---

## Artifacts

All live-probe artifacts under `%TEMP%\quorum-preflight\` (Windows temp, never committed):

- `blocker1-login-headers.txt` — login HTTP, cookie attrs (cookie value redacted), body.
- `blocker2-create-headers.txt` — create-session HTTP, response body, session_id.
- `blocker4-status-trace2.json` — poll trace of the converged session.
- `blocker4-detail-converged.json` — **the canonical seed for `tests/fixtures/wire_format/lippa_v1_session_detail.json`** (criterion 36). 28,877 bytes, 23 top-level keys.

The detail JSON is referenced by `quorum-core` tests; the wire-format fixture under `tests/fixtures/` is a copy of this file, cookie/secret-free by construction (Lippa's detail response carries no auth material).
