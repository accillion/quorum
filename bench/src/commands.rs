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
    let s = st.budget.as_ref().map(|b| b.snapshot());
    println!("run {run_id}: {} outputs; budget {s:?}", st.outputs.len());
    match st.halted {
        Some(h) => Err(format!("run {run_id} halted: {h}")),
        None => Ok(()),
    }
}
