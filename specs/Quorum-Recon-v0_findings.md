# Quorum-Recon-v0 — Findings

**Session:** 2026-05-10
**Mode:** read-only static inspection of Quorum + Lippa repos
**Scope:** API-availability questions that gate Quorum Phase 1A architecture

---

## Branch verdict

**GO-PAT-PARTIAL** — Quorum drives a small Lippa-side spec
(working name `M-MCPLaunch-MinimalScope` or `M-ExternalAuth-MinimalScope`)
to extend Lippa's already-existing `extension_token` Bearer-auth primitive
to the `/api/v1/*` routes Quorum needs. Phase 1A code can ship in parallel
against the cookie-session path as a transitional fallback, then swap to
Bearer once the Lippa-side spec lands.

**Why not GO-JWT.** There is no JWT in Lippa today. `/api/v1/*` is gated
by Starlette `SessionMiddleware` cookie-session auth (file evidence
[apps/api/app/dependencies.py:31-71](../../lippa/apps/api/app/dependencies.py#L31-L71))
and there is no OAuth/OIDC surface. A "GO-JWT" branch is really
"GO-COOKIE": the CLI would email/password POST to `/api/v1/auth/login`
and persist the `Set-Cookie: session=...` header. Workable for an
early-internal user, but brittle (cookie has no advertised max-age,
forces 2FA / email-verified handling into the CLI, ties survival to
SessionMiddleware secret rotation).

**Why not WAIT or DEGRADED.** All five capabilities Quorum Phase 1A
needs are reachable today via cookie auth — the only blocker is auth
shape, not API coverage. WAIT is overkill; DEGRADED would be
self-imposed.

---

## Phase 0 — Desk research confirmation

**PASS.** All three desk-research claims confirmed against current code.

- ✅ Consensus endpoints live at `/api/v1/consensus/*` —
  [apps/api/app/routes/api_v1/consensus.py:117-621](../../lippa/apps/api/app/routes/api_v1/consensus.py).
- ✅ All 8 endpoints present at exact paths claimed in
  [SERVICES.md:1614](../../lippa/SERVICES.md#L1614). See Phase 1.1 matrix below.
- ✅ `model_proposed` is in the source allowlist
  `ck_memory_items_source_v2`. [SERVICES.md:2387](../../lippa/SERVICES.md#L2387)
  enumerates the 8-value set and notes it is "fully validated post-M-AccountMemoryRetrieval Stage 3 (migration 113)".

---

## Phase 1 — Auth findings

### 1.1 Consensus endpoints

**Auth uniformity.** All 7 authenticated endpoints share `Depends(require_auth)`
imported from `app.dependencies`. The 8th (`GET /consensus/shared/{token}`)
is intentionally unauthenticated. Critical finding: `require_auth`
(see [apps/api/app/dependencies.py:31-71](../../lippa/apps/api/app/dependencies.py#L31-L71))
**only reads `request.session.get("user_id")`** — a Starlette
SessionMiddleware cookie. There is **no** `Authorization: Bearer`,
JWT, or PAT acceptance path on `/api/v1/*`.

| # | Method | Path | Auth | Request shape | Response shape (key fields) |
|---|--------|------|------|---------------|-----------------------------|
| 1 | GET    | `/consensus/models`                       | session cookie | none | `[{id, vendor, display_name, health_state, is_default}]` |
| 2 | GET    | `/consensus/sessions`                     | session cookie | query: `project_id?`, `limit≤50`, `offset≥0` | `{sessions[], total, credits_remaining}` |
| 3 | GET    | `/consensus/sessions/{id}`                | session cookie | none | full detail incl. `rounds[]`, `models[]`, `agreement[]`, `divergence[]`, `assumptions[]`, `document` |
| 4 | POST   | `/consensus/sessions`                     | session cookie | **multipart/form-data**: `prompt` (≥20 chars), `project_id?`, `debate_mode∈{standard,document_critique,red_blue_team}`, `model_roles?` (JSON string), `context_text?`, `idempotency_key?`, `document?` (UploadFile) | 201 `{id, status:"pending", credits_reserved, credits_remaining}`. 409 `{error:"duplicate", existing_session_id, status}`. 402 `{error:"insufficient_credits", credits_remaining}`. 403 `Consensus disabled`. 503 `Not enough active models` |
| 5 | GET    | `/consensus/sessions/{id}/status`         | session cookie | none | `{status, rounds_completed, rounds_total, models_active, models_dropped, elapsed_seconds, follow_up_questions}` |
| 6 | POST   | `/consensus/sessions/{id}/link`           | session cookie | JSON `{project_id}` (Pydantic `LinkRequest`) | `{feedback_state, items_proposed}`. 400 if `status != "converged"` |
| 7 | POST   | `/consensus/sessions/{id}/share`          | session cookie | none | `{share_token, share_url}` |
| 8 | GET    | `/consensus/shared/{share_token}`         | **none**       | none | subset detail (no `rounds`, no `follow_up_questions`) |

Notes on shape details that matter for a CLI client:

- **Multipart create (row 4)** uses `await request.form()` not a Pydantic
  body. Quorum's `reqwest` will need `multipart::Form` even when no
  document is attached. ([consensus.py:327-471](../../lippa/apps/api/app/routes/api_v1/consensus.py#L327-L471))
- **Per-model raw findings ARE in the detail response.** `rounds_data`
  loop at [consensus.py:275-287](../../lippa/apps/api/app/routes/api_v1/consensus.py#L275-L287)
  emits `{round_index, model_profile_id, vendor, raw_response,
  agreement_score_vs_others, latency_ms, error_text,
  dropped_after_this_round}` for every (round × model). This means
  Quorum can render its "model votes" UX directly — no Lippa-side spec
  gap on this front.
- **Idempotency key** is per-user. Re-sending the same key returns 409
  with the existing session id, which is exactly what a hook-driven CLI
  needs for retry-safety.
- **Credits are atomic** via `SELECT ... FOR UPDATE` on `users` row
  ([consensus.py:376-389](../../lippa/apps/api/app/routes/api_v1/consensus.py#L376-L389)).

### 1.2 Memory propose endpoint

**File:** [apps/api/app/routes/api_v1/memory.py:71-363](../../lippa/apps/api/app/routes/api_v1/memory.py#L71-L363).
The route is `POST /api/v1/memory/propose` and re-exports `require_auth`
from `app.routes.api_v1.deps` (which is itself a re-export of
`app.dependencies.require_auth`, file evidence
[apps/api/app/routes/api_v1/deps.py:6-9](../../lippa/apps/api/app/routes/api_v1/deps.py#L6-L9)).
So the auth shape is identical to consensus: **session cookie only**.

| Question | Answer | Evidence |
|---|---|---|
| Auth dependency chain | `Depends(require_auth)` (cookie session) | [memory.py:84](../../lippa/apps/api/app/routes/api_v1/memory.py#L84) |
| `conversation_id` required? | **Required.** `_validate_conversation_ownership` raises if missing/cross-tenant. | [memory.py:97-99](../../lippa/apps/api/app/routes/api_v1/memory.py#L97-L99) |
| `turn_id` required? | **Required.** `_validate_turn_belongs_to_conversation` enforces FK relationship. | [memory.py:100-102](../../lippa/apps/api/app/routes/api_v1/memory.py#L100-L102) |
| Rate limit shape | `MODEL_PROPOSED_RATE_LIMIT` per `(conversation_id, proposed_by_model)` per `MODEL_PROPOSED_RATE_WINDOW_HOURS`. Counter source is the `count_recent_model_proposals(db, conversation_id, model)` function in `direct_memory_service`. Spec value documented as 3 per conversation per model per 24 h. | [memory.py:147-166](../../lippa/apps/api/app/routes/api_v1/memory.py#L147-L166) |
| Source allowlist enforcement | **DB-level CHECK** (`ck_memory_items_source_v2`, migration 113). 8 allowed values: `extraction, document_extraction, mcp, consolidation, consensus, user_directed, model_proposed, workspace_memory_migration`. | [SERVICES.md:2387](../../lippa/SERVICES.md#L2387) |

**Critical for Quorum:** `conversation_id` and `turn_id` are NOT NULL.
Quorum has no native conversation — its review session needs to mint a
synthetic Lippa conversation_id + turn_id (or pull from a "Quorum review
session" Lippa conversation) before each memory write. That is a
non-trivial coupling and needs to be designed into Phase 1A's memory
write path. Filed as an item below.

### 1.3 PAT infrastructure state — **C: Spec only**

Three queries, one consistent answer:

1. `personal_access_tokens` — **zero matches** in `apps/api/app/models/`,
   **zero matches** in `apps/api/alembic/versions/` (128 migrations
   total, none mention PAT / personal_access).
2. `lpa_` / `lpk_` token prefix — **zero matches** anywhere in
   `apps/api/`.
3. `M-MCPLaunch` / `M-DeveloperPlatform` — **zero matches** in `apps/api/`
   source. Referenced only in spec docs.

**Important nuance** that reshapes the branch decision: Lippa already
ships a per-user Bearer-token primitive. `User.extension_token` is a
random UUID (regenerable via existing routes at
[apps/api/app/routes/extension.py:123-211](../../lippa/apps/api/app/routes/extension.py#L123-L211))
that is accepted by:

- The Chrome extension surface (`/api/extension/*`) via the
  `x-cf-token` header
  ([extension.py:92](../../lippa/apps/api/app/routes/extension.py#L92)).
- The MCP SSE surface (`/mcp/*`) via `Authorization: Bearer <token>`
  ([apps/api/app/routes/mcp.py:27-53](../../lippa/apps/api/app/routes/mcp.py#L27-L53)).

It is NOT accepted by `/api/v1/*`. So PATs as a brand-new system are
not built (state C), but a Bearer-acceptable per-user-token mechanism
already exists at the model layer. Extending its acceptance to
`/api/v1/*` is the cheapest path to Quorum's external-client story.

---

## Phase 2 — Capability-by-capability

### Capability A — Read project memory

**Surface:** `GET /api/v1/memory-explorer?scope=project&project_id={id}`
([apps/api/app/routes/api_v1/memory_explorer.py:48-114](../../lippa/apps/api/app/routes/api_v1/memory_explorer.py#L48-L114)).
Returns `MemoryExplorerResponse` (scope-narrowed payload, all items in
the project). Auth: `require_auth` (session cookie).

**Gap.** There is **no "top-N by similarity to a code diff"
endpoint** on `/api/v1/*`. The `pack_builder` does similarity ranking
server-side but is not exposed as a JSON API surface (only consumed by
HTMX pack pages and the live-pack worker). Two viable paths for Quorum:

- **Client-side scoring (default).** Quorum pulls all items via
  memory-explorer, embeds the diff via OpenAI text-embedding-3-small
  (Lippa's pinned model — match it for compatibility),
  cosine-ranks locally. Cheap and avoids a Lippa-side spec.
- **Server-side similarity (optional).** Future Lippa-side spec adds
  `POST /api/v1/projects/{id}/memory/similar` that takes raw text or an
  embedding and returns top-N items. Defer until v2.

**Verdict: viable today** with client-side scoring.

### Capability B — Write `convention` memory item

**Surface:** `POST /api/v1/memory/propose` covered in §1.2 above.

For Quorum's use case: `source="model_proposed"`, `proposed_by_model="quorum-<model>"`,
`item_type="convention"`, `scope="project"`, `scope_id=<project_id>`,
`confidence=<float>`, `content=<rule text>`, `excerpt=<diff snippet>`.

**Constraints to design around:**
- Required `conversation_id` + `turn_id` — Quorum must own a Lippa
  conversation per review session (or one persistent "quorum review
  log" conversation per project) and pass real IDs.
- `model_proposed` writes are subject to user-presence check
  ([memory.py:127-142](../../lippa/apps/api/app/routes/api_v1/memory.py#L127-L142)) —
  user message in the conversation within last 15 min. The CLI will
  need to seed the conversation appropriately or this is a Lippa-side
  spec gap (relax presence check for `proposed_by_model` matching a
  registered external client). Filed below.
- Rate limit 3/conv/model/24h means Quorum must batch convention
  candidates per review or rotate `proposed_by_model` IDs.

**Verdict: viable today** with two design constraints to handle.

### Capability C — Submit Consensus session

**Surface:** `POST /api/v1/consensus/sessions` covered in §1.1 row 4.

**Multipart vs JSON.** Confirmed multipart/form-data (uses
`await request.form()` and `UploadFile`). Rust `reqwest::multipart::Form`
handles it cleanly. The CLI does not have to upload a document — that
field is optional — but the request envelope must still be multipart.

**Cost:** 3 credits per session (default `CREDITS_PER_SESSION`,
overridable via workspace setting `CONSENSUS_CREDIT_COST`).

**Verdict: viable today.**

### Capability D — Get Consensus result (with raw per-model findings?)

**Surface:** `GET /api/v1/consensus/sessions/{id}/status` (poll until
`status="converged"`) then `GET /api/v1/consensus/sessions/{id}` for
detail.

**Per-model raw findings: YES.** The detail handler emits a `rounds[]`
array with one entry per `(round × model)` containing `raw_response`
(full LLM output text), `vendor`, `agreement_score_vs_others`,
`latency_ms`, `error_text`, `dropped_after_this_round`
([consensus.py:275-287](../../lippa/apps/api/app/routes/api_v1/consensus.py#L275-L287)).

Quorum can render its "model votes" UX directly. **No Lippa-side spec
gap on result shape.**

**Verdict: viable today.**

### Capability E — Cost attribution

**Surface:** `User.consensus_credits` decrement, atomic, at session
create ([consensus.py:419](../../lippa/apps/api/app/routes/api_v1/consensus.py#L419)),
guarded by `SELECT ... FOR UPDATE` at
[consensus.py:376-378](../../lippa/apps/api/app/routes/api_v1/consensus.py#L376-L378).
Refunded on session expiry / failure (3 sites in
`consensus_service.py` — lines 133, 163, 259).

**Important contrast:** Consensus does **not** write `usage_events`
rows. `record_usage` lives in
[apps/api/app/services/usage_service.py:16](../../lippa/apps/api/app/services/usage_service.py#L16)
and is called from chat / streaming / quota / attachment / account-
memory paths — but `consensus_service.py` has zero references to it
(verified by grep). Consensus billing is the per-user `consensus_credits`
counter, not the per-token `usage_events` ledger.

**For Quorum:** an external CLI authenticated as user U on workspace W
will decrement `U.consensus_credits` correctly. Workspace-level cost
attribution flows through the workspace the user is currently signed
into (`current_user.workspace_id`). No additional plumbing needed.

**Verdict: viable today.**

---

## Phase 3 — Branch decision + reasoning

**Selected: GO-PAT-PARTIAL.**

The auth shape required for Quorum Phase 1A is "a single header value
the CLI can persist in the OS keychain and send on every request." The
cleanest match to that requirement is Bearer-token acceptance on
`/api/v1/*`. Lippa already has the storage column (`User.extension_token`),
the regeneration UI (`/settings/mcp`), and the Bearer-validation logic
(`mcp.py:_authenticate`) — they just aren't wired together at
`/api/v1/*`.

**Lippa-side scope** (small spec — 1-2 weeks of CC work):

1. Lift `_authenticate` from `mcp.py` into a shared dependency
   `app/dependencies.py::optional_bearer_auth` returning `CurrentUser`
   on success, `None` on absence of header.
2. In `require_auth`, fall back to `optional_bearer_auth` when
   `request.session.get("user_id")` is absent. Keep cookie path as the
   default for the React frontend.
3. Single passing integration test per `/api/v1/*` route family
   (consensus, memory/propose, memory-explorer) demonstrating
   Bearer-only auth works.
4. `/api/v1/auth/me` returns the same payload regardless of which auth
   path was used.
5. No new token format. `User.extension_token` UUID is the token.
   (If a typed prefix like `lpa_` is desired, that becomes a separate
   migration + format-validation spec — explicitly out of scope for the
   minimal version.)

**Quorum-side, Phase 1A in parallel:**

- Build the CLI against cookie auth as the temporary mechanism. CLI
  prompts for email/password once, POSTs `/api/v1/auth/login`, stores
  the `session` cookie in the OS keychain via the `keyring` crate.
  Sends it on every request. Refresh by re-prompting on 401.
- Architect the auth layer in `quorum-lippa-client` so the only
  difference between cookie and Bearer modes is one constructor flag —
  swap is one-line at the call site once Lippa-side ships.
- This unblocks Phase 1A immediately. The Bearer migration is a
  dependency upgrade, not a rewrite.

**If Lippa-side cannot ship the minimal spec within Phase 1A's window**,
fall back to GO-COOKIE permanently for v1 and revisit at Phase 2. The
cookie path works; it's just brittle.

---

## Lippa-side specs Quorum needs

| ID (proposed) | Scope (one line) | Priority |
|---|---|---|
| **M-ExternalAuth-MinimalScope** | Extend `User.extension_token` Bearer auth to `/api/v1/*` via fallback in `require_auth` (steps 1-5 above). | **P0** — gates GO-PAT-PARTIAL branch. |
| Spec-MemoryProposeExternalClient | Optional auto-mint of synthetic `(conversation_id, turn_id)` for external-client memory writes, OR a relaxed user-presence rule when `proposed_by_model` matches an allowlisted external client (e.g. `quorum-*`). | P1 — without this, Quorum must spin up a sham Lippa conversation per review. Workable but ugly. |
| Spec-MemorySimilarityEndpoint | `POST /api/v1/projects/{id}/memory/similar` taking text/embedding, returning top-N items by cosine. | P2 — Quorum can do this client-side for v1. Promote if/when client-side embedding latency is the bottleneck. |
| Spec-ConsensusUsageEvents (followup) | Optional: emit `usage_events` row from `consensus_service` for unified billing dashboard. Not blocking Quorum. | P3 — Lippa-internal cleanliness, not a Quorum dependency. |

---

## Confidence level

**HIGH** —
- §1.1 endpoint × auth × schema matrix (direct file read of all 8 routes).
- §1.2 memory propose auth + required fields + rate-limit shape (direct
  file read).
- §1.3 PAT state = C (three independent greps, all empty).
- §2.B viability (POST /memory/propose covered).
- §2.C viability (multipart form confirmed).
- §2.D per-model raw findings (rounds_data emitter at consensus.py:275-287).
- §2.E cost attribution chain (consensus_credits decrement at consensus.py:419,
  refund sites in consensus_service.py, no usage_events writes from
  consensus path).

**MEDIUM** —
- §2.A "top-N by similarity to a code diff" — confirmed *absent* by
  grep of api_v1 routes. Inference is well-supported; the workaround
  (client-side embedding) is the standard answer but its latency
  characteristics on large project memories were not measured.
- §1.2 rate-limit numeric values (3/24h) read from spec docstring at
  memory.py:144-146; not directly verified against
  `direct_memory_service.MODEL_PROPOSED_RATE_LIMIT` constant (file not
  read).

**LOW** — none. Where evidence was missing, the finding states "not
verified" rather than inferring.

---

## Queries that require Rolf approval

None for this recon. All findings are static-analysis from file reads
+ greps. No DB queries were run, no Lippa-side state was modified.

---

## Constraints adherence checklist

- [x] No modifications to any file in `../lippa`.
- [x] No code execution. Static analysis only.
- [x] No DB queries.
- [x] Every finding cites a file path + line number, OR explicitly
      labels itself as inferred.
- [x] Single deliverable file at `specs/Quorum-Recon-v0_findings.md`.
