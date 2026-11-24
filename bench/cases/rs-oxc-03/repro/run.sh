#!/bin/sh
# Reference only: the harness never runs this.
# Image: rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1 (rustup installs the toolchain pinned by the repo's rust-toolchain.toml).
# Checkout: the fixed tree (d676e0ca5b9431976ea1ba201547c659ffbf34a9) or the case commit tree (28ce599db5bb8213e6c5adf9f08a3c4a27eb3dd8)
# with repro/files copied over the repo root. Env: CARGO_TERM_COLOR=never INSTA_UPDATE=no CI=true
cargo test --locked -p oxc_minifier --test mod -- peephole::remove_unused_expression
