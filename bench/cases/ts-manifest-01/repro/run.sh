#!/bin/sh
# Reproduction for ts-manifest-01, run from the repo root inside
# docker.io/library/node:22-bookworm@sha256:e13726ca604184d7a91fcacab1fc09eefd642dc28e79ac34e85540dd93b78cfb
# with CI=1 in the environment.
# Fixed run: tree of 837799bf8ecff20b12e7a516dd816cd00d7b6210 (already contains repro/files).
# Reverted run: case_commit_tree with repro/files copied in at their repo paths
# (dependencies installed once, on the fixed tree; no package file differs).
# The script's exit status is the test command's exit status.
set -e
npm ci && npm run build --workspace=packages/shared
npm test --workspace=packages/backend -- src/routing/proxy/__tests__/provider-client.spec.ts
