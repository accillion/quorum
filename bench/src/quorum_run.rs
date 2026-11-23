//! Drive the installed `quorum` binary on a materialized case (AC 242a):
//! `quorum review --json --range <case-base>..<case-commit> --tier <t>`.
//!
//! The harness never links Quorum's library: it runs the binary built from
//! a pinned commit, so it measures what users get. Results are read from
//! the archive Quorum writes under `.quorum/reviews/`; `--json` stdout is
//! not pure JSON at 0.5.x (BACKLOG, fixed in v0.6 Stage 5), so stdout is
//! kept only as a raw record.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::proxy::Exchange;
use crate::secret::Secret;

pub struct ReviewCall<'a> {
    pub quorum_bin: &'a Path,
    pub repo_dir: &'a Path,
    pub range: &'a str,
    pub tier: &'a str,
    /// The recording proxy's base URL (`http://127.0.0.1:<port>`).
    pub base_url: &'a str,
    pub project_id: &'a str,
    /// The Lippa cookie, passed to the child as `QUORUM_LIPPA_SESSION`
    /// with the matching pin `QUORUM_LIPPA_BASE_URL` (SERVICES §4.2).
    pub session: &'a Secret,
    pub timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct ReviewOutcome {
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
    pub archive: Option<Value>,
    pub archive_file: Option<String>,
    pub elapsed_ms: u64,
}

/// How a review counts (AC 242a, AC 250a).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Exit 0 and an archive: score its findings.
    Completed,
    /// Anything else: after its one retry, zero findings for the case and
    /// repeat (AC 250a).
    Error(String),
    /// A spend refusal: the run halts and the review is never scored.
    Halt(String),
}

/// Write the case repo's `.quorum/config.toml`: the proxy as `base_url`,
/// the bench project, and no remote URL in the bundle.
pub fn write_case_config(repo_dir: &Path, project_id: &str, base_url: &str) -> std::io::Result<()> {
    let qd = repo_dir.join(".quorum");
    std::fs::create_dir_all(&qd)?;
    std::fs::write(
        qd.join("config.toml"),
        format!(
            "# Written by quorum-bench for one review; not part of the case.\nproject_id = \"{project_id}\"\nbase_url = \"{base_url}\"\nremote_url = false\n"
        ),
    )
}

