#!/bin/sh
# Reproduction for ts-qwen-01, run from the repo root inside
# docker.io/library/node:20-bookworm@sha256:8f693eaa7e0a8e71560c9a82b55fd54c2ae920a2ba5d2cde28bac7d1c01c9ba5
# with CI=1 and COREPACK_ENABLE_DOWNLOAD_PROMPT=0 in the environment.
# Fixed run: tree of 6f29d24fb90504aa486b4dd78b1fae2593068c9f (already contains repro/files).
# Reverted run: case_commit_tree with repro/files copied in at their repo paths.
# The script's exit status is the test command's exit status.
set -e
npm ci && npm run build
npm run test --workspace=packages/cli -- src/ui/hooks/useStatusLine.test.ts
