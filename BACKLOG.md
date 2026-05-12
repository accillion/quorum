# Quorum — Backlog

Tickets surfaced mid-session that the active milestone doesn't cover.
Group by next-target version. Most-recent first.

---

## 0.2.1 (release engineering)

- Enable cargo-dist sigstore attestation (`github-attestations = true`)
  and `publish-jobs` in `dist-workspace.toml`.
  - Closes AC 132 fully (currently PARTIAL in v0.2.0 — SHA256SUMS
    shipped, sigstore deferred).
  - Requires `CARGO_REGISTRY_TOKEN` GitHub Actions secret on the
    repo for `publish-jobs`.
