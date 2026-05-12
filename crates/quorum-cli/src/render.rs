//! Markdown rendering for `Review`. Pure function — same output drives
//! Phase 1A stdout and (Phase 1B) ratatui.

use quorum_core::{Finding, FindingSource, Review, Severity};

/// Render `review` as Phase 1A-shape markdown plus the Phase 1B
/// `findings shown, M dismissed` header suffix when
/// `dismissals_applied > 0`. `M = 0` produces the Phase 1A regression
/// baseline (no suffix), per spec §4.10.1 / AC 53.
pub fn render_review_markdown_with_dismissed(review: &Review, dismissals_applied: u32) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "# Quorum review — session {}\n",
        review.session_id
    ));
    let models = if review.model_names.is_empty() {
        "<unknown>".to_string()
    } else {
        review.model_names.join(", ")
    };
    let dismiss_suffix = if dismissals_applied > 0 {
        // Spec wording: "<N> findings shown, <M> dismissed". N is the
        // post-filter count (what the user sees rendered below).
        format!(
            " · {} findings shown, {} dismissed",
            review.findings.len(),
            dismissals_applied
        )
    } else {
        String::new()
    };
    out.push_str(&format!(
        "Reviewed by {models} in {:.1}s{dismiss_suffix}\n",
        review.elapsed.as_secs_f64()
    ));
    if let Some(score) = review.final_agreement_score {
        out.push_str(&format!("Final agreement score: {score:.2}\n"));
    }
    out.push('\n');

    if let Some(summary) = &review.summary_text {
        out.push_str("## Summary\n");
        out.push_str(summary.trim_end());
        out.push_str("\n\n");
    }

    let groups = [
        (Severity::High, "High severity"),
        (Severity::Medium, "Medium severity"),
        (Severity::Low, "Low severity"),
        (Severity::Info, "Notes"),
    ];

    for (sev, label) in groups {
        let items: Vec<&Finding> = review
            .findings
            .iter()
            .filter(|f| f.severity == sev)
            .collect();
        if items.is_empty() {
            continue;
        }
        out.push_str(&format!("## {label} ({})\n", items.len()));
        for f in items {
            out.push_str(&format!("### {} — {}\n", source_token(f.source), f.title));
            if !f.body.is_empty() {
                out.push_str(&f.body);
                out.push('\n');
            }
            out.push('\n');
        }
    }

    out
}

fn source_token(s: FindingSource) -> &'static str {
    match s {
        FindingSource::Agreement => "Agreement",
        FindingSource::Divergence => "Divergence",
        FindingSource::Assumption => "Assumption",
    }
}

pub const LARGE_FINDING_COUNT_NOTE: &str =
    "note: many findings; pipe to `less` or use --json for programmatic consumption.";

pub fn warn_if_large(review: &Review) -> Option<&'static str> {
    if review.findings.len() > 50 {
        Some(LARGE_FINDING_COUNT_NOTE)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn review_with(findings: Vec<Finding>) -> Review {
        Review {
            session_id: "sess_abc".into(),
            findings,
            model_names: vec!["claude-sonnet".into(), "gpt-4o".into()],
            elapsed: Duration::from_secs_f64(12.5),
            project_id: None,
            base_url: "https://x".into(),
            summary_text: None,
            final_agreement_score: None,
        }
    }

    fn dummy_finding(sev: Severity, title: &str) -> Finding {
        Finding {
            severity: sev,
            title: title.into(),
            body: "body".into(),
            source: FindingSource::Divergence,
            supported_by: vec!["m".into()],
            confidence: Some(0.9),
        }
    }

    #[test]
    fn header_suffix_omitted_when_no_dismissals() {
        // AC 53 regression baseline: M=0 keeps Phase 1A output shape.
        let r = review_with(vec![dummy_finding(Severity::Low, "x")]);
        let md = render_review_markdown_with_dismissed(&r, 0);
        let header = md.lines().nth(1).unwrap();
        assert!(header.contains("Reviewed by"));
        assert!(!header.contains("dismissed"), "no suffix when M = 0");
    }

    #[test]
    fn header_suffix_appended_when_dismissals_positive() {
        // AC 53: "Reviewed by X in Ns · 1 findings shown, 3 dismissed".
        let r = review_with(vec![dummy_finding(Severity::Low, "x")]);
        let md = render_review_markdown_with_dismissed(&r, 3);
        let header = md.lines().nth(1).unwrap();
        assert!(
            header.contains("· 1 findings shown, 3 dismissed"),
            "got: {header}"
        );
    }

    #[test]
    fn header_suffix_uses_post_filter_findings_count() {
        // N is the *post-filter* count — the number of findings the
        // user actually sees rendered below the header.
        let r = review_with(vec![
            dummy_finding(Severity::High, "a"),
            dummy_finding(Severity::Medium, "b"),
        ]);
        let md = render_review_markdown_with_dismissed(&r, 5);
        let header = md.lines().nth(1).unwrap();
        assert!(header.contains("2 findings shown, 5 dismissed"));
    }
}
