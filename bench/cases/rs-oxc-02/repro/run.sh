#!/bin/sh
# Reference only: the harness never runs this.
# Image: rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1 (rustup installs the toolchain pinned by the repo's rust-toolchain.toml).
# Checkout: the fixed tree (407b72500e90ebdf5594b1e695fe80e316942897) or the case commit tree (950d9b40cbe756e263ab52e11e1d5ce309753bd1)
# with repro/files copied over the repo root. Env: CARGO_TERM_COLOR=never INSTA_UPDATE=no CI=true
cargo test --locked -p oxc_formatter --test mod -- --exact 'fixtures::ts::comments::r#enum'
