//! `finding_identity_hash` — SHA-256 anchor that lets dismissals survive
//! across Lippa sessions of the same logical finding.
//!
//! ## Three-input hash (post-adjudication D2/D3)
//!
//! The shipped inputs are:
//!   1. `title`         — NFC + lowercase + whitespace-collapse + trim
//!   2. `source.type`   — "divergence" | "agreement" | "assumption" literal
//!   3. `source.models` — alphabetized, joined with `0x1F`
//!
//! Encoding: each field's UTF-8 bytes, separated by `0x1F`. No
//! length-prefix needed: only `source.models` is multi-value and its
//! elements are also joined with `0x1F`, which can collide with the
//! field separator on adversarial input — but `source.models` is a
//! fixed-domain set of model identifiers (`gpt-4o`, `claude-sonnet`,
//! `gemini-pro`, ...) that never contain `0x1F`. The simpler scheme is
//! safe for the actual input domain.
//!
//! ## What the spec (v1.0 §4.2.4) originally specified
//!
//! v1.0 §4.2.4 specified a four-input hash that added a 280-byte
//! `body_opener` derived from `Finding.body`, plus a P39 conditional
//! that would swap `file` in if Lippa exposed it. Phase 1B preflight
//! D2/D3 (`specs/Quorum-Phase1B-Preflight-notes.md`) discovered:
//!
//!   - Lippa's wire format exposes no `file` per cluster → P39 swap
//!     has no anchor; the conditional is dead.
//!   - `cluster_id` is session-random (0% stable across 5 reruns of
//!     the same diff) → not usable.
//!   - `Finding.body` in Phase 1A is synthesized metadata, not rich
//!     prose; the body-opener premise was wrong about the input shape.
//!   - `claim_text` itself drifts ~100% on low-signal diffs → tightening
//!     the cutoff would not help.
//!
//! Adjudication: drop `body_opener`, drop P39, ship the 3-input hash.
//!
//! ## Residual cross-finding collision risk
//!
//! Two distinct logical findings with the same `(title, source.type,
//! sorted_models)` triple will collide. The `cross_finding_collision_rate`
//! integration test asserts the rate stays ≤2% on the v1.0 fixture set;
//! see `crates/quorum-cli/tests/dismissals_filter.rs`.

use sha2::{Digest, Sha256};

use crate::review::{Finding, FindingSource};

/// 32-byte SHA-256 anchor. Hex-encoded for SQLite storage; raw bytes for
/// in-memory comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct FindingIdentityHash(pub [u8; 32]);

impl FindingIdentityHash {
    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for b in &self.0 {
            s.push_str(&format!("{:02x}", b));
        }
        s
    }

    pub fn from_hex(hex: &str) -> Option<Self> {
        if hex.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
        }
        Some(FindingIdentityHash(out))
    }
}

const FIELD_SEP: u8 = 0x1F;

/// Three-input identity hash per the D2/D3 adjudication.
///
/// Inputs (in order):
///   1. `finding.title` — NFC + lowercase + whitespace-collapse + trim
///   2. `finding.source` as one of the three literal strings
///   3. `finding.supported_by` — `.to_lowercase()` + sort + join with `0x1F`
///
/// Hash: SHA-256 over the three fields, each separated by `0x1F`.
pub fn finding_identity_hash(finding: &Finding) -> FindingIdentityHash {
    let title = normalize_title(&finding.title);
    let kind = source_kind(finding.source);
    let mut models: Vec<String> = finding
        .supported_by
        .iter()
        .map(|s| s.to_lowercase())
        .collect();
    models.sort();
    models.dedup();
    let joined_models = models.join(std::str::from_utf8(&[FIELD_SEP]).unwrap());

    let mut hasher = Sha256::new();
    hasher.update(title.as_bytes());
    hasher.update([FIELD_SEP]);
    hasher.update(kind.as_bytes());
    hasher.update([FIELD_SEP]);
    hasher.update(joined_models.as_bytes());

    let mut out = [0u8; 32];
    out.copy_from_slice(&hasher.finalize());
    FindingIdentityHash(out)
}

fn source_kind(src: FindingSource) -> &'static str {
    match src {
        FindingSource::Divergence => "divergence",
        FindingSource::Agreement => "agreement",
        FindingSource::Assumption => "assumption",
    }
}

