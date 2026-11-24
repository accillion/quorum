#!/bin/sh
# Reproduction for ts-defuddle-01, run from the repo root inside
# docker.io/library/node:22-bookworm@sha256:e13726ca604184d7a91fcacab1fc09eefd642dc28e79ac34e85540dd93b78cfb
# with CI=1 and COREPACK_ENABLE_DOWNLOAD_PROMPT=0 in the environment.
# Fixed run: tree of 0fc57fab6a1a91e7d30cae68febad953d075fb95 (already contains repro/files).
# Reverted run: case_commit_tree with repro/files copied in at their repo paths.
# The script's exit status is the test command's exit status.
set -e
npm ci
TZ=UTC npx vitest run tests/fixtures.test.ts