fn archives(repo_dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(repo_dir.join(".quorum").join("reviews"))
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

pub async fn run_review(c: &ReviewCall<'_>) -> std::io::Result<ReviewOutcome> {
    let before = archives(c.repo_dir);
    write_case_config(c.repo_dir, c.project_id, c.base_url)?;
    let started = Instant::now();
    let mut cmd = tokio::process::Command::new(c.quorum_bin);
    cmd.current_dir(c.repo_dir)
        .args(["review", "--json", "--range", c.range, "--tier", c.tier])
        .env("QUORUM_LIPPA_SESSION", c.session.expose())
        .env("QUORUM_LIPPA_BASE_URL", c.base_url)
        .env_remove("QUORUM_LIPPA_EMAIL")
        .env_remove("QUORUM_LIPPA_PASSWORD")
        .env_remove("RUST_LOG")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = cmd.spawn()?;
    let (exit_code, timed_out, stdout, stderr) =
        match tokio::time::timeout(c.timeout, child.wait_with_output()).await {
            Ok(out) => {
                let out = out?;
                (
                    out.status.code(),
                    false,
                    String::from_utf8_lossy(&out.stdout).into_owned(),
                    String::from_utf8_lossy(&out.stderr).into_owned(),
                )
            }
            Err(_) => (None, true, String::new(), String::new()),
        };
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let new: Vec<PathBuf> = archives(c.repo_dir)
        .into_iter()
        .filter(|p| !before.contains(p))
        .collect();
    let (archive, archive_file) = match new.as_slice() {
        [one] => {
            let text = std::fs::read_to_string(one)?;
            (
                serde_json::from_str(&text).ok(),
                one.file_name().map(|n| n.to_string_lossy().into_owned()),
            )
        }
        _ => (None, None),
    };
    Ok(ReviewOutcome {
        exit_code,
        timed_out,
        stdout,
        stderr,
        archive,
        archive_file,
        elapsed_ms,
    })
}

/// Classify a finished review using its outcome and the proxy recording.
/// A 402 on create (Lippa refused for credits) or a Quorum spend-guard
/// refusal halts the run (AC 242a). Quorum 0.5.x has no spend guard yet
/// (v0.6 Stage 6); when it lands, its refusal line must be added here.
pub fn classify(o: &ReviewOutcome, exchanges: &[Exchange]) -> Verdict {
    if crate::proxy::create_status(exchanges) == Some(402) {
        return Verdict::Halt("Lippa answered 402 (credits) on session create".into());
    }
    if o.stderr
        .lines()
        .any(|l| l.trim_start().starts_with("quorum: budget"))
    {
        return Verdict::Halt("Quorum's spend guard refused the review".into());
    }
    if o.timed_out {
        return Verdict::Error("timed out".into());
    }
    match (o.exit_code, &o.archive) {
        (Some(0), Some(_)) => Verdict::Completed,
        (Some(0), None) => Verdict::Error("exit 0 but no archive".into()),
        (code, _) => Verdict::Error(format!("exit {code:?}")),
    }
}

/// The `quorum` binary next to this test binary's target dir, for tests.
#[cfg(test)]
pub(crate) fn workspace_quorum_bin() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"));
    let exe = if cfg!(windows) {
        "quorum.exe"
    } else {
        "quorum"
    };
    let p = target.join("debug").join(exe);
    assert!(
        p.is_file(),
        "{} is missing: build it with `cargo build -p quorum-cli` (cargo test --workspace builds it)",
        p.display()
    );
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_lippa::{FakeLippa, FakeSession};
    use crate::materialize::testrepo::*;
    use crate::proxy::{sent_bundle, RecordingProxy};

    /// The real `quorum` binary, through the recording proxy, against the
    /// local fake Lippa: the harness path runs end to end at zero cost and
    /// the proxy captures the exact bundle.
    #[tokio::test(flavor = "multi_thread")]
    async fn drives_the_real_binary_through_the_proxy() {
        let up = upstream();
        let td = tempfile::TempDir::new().unwrap();
        let case = case_for(&td.path().join("cases"), &up, "rs-x-01");
        let m =
            crate::materialize::materialize_from(&up.repo, &case, &td.path().join("repos/rs-x-01"))
                .unwrap();
        let mut detail = FakeSession::default();
        detail.detail["agreement"] = serde_json::json!([{"claim_text": "add subtracts", "confidence": 0.9, "supported_by": ["gpt-6.2-sol", "claude-opus-5-5"]}]);
        let fake = FakeLippa::start(detail).await.unwrap();
        let proxy = RecordingProxy::start(&fake.base_url()).await.unwrap();
        let cookie = Secret::new(format!("session-canary-{}", "7".repeat(20)));
        let bin = workspace_quorum_bin();
        let base = proxy.base_url();
        let range = m.range();
        let call = ReviewCall {
            quorum_bin: &bin,
            repo_dir: &m.dir,
            range: &range,
            tier: "high",
            base_url: &base,
            project_id: "bench-project",
            session: &cookie,
            timeout: Duration::from_secs(120),
        };
        let o = run_review(&call).await.unwrap();
        let ex = proxy.take();
        assert_eq!(
            classify(&o, &ex),
            Verdict::Completed,
            "stderr: {}",
            o.stderr
        );
        let archive = o.archive.as_ref().unwrap();
        assert_eq!(archive["tier"], "high");
        assert_eq!(archive["findings"][0]["title"], "add subtracts");
        assert!(
            archive["repo"].get("remote_url").is_none(),
            "remote_url = false keeps the source repo out"
        );
        let bundle = sent_bundle(&ex).expect("bundle recorded");
        assert!(bundle.contains("a - b"), "the case diff is in the bundle");
        assert!(
            bundle.contains("rules"),
            "AGENTS.md at the base reaches the bundle"
        );
        assert!(
            !bundle.contains("adds_two_numbers_correctly"),
            "the removed test never does"
        );
        let serialized = serde_json::to_string(&ex).unwrap();
        assert!(
            !serialized.contains("session-canary"),
            "the cookie is not in the recording"
        );
    }

    #[test]
    fn classification() {
        let base = ReviewOutcome {
            exit_code: Some(0),
            timed_out: false,
            stdout: String::new(),
            stderr: String::new(),
            archive: Some(serde_json::json!({})),
            archive_file: None,
            elapsed_ms: 1,
        };
        assert_eq!(classify(&base, &[]), Verdict::Completed);
        let mut e = base.clone();
        e.exit_code = Some(1);
        assert!(matches!(classify(&e, &[]), Verdict::Error(_)));
        let mut t = base.clone();
        t.timed_out = true;
        assert!(matches!(classify(&t, &[]), Verdict::Error(_)));
        let ex = vec![Exchange {
            method: "POST".into(),
            path: "/api/v1/consensus/sessions".into(),
            status: 402,
            request: crate::proxy::RecordedBody::Empty,
            response: crate::proxy::RecordedBody::Empty,
            elapsed_ms: 0,
        }];
        assert!(matches!(classify(&e, &ex), Verdict::Halt(_)));
    }
}