/// NFC + lowercase + whitespace-collapse + trim. Phase 1A's `Finding.title`
/// is Lippa's `claim_text` (a sentence-level claim).
fn normalize_title(s: &str) -> String {
    // NFC: Phase 1B treats `unicode-normalization` as out-of-scope to
    // avoid an extra dependency for a marginal gain (Lippa already emits
    // NFC-normalized claim text in practice). If a future case demands
    // explicit NFC, add `unicode-normalization` and call .nfc() here.
    let lower = s.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut prev_space = true;
    for ch in lower.chars() {
        if (ch as u32) < 0x20 && ch != '\t' {
            // Strip control chars except tab (which collapses to space).
            continue;
        }
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
            continue;
        }
        out.push(ch);
        prev_space = false;
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(title: &str, source: FindingSource, models: &[&str]) -> Finding {
        Finding {
            severity: crate::review::Severity::Medium,
            title: title.to_string(),
            body: "synthesized body — ignored by 3-input hash".into(),
            source,
            supported_by: models.iter().map(|s| s.to_string()).collect(),
            confidence: Some(0.9),
        }
    }

    #[test]
    fn hex_round_trip() {
        let h = finding_identity_hash(&finding("a title", FindingSource::Agreement, &["gpt-4o"]));
        let s = h.to_hex();
        assert_eq!(s.len(), 64);
        assert_eq!(FindingIdentityHash::from_hex(&s).unwrap(), h);
    }

    #[test]
    fn from_hex_rejects_bad_input() {
        assert_eq!(FindingIdentityHash::from_hex("zzz"), None);
        assert_eq!(
            FindingIdentityHash::from_hex(&"x".repeat(64)),
            None,
            "invalid hex chars"
        );
    }

    #[test]
    fn stable_across_body_and_confidence_drift() {
        // AC 114: hash unchanged when only `body` / `confidence` differ.
        let a = Finding {
            confidence: Some(0.95),
            body: "Confidence: 0.95. Supported by: alpha, beta.".into(),
            ..finding(
                "Divergence on X",
                FindingSource::Divergence,
                &["alpha", "beta"],
            )
        };
        let b = Finding {
            confidence: Some(0.71),
            body: "Confidence: 0.71. Supported by: alpha, beta.".into(),
            ..finding(
                "Divergence on X",
                FindingSource::Divergence,
                &["alpha", "beta"],
            )
        };
        assert_eq!(finding_identity_hash(&a), finding_identity_hash(&b));
    }

    #[test]
    fn changes_when_models_differ() {
        let a = finding("same title", FindingSource::Agreement, &["alpha", "beta"]);
        let b = finding(
            "same title",
            FindingSource::Agreement,
            &["alpha", "beta", "gamma"],
        );
        assert_ne!(finding_identity_hash(&a), finding_identity_hash(&b));
    }

    #[test]
    fn models_order_does_not_matter() {
        let a = finding("same title", FindingSource::Agreement, &["alpha", "beta"]);
        let b = finding("same title", FindingSource::Agreement, &["beta", "alpha"]);
        assert_eq!(finding_identity_hash(&a), finding_identity_hash(&b));
    }

    #[test]
    fn models_case_normalized() {
        let a = finding(
            "title",
            FindingSource::Agreement,
            &["Claude-Sonnet", "gpt-4o"],
        );
        let b = finding(
            "title",
            FindingSource::Agreement,
            &["claude-sonnet", "GPT-4O"],
        );
        assert_eq!(finding_identity_hash(&a), finding_identity_hash(&b));
    }

    #[test]
    fn changes_when_source_type_differs() {
        let a = finding("title", FindingSource::Agreement, &["alpha"]);
        let b = finding("title", FindingSource::Divergence, &["alpha"]);
        assert_ne!(finding_identity_hash(&a), finding_identity_hash(&b));
    }

    #[test]
    fn whitespace_collapse_and_lowercase_in_title() {
        let a = finding(
            "  Race  Condition\n in cache  ",
            FindingSource::Divergence,
            &["m"],
        );
        let b = finding("race condition in cache", FindingSource::Divergence, &["m"]);
        assert_eq!(finding_identity_hash(&a), finding_identity_hash(&b));
    }

    #[test]
    fn confidence_drift_does_not_change_hash() {
        // Belt-and-braces: confidence is excluded entirely.
        let mut a = finding("t", FindingSource::Assumption, &[]);
        let mut b = a.clone();
        a.confidence = Some(0.5);
        b.confidence = Some(0.99);
        assert_eq!(finding_identity_hash(&a), finding_identity_hash(&b));
    }
}
