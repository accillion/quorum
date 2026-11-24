#!/bin/sh
# Run from the repo root inside rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1
# (rust:1.95-bookworm; fixed tree = source_commit; reverted = case commit tree + repro/files copied in).
set -e
LEAN_CTX_EMBEDDINGS_AUTO_DOWNLOAD=0 cargo test --locked --manifest-path rust/Cargo.toml --lib dashboard::tests -- --test-threads=1
