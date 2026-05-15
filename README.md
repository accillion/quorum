# Quorum

[![crates.io](https://img.shields.io/crates/v/quorum-cli.svg)](https://crates.io/crates/quorum-cli)

Multi-model code reviewer for the developer's machine.

Quorum reviews staged git diffs using consensus across frontier LLMs (via
[Lippa](https://app.lippa.ai)) plus codebase memory accumulated from prior reviews.

**Status:** Phase 1C **shipped at `v0.3.0`** — conventions-promotion
state machine (`candidate` → `local_only` → `promoted_convention`),
`quorum convention` CLI subcommand group, TUI dismissal-history view
with promote/demote modals, and `## Local conventions (auto-derived)`
bundle subsection. Phase 1B's interactive dismiss TUI, hook installer,
non-interactive CI auth, prebuilt binaries for 5 platforms, crates.io
publish, and sigstore attestation (AC 132 LIVE since v0.2.1) all
continue to ship. See [`HISTORY.md`](./HISTORY.md) for the milestone
log and [`SERVICES.md`](./SERVICES.md) for service-level behavioural
rules. For per-version change details, see
[GitHub Releases](https://github.com/accillion/quorum/releases).

**Upstream:** consumes Lippa via its public `/api/v1/*` surface. Quorum
makes no Lippa-side changes. See [`CLAUDE.md`](./CLAUDE.md) for full
project context.

## Documentation

API docs are published on docs.rs:

- [`quorum-core`](https://docs.rs/quorum-core) — review pipeline,
  aggregator, memory loop.
- [`quorum-lippa-client`](https://docs.rs/quorum-lippa-client) — Lippa
  API client (Consensus, memory).
- [`quorum-cli`](https://docs.rs/quorum-cli) — binary entry point,
  command parsing, output, hooks.

## Build

Requires Rust stable (`rustup` recommended; `winget install
Rustlang.Rustup` on Windows).

On Linux, the OS keyring path links against the system D-Bus / Secret
Service development headers. Install before building if you intend to
use the keyring (omit if you only ever pass `--no-keyring`):

```bash
sudo apt-get install libdbus-1-dev pkg-config   # Debian / Ubuntu
sudo dnf install dbus-devel pkgconf-pkg-config  # Fedora / RHEL
```

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

By default, `auth login` requires an interactive terminal. For CI flows,
see "Non-interactive auth" below. The session
cookie goes into your OS keychain (Windows Credential Manager / macOS
Keychain / Linux Secret Service). On headless boxes pass `--no-keyring`
to fall back to a per-host file at `~/.config/quorum/sessions/<host>.session`
(mode 0600 on Unix); a stderr warning is emitted at login.

## Running a review

```bash
git add <files-you-want-reviewed>
quorum review                  # markdown to stdout, archive to .quorum/reviews/<ISO>.json
quorum review --json           # same buffer to stdout AND to disk (byte-identical)
quorum review --tui            # interactive TUI: navigate, dismiss with reason, undo (Phase 1B)
quorum review --range HEAD~3..HEAD   # review a commit range instead of staged diff
quorum review --no-expire      # dismissals from this session do not auto-expire (default: 365d)
```

The TUI surfaces findings in a list + body pane with a status bar.
Keys: `j`/`k` navigate, `g`/`G` jump to first/last, `PgDn`/`PgUp`
scroll the body, `d` (or `Enter`) opens the dismiss-reason prompt
(`f` false positive, `i` intentional, `s` out of scope, `w` won't
fix, `o` free-text), `u` undoes the most recent in-session dismissal
(unbounded undo stack), `q` or `Esc` quits, `?` shows help. Body
rendering is plain wrapped text — no markdown styling parser. The
terminal is restored on every exit path including panic.

Subsequent reviews suppress findings matching active dismissals via
the local `.quorum/dismissals.sqlite` store (auto-gitignored on
first creation). Dismissed findings appear in the archive's
`suppressed_findings[]` audit trail with hash + title + reason +
timestamp — never the free-text note.

## Git hooks

```bash
quorum install --hook=pre-commit    # writes .git/hooks/pre-commit
quorum install --hook=pre-push      # writes .git/hooks/pre-push
quorum uninstall --hook=pre-commit  # idempotent; refuses non-Quorum hooks
```

Hook templates are POSIX `#!/bin/sh`, marked with
`# quorum-managed-hook v1` in the first 5 lines. Re-running `install`
overwrites a Quorum-managed file idempotently; refuses to overwrite
a hook of unknown provenance.

Hook policy via `QUORUM_HOOK_POLICY` env var:
- `fail-open` (default): only exit 1 (high-severity findings) blocks
  the commit / push. Auth and tooling failures fail open.
- `fail-closed`: also block on exit 2 (tooling) and exit 3 (auth).
- `warn`: nothing blocks regardless of exit code.

Bypass for one operation: `env QUORUM_SKIP=1 git commit ...` or
`env QUORUM_SKIP=1 git push ...`.

Pre-push reviews each ref tuple Git sends on stdin as a commit range
(`base..head`). Branch deletions and tag pushes are skipped with a
stderr note (code review is not meaningful for either). Each tuple
produces its own archive at
`.quorum/reviews/<push-start-ISO>.tuple-<N>.json`.

## Local conventions (Phase 1C)

Dismissals accumulate as `candidate` rows. After repeated dismissals of
the same finding (per `[memory]` config thresholds), Quorum auto-promotes
them to `local_only` — surfaced inline in the bundle's
`## Local conventions (auto-derived)` subsection so future reviews see
the convention without needing the dismissal history.

```bash
quorum convention list                  # all local conventions
quorum convention list --state candidate
quorum convention list --json
quorum convention show <short-hash>     # full text + transition history
quorum convention history <short-hash>  # state transitions only
quorum convention promote --text "..." <short-hash>   # local_only → promoted (writes .quorum/conventions.md)
quorum convention promote --from-editor <short-hash>  # opens $EDITOR
quorum convention demote <short-hash>                 # promoted → local_only
quorum convention prune --dry-run       # candidates older than candidate_expire_days
quorum convention prune --yes
```

Promoted conventions land in `.quorum/conventions.md` as a managed
section between fence markers; the file is byte-preservation-safe
and the rest of the file is yours. The TUI's `H` view shows the
dismissal-history pane; `p` opens a promote modal on `local_only`
rows and `Shift+D` opens a demote confirmation on `promoted_convention`
rows. See `--help` on each subcommand for full flag detail.

## Install

**v0.3.0** is the current release. Pick the path that fits.

### Cargo

```bash
cargo install quorum-cli
quorum --version    # quorum 0.3.0 (unknown)
```

The `(unknown)` SHA is by design — crates.io tarballs ship without
a `.git/` directory, so the embedded build-SHA falls back to the
literal string `unknown`. Local source builds embed the actual SHA.

### Prebuilt binaries (cargo-dist)

Five platform builds attached to each release, each with a per-file
SHA256 sidecar plus an aggregate `sha256.sum`:

- `quorum-cli-x86_64-unknown-linux-gnu.tar.xz`
- `quorum-cli-aarch64-unknown-linux-gnu.tar.xz`
- `quorum-cli-x86_64-apple-darwin.tar.xz`
- `quorum-cli-aarch64-apple-darwin.tar.xz`
- `quorum-cli-x86_64-pc-windows-msvc.zip` + `.msi`

Direct download from
[the v0.3.0 release](https://github.com/accillion/quorum/releases/tag/v0.3.0),
or one-line install via the cargo-dist installers also attached to
the release:

```bash
# Linux / macOS
curl --proto '=https' --tlsv1.2 -LsSf \
    https://github.com/accillion/quorum/releases/latest/download/quorum-cli-installer.sh | sh

# Windows (PowerShell)
powershell -ExecutionPolicy ByPass -c "irm https://github.com/accillion/quorum/releases/latest/download/quorum-cli-installer.ps1 | iex"
```

### Homebrew

A formula (`quorum-cli.rb`) is built by cargo-dist and attached to
each release. A Homebrew tap is **not yet published**; until it is,
install via `cargo install` or the prebuilt binaries above.

### Verifying downloads

```bash
sha256sum -c sha256.sum             # Linux / macOS
Get-FileHash -Algorithm SHA256 ...  # Windows
```

Sigstore attestation is **live** for every release from v0.2.1
onward (cargo-dist `github-attestations = true`, `actions/attest-build-provenance@v3`).
Verify any release asset with:

```bash
gh attestation verify quorum-cli-<target>.tar.xz --owner accillion
```

Expect a Fulcio-signed SLSA provenance v1 attestation tying the
asset to a specific `release.yml@refs/tags/<version>` build, with
an RFC3161 timestamp from `timestamp.githubapp.com`. SHA256
verification against the per-file sidecars or the aggregate
`sha256.sum` remains supported as an independent integrity check.

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
- `.quorum/dismissals.sqlite` (plus `-wal` / `-shm` sidecars) —
  written on first dismissal via TUI or CLI. Auto-appended to
  `.gitignore` on first creation. Contains finding identity hashes,
  title snapshots, dismissal reasons, and any free-text notes you
  type at the dismiss prompt. Never holds session cookies, passwords,
  or any other auth material.
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
