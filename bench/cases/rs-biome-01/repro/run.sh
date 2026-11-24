#!/bin/sh
# Reference only: the harness never runs this.
# Image: rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1 (rustup installs the toolchain pinned by the repo's rust-toolchain.toml).
# Checkout: the fixed tree (36d5aa77d751bf33485cd2c92d89e4c5764e1e50) or the case commit tree (c9238bfc731f55c916ba300b05aa8913a9aa6a2f)
# with repro/files copied over the repo root. Env: CARGO_TERM_COLOR=never INSTA_UPDATE=no CI=true
cargo test --locked -p biome_html_analyze --test spec_tests -- specs::correctness::use_vue_valid_v_bind
