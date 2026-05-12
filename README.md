# Quorum

Multi-model code reviewer for the developer's machine.

Quorum reviews staged git diffs using consensus across frontier LLMs (via
[Lippa](https://app.lippa.ai)) plus codebase memory accumulated from prior reviews.

**Status:** Phase 1A walking skeleton — `quorum auth login`, `quorum link`,
`quorum review` work end-to-end. Interactive dismissal, hook installer,
binary distribution, and memory write-back are Phase 1B+. See
[`HISTORY.md`](./HISTORY.md) for milestone history and
[`SERVICES.md`](./SERVICES.md) for service-level behavioural rules.

**Upstream:** consumes Lippa via its public `/api/v1/*` surface. Quorum
makes no Lippa-side changes. See [`CLAUDE.md`](./CLAUDE.md) for full
project context.

## Build

Requires Rust stable (`rustup` recommended; `winget install
Rustlang.Rustup` on Windows).

```bash
git clone https://github.com/accillion/quorum
cd quorum
cargo build --release
# binary at ./target/release/quorum(.exe)
```

## First-time setup

```bash
# 1. Authenticate against your Lippa instance (interactive, TTY required).
./target/release/quorum auth login --url https://app.lippa.ai

# 2. Bind the current repo to a Lippa project.
cd path/to/your/repo
quorum link --project <your-lippa-project-id>
# Writes .quorum/config.toml. Commit it if you want teammates to share
# the same Lippa project binding; .gitignore it otherwise.
```

`auth login` requires an interactive terminal in Phase 1A. The session
cookie goes into your OS keychain (Windows Credential Manager / macOS
Keychain / Linux Secret Service). On headless boxes pass `--no-keyring`
to fall back to a per-host file at `~/.config/quorum/sessions/<host>.session`
(mode 0600 on Unix); a stderr warning is emitted at login.

## Running a review

```bash
git add <files-you-want-reviewed>
quorum review                  # markdown to stdout, archive to .quorum/reviews/<ISO>.json
quorum review --json           # same buffer to stdout AND to disk (byte-identical)
```

Exit codes:
- `0` — review completed; no high-severity findings (or auth/link command succeeded).
- `1` — review completed; one or more high-severity findings.
- `2` — tooling error (missing config, oversized bundle, terminal session state, network).
- `3` — authentication required or expired. Stderr distinguishes
  create-401 (run `quorum auth login`) from poll-401 (session ended
  mid-review).

## What gets sent to Lippa (privacy note)

Quorum's bundle is your *staged* diff plus context wrapped in
delimiters. Specifically:

- **Diff body** (HEAD vs index, 3 context lines, ≤100 KB) and the
  contents of changed files (from index blobs, ≤80 KB total).
- **Repo memory** — first of `CLAUDE.md` / `AGENTS.md` / `.cursorrules`
  that exists, ≤20 KB.
- **Conventions** — `.quorum/conventions.md` if it's committed AND
  byte-identical to HEAD (untracked / uncommitted-changes versions are
  ignored), ≤10 KB.
- **Repo metadata** — branch name, head SHA, optional remote URL
  (HTTPS credentials stripped; opt out with `quorum link --no-remote-url`).

The following are NEVER sent:
- Unstaged hunks of partially-staged files (the diff comes from index
  blobs, not the working tree).
- Untracked files.
- Files matching the hardcoded deny-list: `.env`, `.env.*`, `*.pem`,
  `*.key`, `*.p12`, `*.pfx`, `id_rsa`/`id_dsa`/`id_ecdsa`/`id_ed25519`,
  `.aws/credentials`, `.npmrc`, `.netrc`, `.pgpass`, `secrets.yml`/`.yaml`/`.json`.
  Patterns match at any depth. Excluded files appear in the bundle as
  `[excluded by deny-list: <path>]` markers, never as content.
- Binary files (null-byte detection + git2's heuristic) and files >2 MB.
- Your email, password, session cookie, or any auth artifact in the
  JSON archive on disk.

The total bundle hard cap is 200 KB. Overflow above per-section budgets
produces visible truncation markers. The deny-list is **defense-in-depth,
not a security guarantee** — a privacy-conscious user reviews what they
stage. Phase 2 introduces customizable redaction patterns.

## Files Quorum writes locally

- `.quorum/config.toml` — written by `quorum link`. Holds `project_id`,
  `base_url`, `remote_url` flag.
- `.quorum/reviews/<ISO>.json` — one file per review, deterministic
  schema (`schema_version: 1` first, alphabetical thereafter).
- OS keychain entry: service `quorum`, account `lippa-session@<host>`.

## Non-interactive auth (CI bootstrap)

Phase 1B Stage 4 adds three CI-friendly auth shapes, prioritized by
preference:

1. **`QUORUM_LIPPA_SESSION` (preferred).** Already-captured cookie
   passed directly via environment. `quorum review` consumes it
   without touching the keyring and without an `auth login` step.

   ```bash
   # one-time on a trusted workstation (interactive):
   quorum auth status --show-session -y
   # ... copy the printed value into your CI secret manager,
   # then in CI:
   export QUORUM_LIPPA_SESSION=<value>
   quorum review --hook-mode=pre-commit   # or just `quorum review`
   ```

   When both `QUORUM_LIPPA_SESSION` and a keyring entry are present,
   the env var wins. A one-line stderr note is emitted in interactive
   `quorum review` invocations only — suppressed under `--hook-mode=*`
   so CI logs stay clean.

2. **`QUORUM_LIPPA_EMAIL` + `QUORUM_LIPPA_PASSWORD` (fallback).** For
   environments without session-injection tooling. Consumed by
   `quorum auth login --non-interactive`; the cookie persists per
   the usual keyring / `--no-keyring` policy.

   ```bash
   export QUORUM_LIPPA_EMAIL=you@example.com
   export QUORUM_LIPPA_PASSWORD=...
   quorum auth login --non-interactive
   ```

3. **Interactive TTY login (default, Phase 1A behavior).** Unchanged.

## Security threat model

The non-interactive paths above trade one operational property
(unattended auth) for several attack surfaces you should be aware of.
Phase 1B `0.2.0` ships honest, narrow defenses; deeper hardening is
deferred to Phase 2+ (the redaction layer is the natural place).

**Process inspection.** On Linux any process running as the same uid
can read `/proc/<pid>/environ`. On macOS the equivalent requires
`task_for_pid` (gated by entitlements / SIP). On Windows
`OpenProcessToken` is the analog. `QUORUM_LIPPA_SESSION` and
`QUORUM_LIPPA_PASSWORD` are environment variables; they are visible
in the process's environment for the lifetime of the process.
**Mitigation:** Quorum runs as a short-lived CLI; the env values live
only as long as `quorum review` / `quorum auth login --non-interactive`
takes to complete. Long-running daemons that inherit these env vars
broaden the window — don't export them globally if you can avoid it.

**Shell history leakage.** `export QUORUM_LIPPA_SESSION=eyJ...`
written into `.bash_history` / `.zsh_history` is a common foot-gun.
**Recommendation:** in shells, read the secret from stdin with
`-s` so it never lands in history:

```bash
read -rs QUORUM_LIPPA_SESSION
export QUORUM_LIPPA_SESSION
```

Or store the secret in a file with 0600 mode and `set -a; source
~/.quorum.env; set +a` it just before the `quorum` invocation. CI
secret managers (GitHub Actions secrets, GitLab CI variables, etc.)
inject the env var directly without a shell history hop.

**Logging.** Quorum redacts all secret material at every `tracing`
level. The cookie value is wrapped in the `Secret` newtype at the
parse boundary; its `Debug` and `Display` impls emit
`Secret(<redacted>)` / `<redacted>`. The password is wrapped at the
`std::env::var` call site in non-interactive login, only `expose()`'d
once when passed to `login_with_cookie`, and explicitly dropped
immediately afterward — no in-memory mirror beyond the login call
site. Verified by the `secret_redaction.rs` suite under
`RUST_LOG=trace`.

**Persistence semantics.** Under `QUORUM_LIPPA_SESSION` mode the env
var IS the persistence — Quorum does NOT write the env-provided
cookie to the keyring. The cookie is read once per process. This
keeps your CI's "rotate the secret" workflow as a single env-var
update, not an env update plus a `quorum auth login` step.

`quorum auth logout` performs a best-effort `POST /api/v1/auth/logout`
against Lippa with a 5-second timeout, then removes the local entry.
If the server-side call fails (network drop, transient 5xx), the
cookie remains valid on the Lippa side until its 14-day natural
expiry — the local entry is gone either way. Rotate the cookie if
the host was compromised.

## Uninstalling

```bash
quorum auth logout                     # removes keychain entry
rm -rf .quorum/                        # removes per-repo state
# remove the binary you built
```

If the binary is already gone, clean up the keychain manually:
- macOS: `security delete-generic-password -s quorum`
- Linux: `secret-tool clear service quorum`
- Windows: Credential Manager → Generic Credentials → "quorum"
- `--no-keyring` users: `rm ~/.config/quorum/sessions/*.session` (or
  `%APPDATA%\quorum\sessions\*.session` on Windows).

## License

Apache-2.0 — see [LICENSE](./LICENSE).
