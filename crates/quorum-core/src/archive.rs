//! Deterministic JSON archive writer.
//!
//! Phase 1A serializes the archive object ONCE into a buffer; the buffer
//! is the source of bytes for both `.quorum/reviews/<ISO>.json` and
//! `quorum review --json` stdout. Byte-identity is by construction, not
//! by re-serialization (spec §4.3.3, AC 17 + AC 26).
//!
//! Key ordering: `schema_version` is emitted first by convention, followed
//! by remaining keys in alphabetical order. This is implemented by
//! constructing a `BTreeMap`-like ordered structure rather than relying on
//! HashMap iteration.

use crate::review::{Review, Severity};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ArchiveInputs {
    pub started_at: time::OffsetDateTime,
    pub elapsed: Duration,
    pub session_id: String,
    pub project_id: Option<String>,
    pub base_url: String,
    pub remote_url: Option<String>, // None if `remote_url = false` in config
    pub branch: String,
    pub head_sha: String,
    /// Number of dismissals that filtered out findings from this review.
    /// Equals `suppressed_findings.len()` plus any newly-dismissed-in-TUI
    /// entries (which are also in `suppressed_findings`).
    pub dismissals_applied: u32,
    /// Per-row audit trail of dismissals that hid findings (or that were
    /// applied during the TUI session). Carries hash + title + reason +
    /// timestamp — never note text, never body_snapshot. v1.0 §4.10.2.
    pub suppressed_findings: Vec<SuppressionSummary>,
}

/// What lands in the archive's `suppressed_findings[]`. v1.0 §4.10.2.
/// Note: deliberately excludes `note` and `body_snapshot` — those stay in
/// SQLite. The archive surfaces an audit trail without leaking free-text
/// dismissal notes to a file that might accidentally be committed.
#[derive(Debug, Clone)]
pub struct SuppressionSummary {
    /// Hex-encoded SHA-256, 64 chars.
    pub finding_identity_hash: String,
    /// Title at dismissal time (`title_snapshot` column).
    pub title_snapshot: String,
    /// "divergence" | "agreement" | "assumption".
    pub source_type_snapshot: String,
    /// Reason token: false_positive | intentional | out_of_scope | wont_fix | other.
    pub reason: String,
    /// RFC 3339 UTC timestamp.
    pub dismissed_at: String,
}

/// Serialize a `Review` plus inputs into a deterministic, pretty-printed
/// JSON byte buffer.  Returns `(buf, iso_started_at)`. The caller uses
/// `iso_started_at` as part of the filename (with `:` → `-` substitution).
///
/// Schema fields (alphabetical except `schema_version` first):
///   - schema_version (always 1)
///   - base_url
///   - elapsed_seconds
///   - findings[]
///   - final_agreement_score (optional)
///   - model_names[]
///   - project_id (optional)
///   - repo { branch, head_sha, remote_url? }
///   - session_id
///   - started_at
///   - summary_text (optional)
pub fn build(review: &Review, inp: &ArchiveInputs) -> Vec<u8> {
    use serde_json::json;
    let started_at = inp
        .started_at
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());

    let mut repo = serde_json::Map::new();
    repo.insert("branch".into(), json!(inp.branch));
    repo.insert("head_sha".into(), json!(inp.head_sha));
    if let Some(url) = &inp.remote_url {
        repo.insert("remote_url".into(), json!(url));
    }

    let findings_json: Vec<serde_json::Value> = review
        .findings
        .iter()
        .map(|f| {
            json!({
                "body": f.body,
                "confidence": f.confidence,
                "severity": severity_token(f.severity),
                "source": source_token(f.source),
                "supported_by": f.supported_by,
                "title": f.title,
            })
        })
        .collect();

    let suppressed_json: Vec<serde_json::Value> = inp
        .suppressed_findings
        .iter()
        .map(|s| {
            json!({
                "dismissed_at": s.dismissed_at,
                "finding_identity_hash": s.finding_identity_hash,
                "reason": s.reason,
                "source_type_snapshot": s.source_type_snapshot,
                "title_snapshot": s.title_snapshot,
            })
        })
        .collect();

    let mut top = serde_json::Map::new();
    // schema_version first by *insertion* order in the buffer.
    // We use a manual pretty-printer to enforce field order.
    top.insert("schema_version".into(), json!(2));
    top.insert("base_url".into(), json!(inp.base_url));
    top.insert("dismissals_applied".into(), json!(inp.dismissals_applied));
    top.insert("elapsed_seconds".into(), json!(inp.elapsed.as_secs_f64()));
    top.insert("findings".into(), json!(findings_json));
    if let Some(s) = review.final_agreement_score {
        top.insert("final_agreement_score".into(), json!(s));
    }
    top.insert("model_names".into(), json!(review.model_names));
    if let Some(p) = &inp.project_id {
        top.insert("project_id".into(), json!(p));
    }
    top.insert("repo".into(), serde_json::Value::Object(repo));
    top.insert("session_id".into(), json!(inp.session_id));
    top.insert("started_at".into(), json!(started_at));
    if let Some(s) = &review.summary_text {
        top.insert("summary_text".into(), json!(s));
    }
    top.insert(
        "suppressed_findings".into(),
        serde_json::Value::Array(suppressed_json),
    );

    write_ordered(
        &serde_json::Value::Object(top),
        &[
            "schema_version",
            "base_url",
            "dismissals_applied",
            "elapsed_seconds",
            "findings",
            "final_agreement_score",
            "model_names",
            "project_id",
            "repo",
            "session_id",
            "started_at",
            "summary_text",
            "suppressed_findings",
        ],
    )
}

