//! Corpus verification at zero cost: validate each case, materialize it,
//! run the leakage checks (AC 244) and, with a `quorum` binary, a dry run
//! against the local fake Lippa that captures the real bundle and checks
//! it fits the default budget with no cut (AC 243) and holds no repro test
//! line (AC 244).

use std::path::Path;
use std::time::Duration;

use serde::Serialize;

use crate::corpus::{diff_stats, Case};
use crate::fake_lippa::{FakeLippa, FakeSession};
use crate::materialize::{
    bundle_leaks, case_tree_lines, leakage_problems, materialize, repro_files, Materialized,
};
use crate::proxy::{sent_bundle, RecordingProxy};
use crate::quorum_run::{classify, run_review, ReviewCall, Verdict};
use crate::secret::Secret;

#[derive(Debug, Clone, Default, Serialize)]
pub struct CaseReport {
    pub id: String,
    pub problems: Vec<String>,
    pub bundle_bytes: Option<usize>,
    pub bundle_sha256: Option<String>,
    /// Unchanged context files the budget left out (allowed; reported).
    pub context_files_omitted: usize,
}

/// Cuts that would change what the reviewer sees of the change itself.
pub fn bundle_cut_problems(
    bundle: &str,
    changed: &std::collections::BTreeSet<String>,
) -> (Vec<String>, usize) {
    let mut problems = Vec::new();
    let mut context_omitted = 0;
    if bundle.contains("[diff truncated:") {
        problems.push("the diff section was truncated".into());
    }
    for line in bundle.lines() {
        if let Some(rest) = line.strip_prefix("[file omitted: ") {
            let path = rest.split(',').next().unwrap_or("");
            if changed.contains(path) {
                problems.push(format!("changed file `{path}` omitted for budget"));
            } else {
                context_omitted += 1;
            }
        }
        if let Some(rest) = line.strip_prefix("[excluded by deny-list: ") {
            let path = rest.split(',').next().unwrap_or("");
            if changed.contains(path) {
                problems.push(format!("changed file `{path}` withheld by the deny list"));
            }
        }
    }
    (problems, context_omitted)
}

pub async fn dry_run(
    case: &Case,
    m: &Materialized,
    quorum_bin: &Path,
    report: &mut CaseReport,
) -> std::io::Result<()> {
    let fake = FakeLippa::start(FakeSession::default()).await?;
    let proxy = RecordingProxy::start(&fake.base_url())
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let base = proxy.base_url();
    let range = m.range();
    let session = Secret::new("dry-run-session-not-a-cookie");
    let call = ReviewCall {
        quorum_bin,
        repo_dir: &m.dir,
        range: &range,
        tier: "high",
        base_url: &base,
        project_id: "dry-run",
        session: &session,
        timeout: Duration::from_secs(300),
    };
    let o = run_review(&call).await?;
    let ex = proxy.take();
    if classify(&o, &ex) != Verdict::Completed {
        report.problems.push(format!(
            "dry run did not complete: exit {:?}; stderr: {}",
            o.exit_code,
            o.stderr.trim()
        ));
        return Ok(());
    }
    let Some(bundle) = sent_bundle(&ex) else {
        report.problems.push("dry run sent no bundle".into());
        return Ok(());
    };
    report.bundle_bytes = Some(bundle.len());
    report.bundle_sha256 = Some(crate::corpus::sha256_hex(bundle.as_bytes()));
    let (paths, _) = diff_stats(&String::from_utf8_lossy(&case.diff));
    let (cuts, ctx) = bundle_cut_problems(bundle, &paths);
    report.problems.extend(cuts);
    report.context_files_omitted = ctx;
    let lines = case_tree_lines(m).map_err(|e| std::io::Error::other(e.to_string()))?;
    let repro = repro_files(case)?;
    report.problems.extend(bundle_leaks(bundle, &repro, &lines));
    Ok(())
}

/// Validate, materialize and check one case. Problems are collected, not
/// raised, so one run reports every case.
pub async fn verify_case(case: &Case, cache: &Path, quorum_bin: Option<&Path>) -> CaseReport {
    let mut r = CaseReport {
        id: case.id().into(),
        ..Default::default()
    };
    let m = match materialize(case, cache).await {
        Ok(m) => m,
        Err(e) => {
            r.problems.push(format!("materialize: {e}"));
            return r;
        }
    };
    match leakage_problems(case, &m) {
        Ok(p) => r.problems.extend(p),
        Err(e) => r.problems.push(format!("leakage check: {e}")),
    }
    if let Some(bin) = quorum_bin {
        if let Err(e) = dry_run(case, &m, bin, &mut r).await {
            r.problems.push(format!("dry run: {e}"));
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cut_detection() {
        let changed: std::collections::BTreeSet<String> = ["src/a.rs".to_string()].into();
        let b = "x\n[file omitted: src/a.rs, 10 bytes; budget exhausted]\n[file omitted: src/ctx.rs, 9 bytes; budget exhausted]\n";
        let (p, ctx) = bundle_cut_problems(b, &changed);
        assert_eq!(p.len(), 1);
        assert_eq!(ctx, 1);
        let (p, _) = bundle_cut_problems(
            "[diff truncated: 5 bytes exceeded 100KB; review only top portion]",
            &changed,
        );
        assert_eq!(p.len(), 1);
        let (p, _) = bundle_cut_problems("[excluded by deny-list: src/a.rs, pattern=x]", &changed);
        assert_eq!(p.len(), 1);
        assert!(bundle_cut_problems("clean", &changed).0.is_empty());
    }

    /// The whole zero-cost path on the test case: materialize from a local
    /// upstream, leakage checks, then the real binary's bundle.
    #[tokio::test(flavor = "multi_thread")]
    async fn dry_run_on_a_valid_case_reports_no_problem() {
        use crate::materialize::testrepo::*;
        let up = upstream();
        let td = tempfile::TempDir::new().unwrap();
        let case = case_for(&td.path().join("cases"), &up, "rs-x-01");
        let m =
            crate::materialize::materialize_from(&up.repo, &case, &td.path().join("repos/rs-x-01"))
                .unwrap();
        let mut r = CaseReport::default();
        dry_run(
            &case,
            &m,
            &crate::quorum_run::workspace_quorum_bin(),
            &mut r,
        )
        .await
        .unwrap();
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        assert!(r.bundle_bytes.unwrap() > 0);
    }
}
