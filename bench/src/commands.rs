//! The paid `run` command and the `results` command. `run` is for 1b: it
//! needs `--confirm-paid`, both caps, the Lippa cookie in
//! `QUORUM_LIPPA_SESSION`, every vendor and judge key, a configured bench
//! project and a passing empty-project preflight.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::BenchConfig;
use crate::corpus::{load_corpus, manifest_text, sha256_hex};
use crate::pipeline::Judge;
use crate::prices::PriceTable;
use crate::runner::{run_all, write_state, Deps, RunState};
use crate::scrub::Scrubber;
use crate::secret::{KeySource, LIPPA_SESSION_ENV};
use crate::templates::Template;
use crate::vendors::{http_client, key_env, Provider};

pub struct RunOpts {
    pub max_credits: u64,
    pub max_usd_micros: u64,
    pub quorum_bin: PathBuf,
    pub case: Option<String>,
    pub confirm_paid: bool,
}

/// The files the design freeze pins, repo-relative.
pub fn frozen_files(cfg: &BenchConfig) -> Vec<String> {
    vec![
        "bench/bench.toml".into(),
        "bench/prices.toml".into(),
        "bench/MANIFEST.toml".into(),
        cfg.s_arms.prompt.clone(),
        cfg.judge.rubric.clone(),
        cfg.extraction.rubric.clone(),
        cfg.clustering.rubric.clone(),
    ]
}

pub fn file_sha(root: &Path, rel: &str) -> Result<String, String> {
    std::fs::read(root.join(rel))
        .map(|b| sha256_hex(&b))
        .map_err(|e| format!("{rel}: {e}"))
}

fn template(root: &Path, rel: &str) -> Result<Template, String> {
    Template::load(&root.join(rel)).map_err(|e| e.to_string())
}

pub async fn cmd_run(root: &Path, keys: &KeySource, o: RunOpts) -> Result<(), String> {
    if !o.confirm_paid {
        return Err("`run` spends Lippa credits and vendor USD: pass --confirm-paid once the budget is approved".into());
    }
    let bench = root.join("bench");
    let cfg = BenchConfig::load(&bench.join("bench.toml")).map_err(|e| e.to_string())?;
    let prices = PriceTable::load(&bench.join("prices.toml")).map_err(|e| e.to_string())?;
    let session = keys
        .get(LIPPA_SESSION_ENV)
        .ok_or("QUORUM_LIPPA_SESSION is not set")?;
    let mut vendors = BTreeMap::new();
    for m in cfg.s_arms.models.values() {
        let env = key_env(&m.vendor).ok_or_else(|| format!("unknown vendor {}", m.vendor))?;
        let k = keys.get(env).ok_or_else(|| format!("{env} is not set"))?;
        vendors.insert(
            m.vendor.clone(),
            Provider::for_vendor(&m.vendor, k).expect("known vendor"),
        );
    }
    let jenv = key_env(&cfg.judge.vendor).ok_or("unknown judge vendor")?;
    let jkey = keys.get(jenv).ok_or_else(|| format!("{jenv} is not set"))?;
    let judge = Judge {
        provider: Provider::for_vendor(&cfg.judge.vendor, jkey).expect("known vendor"),
        model: cfg.judge.model.clone(),
        output_cap: cfg.judge.output_cap,
        effort: cfg.judge.effort.clone(),
        seed: cfg.judge.seed,
    };
    let http = http_client();
    crate::preflight::check_project_empty(
        &http,
        &cfg.lippa.base_url,
        &cfg.lippa.project_id,
        &session,
    )
    .await
    .map_err(|e| e.to_string())?;
    let cases = load_corpus(&bench.join("cases")).map_err(|e| e.to_string())?;
    let manifest = manifest_text(&cases).map_err(|e| e.to_string())?;
    if std::fs::read_to_string(bench.join("MANIFEST.toml")).unwrap_or_default() != manifest {
        return Err("bench/MANIFEST.toml is stale: the corpus changed after the freeze".into());
    }
    let cases: Vec<_> = cases
        .into_iter()
        .filter(|c| o.case.as_deref().map_or(true, |id| c.id() == id))
        .collect();
    let cache = bench.join(".cache");
    let mut mats = Vec::new();
    for c in &cases {
        mats.push((
            c,
            crate::materialize::materialize(c, &cache).map_err(|e| e.to_string())?,
        ));
    }
    let version = std::process::Command::new(&o.quorum_bin)
        .arg("--version")
        .output()
        .map(|x| String::from_utf8_lossy(&x.stdout).trim().to_string())
        .map_err(|e| format!("quorum --version: {e}"))?;
    let run_id = crate::runid::new_run_id(time::OffsetDateTime::now_utc());
    let deps = Deps {
        prices,
        single: template(root, &cfg.s_arms.prompt)?,
        extraction: template(root, &cfg.extraction.rubric)?,
        clustering: template(root, &cfg.clustering.rubric)?,
        rubric: template(root, &cfg.judge.rubric)?,
        vendors,
        judge,
        lippa: cfg.lippa.base_url.clone(),
        session,
        quorum_bin: o.quorum_bin.clone(),
        run_dir: bench.join("runs").join(&run_id),
        scrubber: Scrubber::new(keys.known_values()),
        http,
        cfg,
    };
    let mut st = RunState::new(o.max_credits, o.max_usd_micros);
    st.manifest.run_id = run_id.clone();
    st.manifest.created_at = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    st.manifest.quorum_commit = deps.cfg.quorum.commit.clone();
    st.manifest.quorum_version = version;
    st.manifest.corpus_manifest_sha256 = sha256_hex(manifest.as_bytes());
    st.manifest.judge_model = deps.cfg.judge.model.clone();
    for f in frozen_files(&deps.cfg) {
        st.manifest.frozen.insert(f.clone(), file_sha(root, &f)?);
    }
    run_all(&deps, &mut st, &mats).await;
    write_state(&deps, &st).map_err(|e| e.to_string())?;
    if st.halted.is_none() {
        let n = write_queue(
            &deps.run_dir,
            &cases,
            deps.cfg.adjudication.human_share,
            deps.cfg.run.seed,
        )?;
        println!(
            "{n} item(s) queued for blind adjudication: quorum-bench adjudicate --run {run_id}"
        );
    }
    let s = st.budget.as_ref().map(|b| b.snapshot());
    println!("run {run_id}: {} outputs; budget {s:?}", st.outputs.len());
    match st.halted {
        Some(h) => Err(format!("run {run_id} halted: {h}")),
        None => Ok(()),
    }
}

