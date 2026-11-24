#!/bin/sh
# Reproduction for ts-agentsjs-03, run from the repo root inside
# docker.io/library/node:22-bookworm@sha256:e13726ca604184d7a91fcacab1fc09eefd642dc28e79ac34e85540dd93b78cfb
# with CI=1 and COREPACK_ENABLE_DOWNLOAD_PROMPT=0 in the environment.
# Fixed run: tree of 94f606ce3715a9fa8887aeb035f7d7d2d6e5226c (already contains repro/files).
# Reverted run: case_commit_tree with repro/files copied in at their repo paths.
# The script's exit status is the test command's exit status.
set -e
corepack enable && pnpm install --frozen-lockfile && pnpm build
pnpm vitest run packages/agents-core/test/run.stream.test.ts packages/agents-core/test/run.test.ts
