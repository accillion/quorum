//! Phase 1C Stage 3 — Lippa forward-compat seam (AC 166).
//!
//! Spec §7.4 asserts that **no Phase 1C code path issues a call to
//! `/api/v1/memory/propose`**. Cloud sync is Phase 1D scope; introducing
//! the endpoint into the client surface before then would silently widen
//! the call-site set this test guards.
//!
//! Implementation: structural source-string scan of every `.rs` file
//! under `crates/quorum-lippa-client/src/`. If a future change adds the
//! literal `/api/v1/memory/propose` anywhere in the client surface, this
//! test fails and forces a deliberate review — which is the intended
//! behavior.
//!
//! Brittleness note: a test fixture or comment that legitimately needs
//! to mention the literal would also trip this test. That's by design.
//! When Phase 1D opens the cloud-write path, this test gets updated in
//! the same commit that adds the call.

use std::fs;
use std::path::{Path, PathBuf};

const FORBIDDEN_ENDPOINT: &str = "/api/v1/memory/propose";

fn lippa_client_src_dir() -> PathBuf {
    // `CARGO_MANIFEST_DIR` at test time is the crate root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn collect_rs_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).expect("read src dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            out.extend(collect_rs_files(&path));
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(path);
        }
    }
    out
}

#[test]
fn quorum_lippa_client_does_not_reference_memory_propose() {
    let src = lippa_client_src_dir();
    let files = collect_rs_files(&src);
    assert!(
        !files.is_empty(),
        "expected at least one .rs file under {:?}",
        src
    );
    let mut hits: Vec<(PathBuf, usize)> = Vec::new();
    for f in &files {
        let body = fs::read_to_string(f).expect("read source file");
        for (idx, line) in body.lines().enumerate() {
            if line.contains(FORBIDDEN_ENDPOINT) {
                hits.push((f.clone(), idx + 1));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "AC 166: `{}` must not appear in the quorum-lippa-client \
         call-site set. Hits: {:?}. If Phase 1D needs this endpoint, \
         update this test in the same commit.",
        FORBIDDEN_ENDPOINT,
        hits
    );
}

#[test]
fn quorum_lippa_client_endpoint_set_is_phase_1a_1b_only() {
    // Phase 1A/1B surface — the allowed-endpoint set. Stage 3 doesn't
    // mechanise this set into a typed enum (none exists yet); we just
    // assert that every `/api/v1/...` literal in the client source
    // matches one of the documented endpoint prefixes. New endpoints
    // would surface here and force a review.
    let allowed_prefixes = [
        "/api/v1/auth/login",
        "/api/v1/auth/logout",
        "/api/v1/me",
        "/api/v1/consensus/sessions",
    ];
    let src = lippa_client_src_dir();
    let files = collect_rs_files(&src);
    for f in &files {
        let body = fs::read_to_string(f).expect("read source file");
        for (idx, line) in body.lines().enumerate() {
            // Find `/api/v1/...` substrings and check the prefix is
            // one of the allowed ones.
            let mut search_start = 0;
            while let Some(rel) = line[search_start..].find("/api/v1/") {
                let abs = search_start + rel;
                let rest = &line[abs..];
                // Extract up to the next non-path char (quote, space,
                // backtick, brace, paren, comma, etc.).
                let end_rel = rest
                    .char_indices()
                    .find(|(_, c)| !matches!(c, '/' | '_' | '-' | '.' | 'a'..='z' | 'A'..='Z' | '0'..='9'))
                    .map(|(i, _)| i)
                    .unwrap_or(rest.len());
                let endpoint = &rest[..end_rel];
                let matched = allowed_prefixes
                    .iter()
                    .any(|prefix| endpoint.starts_with(prefix));
                assert!(
                    matched,
                    "{}:{idx}: unknown Lippa endpoint reference {endpoint:?}; \
                     allowed prefixes are {:?}. Either it's a real new endpoint \
                     (review required) or the test needs an update.",
                    f.display(),
                    allowed_prefixes
                );
                search_start = abs + end_rel.max(1);
            }
        }
    }
}
