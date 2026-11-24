#!/bin/sh
# Reproduction for ts-workers-01, run from the repo root inside
# docker.io/library/node:20-bookworm@sha256:8f693eaa7e0a8e71560c9a82b55fd54c2ae920a2ba5d2cde28bac7d1c01c9ba5
# with CI=1 and COREPACK_ENABLE_DOWNLOAD_PROMPT=0 in the environment.
# Fixed run: tree of 7e5585dbf844fda0e1688797ce31c7e634f3f4ba (already contains repro/files).
# Reverted run: case_commit_tree with repro/files copied in at their repo paths.
# The script's exit status is the test command's exit status.
set -e
corepack enable && pnpm install --frozen-lockfile && pnpm turbo build --filter=miniflare
pnpm -F miniflare build && cd packages/miniflare && pnpm exec ava test/plugins/d1/index.spec.ts