fn read_json(path: &Path) -> Result<serde_json::Value, String> {
    let t = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&t).map_err(|e| format!("{}: {e}", path.display()))
}

/// Build the blind human queue of a finished run (AC 252, AC 248a,
/// [S1a-ADJ]) from its `state.json` and per-case files.
pub fn write_queue(
    run_dir: &Path,
    cases: &[crate::corpus::Case],
    share: f64,
    seed: u64,
) -> Result<usize, String> {
    use crate::adjudicate::{attach_titles, build_queue, queue_path, titles_by_key, QueueInputs};
    let saved: crate::results::SavedState =
        serde_json::from_value(read_json(&run_dir.join("state.json"))?)
            .map_err(|e| e.to_string())?;
    // Each extraction with the answer it read: a Round 0 seat answer from
    // the Q-high session detail, or an S- response (claims fallback).
    let mut extractions = Vec::new();
    for x in &saved.extractions {
        let dir = run_dir
            .join("cases")
            .join(&x.case_id)
            .join(format!("r{}", x.repeat));
        let answer = if x.seat.starts_with("S-") {
            read_json(&dir.join(format!("{}.json", x.seat)))?["response"]
                .as_str()
                .unwrap_or("")
                .to_string()
        } else {
            let q = read_json(&dir.join("Q-high.json"))?;
            let ex: Vec<crate::proxy::Exchange> =
                serde_json::from_value(q["exchanges"].clone()).map_err(|e| e.to_string())?;
            crate::proxy::session_detail(&ex)
                .map(crate::runner::round0_seats)
                .unwrap_or_default()
                .into_iter()
                .find(|s| s.profile_id == x.seat)
                .map(|s| s.answer)
                .unwrap_or_default()
        };
        extractions.push((x.clone(), answer));
    }
    // Each clustering with its items' titles, rebuilt in the runner's
    // order: the case-repeat's Round 0 extractions, claims in order.
    let mut clusterings = Vec::new();
    for c in &saved.clusterings {
        let titles: Vec<String> = saved
            .extractions
            .iter()
            .filter(|x| x.case_id == c.case_id && x.repeat == c.repeat && !x.seat.starts_with("S-"))
            .flat_map(|x| x.claims.iter().cloned())
            .collect();
        clusterings.push((c.clone(), titles));
    }
    let mut items = build_queue(&QueueInputs {
        cases,
        decisions: &saved.decisions,
        clusterings: &clusterings,
        extractions: &extractions,
        share,
        seed,
    });
    attach_titles(&mut items, &titles_by_key(&saved.outputs));
    let path = queue_path(run_dir);
    std::fs::create_dir_all(path.parent().expect("parent")).map_err(|e| e.to_string())?;
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&items).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(items.len())
}

