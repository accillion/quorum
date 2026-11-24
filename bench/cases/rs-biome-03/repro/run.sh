#!/bin/sh
# Reference only: the harness never runs this.
# Image: rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1 (rustup installs the toolchain pinned by the repo's rust-toolchain.toml).
# Checkout: the fixed tree (098ba41c99e6efaac8eb182eec258a567bb00123) or the case commit tree (738f25af2d126aaa51031ec9d7cb6635f1f54da3)
# with repro/files copied over the repo root. Env: CARGO_TERM_COLOR=never INSTA_UPDATE=no CI=true
cargo test --locked -p biome_js_analyze --test spec_tests -- specs::correctness::no_unsafe_optional_chaining
