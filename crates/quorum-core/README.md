# quorum-core

The Quorum review pipeline as a library: bundle assembly, deny-list,
git2-based staged-diff and commit-range diff capture, conventions
reader, JSON archive writer, wire-format mapping, and (Phase 1B+) the
local SQLite dismissals memory store.

Synchronous; no async runtime, no HTTP client. The Lippa-facing
network plumbing lives in
[`quorum-lippa-client`](https://crates.io/crates/quorum-lippa-client).

The mapping site `review_from_json` is the only legal point at which
Lippa's wire format becomes a `Review` value — wire-format dispatch
extension lives here.

For the user-facing `quorum` CLI, see
[`quorum-cli`](https://crates.io/crates/quorum-cli) and the
[repository root README](https://github.com/accillion/quorum#readme).

License: Apache-2.0.