pub fn cmd_queue(root: &Path, run: &str) -> Result<(), String> {
    let bench = root.join("bench");
    let cfg = BenchConfig::load(&bench.join("bench.toml")).map_err(|e| e.to_string())?;
    let cases = load_corpus(&bench.join("cases")).map_err(|e| e.to_string())?;
    let n = write_queue(
        &bench.join("runs").join(run),
        &cases,
        cfg.adjudication.human_share,
        cfg.run.seed,
    )?;
    println!("{n} item(s) queued for blind adjudication");
    Ok(())
}

pub fn cmd_results(root: &Path, run: &str, draws: usize) -> Result<(), String> {
    let bench = root.join("bench");
    let dir = bench.join("runs").join(run);
    let cfg = BenchConfig::load(&bench.join("bench.toml")).map_err(|e| e.to_string())?;
    let saved: crate::results::SavedState =
        serde_json::from_value(read_json(&dir.join("state.json"))?).map_err(|e| e.to_string())?;
    let human = crate::adjudicate::human_decisions(&dir).map_err(|e| e.to_string())?;
    let cases = load_corpus(&bench.join("cases")).map_err(|e| e.to_string())?;
    let r = crate::results::compute(&cases, &saved, &human, draws, cfg.run.seed)
        .map_err(|e| e.join("\n"))?;
    let text = serde_json::to_string_pretty(&r).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("results.json"), &text).map_err(|e| e.to_string())?;
    println!("{text}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::fake_lippa::{FakeLippa, FakeSession};
    use crate::materialize::testrepo::*;
    use crate::runner::tests::{deps, detail};

    /// Run, queue, adjudicate blind, then compute results: the whole 1b
    /// path at zero cost.
    #[tokio::test(flavor = "multi_thread")]
    async fn run_queue_adjudicate_results() {
        let up = upstream();
        let td = tempfile::TempDir::new().unwrap();
        let case = case_for(&td.path().join("cases"), &up, "rs-x-01");
        let m =
            crate::materialize::materialize_from(&up.repo, &case, &td.path().join("repos/rs-x-01"))
                .unwrap();
        let fake = FakeLippa::start(FakeSession {
            detail: detail(None),
        })
        .await
        .unwrap();
        let d = deps(
            td.path(),
            &fake.base_url(),
            Arc::new(Mutex::new(Vec::new())),
        )
        .await;
        let mut st = RunState::new(100, 100_000_000);
        run_all(&d, &mut st, &[(&case, m)]).await;
        write_state(&d, &st).unwrap();
        let cases = vec![case];
        // Share 1.0: every item is checked, so every kind appears.
        let n = write_queue(&d.run_dir, &cases, 1.0, 3).unwrap();
        let q = crate::adjudicate::load_queue(&d.run_dir).unwrap();
        assert_eq!(n, q.len());
        assert!(q.iter().any(|i| matches!(i, crate::adjudicate::Item::Extraction { answer, .. } if answer.contains("subtracts"))));
        assert!(q.iter().any(|i| matches!(i, crate::adjudicate::Item::Clustering { groups, .. } if !groups.is_empty())));
        assert!(q.iter().all(|i| match i {
            crate::adjudicate::Item::Finding { finding, .. } => !finding.title.is_empty(),
            _ => true,
        }));
        let mut script = String::new();
        for it in &q {
            script.push_str(if matches!(it, crate::adjudicate::Item::Finding { .. }) {
                "m\n"
            } else {
                "y\n"
            });
        }
        let left = crate::adjudicate::run_session(
            &d.run_dir,
            &mut std::io::Cursor::new(script),
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(left, 0);
        let saved: crate::results::SavedState =
            serde_json::from_value(read_json(&d.run_dir.join("state.json")).unwrap()).unwrap();
        let human = crate::adjudicate::human_decisions(&d.run_dir).unwrap();
        let r = crate::results::compute(&cases, &saved, &human, 200, 1).unwrap();
        assert_eq!(r.label, crate::results::PILOT_LABEL);
        // One case, one repository: the bootstrap cannot run.
        assert!(matches!(r.c1.label, crate::stats::Label::NotTested(_)));
        assert_eq!(r.descriptive["Q-high"].recall_all, Some(1.0));
    }
}