fn severity_token(s: Severity) -> &'static str {
    match s {
        Severity::High => "high",
        Severity::Medium => "medium",
        Severity::Low => "low",
        Severity::Info => "info",
    }
}

fn source_token(s: crate::review::FindingSource) -> &'static str {
    match s {
        crate::review::FindingSource::Agreement => "agreement",
        crate::review::FindingSource::Divergence => "divergence",
        crate::review::FindingSource::Assumption => "assumption",
    }
}

/// Pretty-print a JSON value with a forced top-level key order. Nested
/// objects are alpha-sorted via a one-pass recursive sort.
fn write_ordered(value: &serde_json::Value, top_order: &[&str]) -> Vec<u8> {
    let normalized = normalize(value);
    let mut buf = Vec::with_capacity(512);
    write_value(&mut buf, &normalized, 0, Some(top_order));
    buf.push(b'\n');
    buf
}

/// Recursively turn objects into alpha-sorted `BTreeMap`-equivalent.
fn normalize(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let mut out = serde_json::Map::new();
            for k in keys {
                out.insert(k.clone(), normalize(&m[k]));
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(a) => serde_json::Value::Array(a.iter().map(normalize).collect()),
        other => other.clone(),
    }
}

fn write_indent(buf: &mut Vec<u8>, depth: usize) {
    for _ in 0..depth * 2 {
        buf.push(b' ');
    }
}

fn write_value(
    buf: &mut Vec<u8>,
    v: &serde_json::Value,
    depth: usize,
    forced_order: Option<&[&str]>,
) {
    match v {
        serde_json::Value::Object(m) => {
            if m.is_empty() {
                buf.extend_from_slice(b"{}");
                return;
            }
            buf.push(b'{');
            buf.push(b'\n');
            let keys: Vec<String> = match forced_order {
                Some(order) => order
                    .iter()
                    .map(|s| s.to_string())
                    .filter(|s| m.contains_key(s))
                    .collect(),
                None => m.keys().cloned().collect(),
            };
            let len = keys.len();
            for (i, k) in keys.iter().enumerate() {
                write_indent(buf, depth + 1);
                buf.extend_from_slice(serde_json::to_string(k).unwrap().as_bytes());
                buf.extend_from_slice(b": ");
                write_value(buf, &m[k], depth + 1, None);
                if i + 1 < len {
                    buf.push(b',');
                }
                buf.push(b'\n');
            }
            write_indent(buf, depth);
            buf.push(b'}');
        }
        serde_json::Value::Array(a) => {
            if a.is_empty() {
                buf.extend_from_slice(b"[]");
                return;
            }
            buf.push(b'[');
            buf.push(b'\n');
            let len = a.len();
            for (i, item) in a.iter().enumerate() {
                write_indent(buf, depth + 1);
                write_value(buf, item, depth + 1, None);
                if i + 1 < len {
                    buf.push(b',');
                }
                buf.push(b'\n');
            }
            write_indent(buf, depth);
            buf.push(b']');
        }
        other => {
            buf.extend_from_slice(serde_json::to_string(other).unwrap().as_bytes());
        }
    }
}

/// Translate a UTC OffsetDateTime to the archive filename (`:` → `-` for
/// Windows-compat — see spec §4.3.3, AC 27).
pub fn archive_filename(started_at: time::OffsetDateTime) -> String {
    let iso = started_at
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "unknown.json".to_string());
    format!("{}.json", iso.replace(':', "-"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::{Finding, FindingSource, Review, Severity};

    fn fixture_review() -> Review {
        Review {
            session_id: "s_1".into(),
            findings: vec![Finding {
                severity: Severity::High,
                title: "uh oh".into(),
                body: "Confidence: 0.95. Supported by: gpt-4o.".into(),
                source: FindingSource::Divergence,
                supported_by: vec!["gpt-4o".into()],
                confidence: Some(0.95),
            }],
            model_names: vec!["claude-sonnet".into(), "gpt-4o".into()],
            elapsed: Duration::from_secs_f64(12.5),
            project_id: Some("p_1".into()),
            base_url: "https://app.lippa.ai".into(),
            summary_text: Some("Brief.".into()),
            final_agreement_score: Some(0.92),
        }
    }

    fn fixture_inputs() -> ArchiveInputs {
        ArchiveInputs {
            started_at: time::OffsetDateTime::from_unix_timestamp(1_715_350_981).unwrap(),
            elapsed: Duration::from_secs_f64(12.5),
            session_id: "s_1".into(),
            project_id: Some("p_1".into()),
            base_url: "https://app.lippa.ai".into(),
            remote_url: Some("https://github.com/o/r".into()),
            branch: "main".into(),
            head_sha: "abc1234".into(),
            dismissals_applied: 0,
            suppressed_findings: Vec::new(),
        }
    }

    #[test]
    fn buffer_is_deterministic_and_schema_version_first() {
        let r = fixture_review();
        let inp = fixture_inputs();
        let buf1 = build(&r, &inp);
        let buf2 = build(&r, &inp);
        assert_eq!(buf1, buf2);
        // schema_version key appears before any other top-level key.
        let s = String::from_utf8(buf1).unwrap();
        let svp = s.find("\"schema_version\":").unwrap();
        let bup = s.find("\"base_url\":").unwrap();
        assert!(svp < bup, "schema_version must precede base_url");
    }

    #[test]
    fn archive_filename_substitutes_colon() {
        let dt = time::OffsetDateTime::from_unix_timestamp(1_715_350_981).unwrap();
        let name = archive_filename(dt);
        assert!(name.ends_with(".json"));
        assert!(!name.contains(':'));
    }
}
