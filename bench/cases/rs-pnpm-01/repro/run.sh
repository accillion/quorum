#!/bin/sh
# Run from the repo root inside rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1
# (fixed tree = source_commit; reverted = case commit tree + repro/files copied in).
set -e
cargo test --locked -p pacquet-resolving-npm-resolver --lib -- fetch_full_metadata_cached::tests
