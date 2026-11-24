#!/bin/sh
# Reproduction for ts-agentsjs-02, run from the repo root inside
# docker.io/library/node:22-bookworm@sha256:e13726ca604184d7a91fcacab1fc09eefd642dc28e79ac34e85540dd93b78cfb
# with CI=1 and COREPACK_ENABLE_DOWNLOAD_PROMPT=0 in the environment.
# Fixed run: tree of a0b1f3b659df112d67d75cdb291f31426a080504 (already contains repro/files).
# Reverted run: case_commit_tree with repro/files copied in at their repo paths.
# The script's exit status is the test command's exit status.
set -e
corepack enable && pnpm install --frozen-lockfile && pnpm build
pnpm vitest run packages/agents-realtime/test/realtimeSession.test.ts
