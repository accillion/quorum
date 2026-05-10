//! Markdown rendering for `Review`. Pure function — same output drives
//! Phase 1A stdout and (Phase 1B) ratatui.

use quorum_core::{Finding, FindingSource, Review, Severity};

pub fn render_review_markdown(review: &Review) -> String {
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
    out.push_str(&format!(
        "Reviewed by {models} in {:.1}s\n",
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
