# Quorum

Multi-model code reviewer for the developer's machine.

Quorum reviews staged git diffs using consensus across frontier LLMs (via Lippa) plus codebase memory accumulated from prior reviews. Conventions live as a tracked, version-controlled file in your repo (`.quorum/conventions.md`); dismissals with reasons feed back into the reviewer so it learns your codebase's settled debates.

**Status:** pre-Phase-1A. Recon complete; Phase 1A spec in flight.

**Upstream:** consumes Lippa (https://app.lippa.ai) via public API. No Lippa code modifications. See `CLAUDE.md` for full project context.

## License

MIT — see [LICENSE](./LICENSE).