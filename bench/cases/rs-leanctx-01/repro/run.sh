#!/bin/sh
# Run from the repo root inside rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1
# (fixed tree = case base, the fixed tree minus its tests with Gate B comment
# edits, plus repro/files; reverted = case commit tree + repro/files).
# Single-threaded: upstream tests race on LEAN_CTX_SHELL_* environment
# variables under parallel test threads (see case.toml notes).
set -e
cargo test --manifest-path rust/Cargo.toml --lib core::shell_allowlist::tests -- --test-threads=1
