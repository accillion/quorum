#!/bin/sh
# Reference only: the harness never runs this.
# Image: rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1 (rustup installs the toolchain pinned by the repo's rust-toolchain.toml).
# Checkout: the fixed tree (3c6513d4e9a82a195785144caa9d96093c3861ff) or the case commit tree (973d34f241a39ff39223707a29c6e50928a31a14)
# with repro/files copied over the repo root. Env: CARGO_TERM_COLOR=never INSTA_UPDATE=no CI=true
cargo test --locked -p biome_html_analyze --test spec_tests -- specs::correctness::use_vue_valid_v_on::
