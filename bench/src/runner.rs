//! The run (1b): every case, repeat and arm, under the budget, then
//! judging. In 1a it runs only against the fake Lippa and mock models.
//!
//! Per case and repeat:
//! 1. `Q-<tier>` for each tier: the real `quorum` binary through the
//!    recording proxy. The first `Q-high` session of the run fixes the
//!    panel (AC 248); a `Q-high` session reporting another panel makes the
//!    case-repeat **uncontrolled** (excluded from C1 and C2, reported).
//! 2. `S-<model>` for each panel model with a vendor mapping: the exact
//!    bundle bytes of this repeat's `Q-high` session, wrapped in
//!    `bench/prompts/single-v1.md`, at the seat's settings (Gate A ruling
//!    4). Claims are parsed; an answer without a claims block falls back to
//!    the extraction rubric.
//! 3. `E-union` / `E-majority` from this `Q-high` session's own Round 0
//!    answers ([S1a-ADJ] to AC 248a): extraction per seat, then
//!    clustering. Fewer than three healthy Round 0 seats excludes the
//!    case-repeat from C2.
//!
//! Then the judge classifies every unique stripped finding per case.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use crate::budget::{Budget, Settle, Unit};
use crate::calls::{paid_call, CallLog, StepError};
use crate::claims::{cap_claims, cap_words, parse_claims};
use crate::config::BenchConfig;
use crate::corpus::{sha256_hex, Case};
use crate::findings::{finding_key, from_archive, strip, Arm, RawFinding, StrippedFinding};
use crate::manifest::{CallRecord, RunManifest};
use crate::materialize::Materialized;
use crate::pipeline::{
    cluster, ensembles, extract, judge_case, ClusterItem, Clustering, Ctx, Decision, Extraction,
    Judge,
};
use crate::prices::PriceTable;
use crate::proxy::{sent_bundle, session_detail, Exchange, RecordingProxy};
use crate::quorum_run::{classify, run_review, ReviewCall, Verdict};
use crate::score::{ArmOutput, Status};
use crate::scrub::Scrubber;
use crate::secret::Secret;
use crate::templates::Template;
use crate::vendors::{ModelRequest, Provider};

pub struct Deps {
    pub cfg: BenchConfig,
    pub prices: PriceTable,
    pub single: Template,
    pub extraction: Template,
    pub clustering: Template,
    pub rubric: Template,
    /// Provider per vendor name for the `S-` arms.
    pub vendors: BTreeMap<String, Provider>,
    pub judge: Judge,
    /// Lippa's base URL (the proxy's upstream).
    pub lippa: String,
    pub session: Secret,
    pub quorum_bin: PathBuf,
    pub run_dir: PathBuf,
    pub scrubber: Scrubber,
    pub http: reqwest::Client,
}

/// One Round 0 seat of a session.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Seat {
    pub profile_id: String,
    /// `answered_model_id` when a fallback answered, else the profile's model.
    pub observed_model: String,
    pub answer: String,
    pub healthy: bool,
}

/// Round 0 seats from a session detail, in profile order.
pub fn round0_seats(detail: &Value) -> Vec<Seat> {
    let mut v: Vec<Seat> = detail["rounds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["round_index"].as_u64() == Some(0))
        .map(|r| {
            let profile = r["model_profile_id"].as_str().unwrap_or("").to_string();
            let observed = r["answered_model_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| profile.clone());
            let answer = r["raw_response"].as_str().unwrap_or("").to_string();
            let errored = r["error_text"]
                .as_str()
                .is_some_and(|e| !e.trim().is_empty());
            Seat {
                healthy: !errored && !answer.trim().is_empty(),
                profile_id: profile,
                observed_model: observed,
                answer,
            }
        })
        .collect();
    v.sort_by(|a, b| a.profile_id.cmp(&b.profile_id));
    v
}

pub fn panel_of(seats: &[Seat]) -> BTreeMap<String, String> {
    seats
        .iter()
        .map(|s| (s.profile_id.clone(), s.observed_model.clone()))
        .collect()
}

#[derive(Default, Serialize)]
pub struct RunState {
    pub manifest: RunManifest,
    pub outputs: Vec<ArmOutput>,
    pub extractions: Vec<Extraction>,
    pub clusterings: Vec<Clustering>,
    #[serde(skip)]
    pub cluster_items: BTreeMap<(String, u32), Vec<ClusterItem>>,
    pub calls: Vec<CallLog>,
    pub uncontrolled: Vec<(String, u32)>,
    pub c2_excluded: Vec<(String, u32, String)>,
    pub decisions: Vec<Decision>,
    pub halted: Option<String>,
    #[serde(skip)]
    pub budget: Option<Budget>,
}

impl RunState {
    pub fn new(max_credits: u64, max_usd_micros: u64) -> Self {
        Self {
            budget: Some(Budget::new(max_credits, max_usd_micros)),
            ..Default::default()
        }
    }
    fn budget(&mut self) -> &mut Budget {
        self.budget.as_mut().expect("budget")
    }
}

fn write_json(path: &Path, v: &Value, s: &Scrubber) -> std::io::Result<()> {
    std::fs::create_dir_all(path.parent().expect("parent"))?;
    std::fs::write(
        path,
        serde_json::to_string_pretty(&s.scrub_json(v)).unwrap_or_default(),
    )
}

/// Credits a Q- review spent, from what the proxy saw.
fn credit_settle(exchanges: &[Exchange]) -> Settle {
    match crate::proxy::create_status(exchanges) {
        None => Settle::NeverSent,
        Some(s) if s >= 400 => Settle::Billed(0),
        Some(_) => match session_detail(exchanges) {
            Some(d)
                if d["billing_state"]
                    .as_str()
                    .is_some_and(|b| b.contains("refund")) =>
            {
                Settle::Billed(0)
            }
            Some(d) => d["credits_consumed"]
                .as_u64()
                .map(Settle::Billed)
                .unwrap_or(Settle::Uncertain),
            None => Settle::Uncertain,
        },
    }
}

pub struct QResult {
    pub verdict: Verdict,
    pub archive: Option<Value>,
    pub exchanges: Vec<Exchange>,
    pub latency_ms: u64,
    pub attempts: u32,
}

/// One Q- review, with its one retry (AC 250a), under the credit cap.
async fn run_q(
    deps: &Deps,
    st: &mut RunState,
    m: &Materialized,
    tier: &str,
) -> Result<QResult, String> {
    let mut attempts = 0;
    loop {
        let r = st
            .budget()
            .reserve(Unit::Credits, deps.cfg.lippa.credits_per_session)
            .map_err(|e| e.to_string())?;
        attempts += 1;
        let proxy = RecordingProxy::start(&deps.lippa)
            .await
            .map_err(|e| e.to_string())?;
        let base = proxy.base_url();
        let range = m.range();
        let call = ReviewCall {
            quorum_bin: &deps.quorum_bin,
            repo_dir: &m.dir,
            range: &range,
            tier,
            base_url: &base,
            project_id: &deps.cfg.lippa.project_id,
            session: &deps.session,
            timeout: Duration::from_secs(deps.cfg.run.review_timeout_seconds),
        };
        let o = run_review(&call).await.map_err(|e| e.to_string())?;
        let ex = proxy.take();
        st.budget()
            .settle(r, credit_settle(&ex))
            .map_err(|e| e.to_string())?;
        let verdict = classify(&o, &ex);
        if matches!(verdict, Verdict::Error(_)) && attempts < 2 {
            continue;
        }
        return Ok(QResult {
            verdict,
            archive: o.archive,
            exchanges: ex,
            latency_ms: o.elapsed_ms,
            attempts,
        });
    }
}

/// Run every arm for one case and repeat. `Err` halts the whole run.
pub async fn run_case_repeat(
    deps: &Deps,
    st: &mut RunState,
    case: &Case,
    m: &Materialized,
    repeat: u32,
) -> Result<(), String> {
    let cid = case.id().to_string();
    let dir = deps
        .run_dir
        .join("cases")
        .join(&cid)
        .join(format!("r{repeat}"));
    let mut high: Option<(String, Value)> = None;
    for tier in &deps.cfg.run.tiers {
        let q = run_q(deps, st, m, tier).await?;
        let arm = Arm::Q(tier.clone());
        let bundle = sent_bundle(&q.exchanges).unwrap_or("").to_string();
        let detail = session_detail(&q.exchanges).cloned();
        write_json(
            &dir.join(format!("{arm}.json")),
            &json!({"verdict": format!("{:?}", q.verdict), "archive": q.archive, "exchanges": q.exchanges}),
            &deps.scrubber,
        )
        .map_err(|e| e.to_string())?;
        let seats = detail.as_ref().map(round0_seats).unwrap_or_default();
        st.manifest.records.push(CallRecord {
            case_id: cid.clone(),
            arm: arm.to_string(),
            repeat,
            bundle_sha256: sha256_hex(bundle.as_bytes()),
            prompt_sha256: sha256_hex(bundle.as_bytes()),
            tier: Some(tier.clone()),
            session_id: detail
                .as_ref()
                .and_then(|d| d["id"].as_str().map(str::to_string)),
            observed_models: panel_of(&seats),
            latency_ms: q.latency_ms,
            credits: detail
                .as_ref()
                .and_then(|d| d["credits_consumed"].as_u64())
                .unwrap_or(0),
            outcome: format!("{:?}", q.verdict),
            attempts: q.attempts,
            ..Default::default()
        });
        let status = match &q.verdict {
            Verdict::Halt(why) => return Err(format!("{cid} r{repeat} {arm}: {why}")),
            Verdict::Completed => Status::Completed,
            Verdict::Error(e) => Status::Error(e.clone()),
        };
        let findings = q
            .archive
            .as_ref()
            .map(|a| from_archive(&arm, &cid, repeat, a))
            .unwrap_or_default();
        st.outputs.push(ArmOutput {
            arm,
            case_id: cid.clone(),
            repeat,
            status,
            findings,
        });
        if tier == "high" {
            if let (Verdict::Completed, Some(d)) = (&q.verdict, detail) {
                high = Some((bundle, d));
            }
        }
    }
    let Some((bundle, detail)) = high else {
        st.c2_excluded
            .push((cid.clone(), repeat, "Q-high did not complete".into()));
        return Ok(());
    };
    let seats = round0_seats(&detail);
    let panel = panel_of(&seats);
    if st.manifest.panel.is_empty() {
        st.manifest.panel = panel.clone();
    } else if st.manifest.panel != panel {
        st.uncontrolled.push((cid.clone(), repeat));
    }

    // S- arms: the run panel's models that have a vendor mapping.
    let run_models: Vec<String> = st.manifest.panel.values().cloned().collect();
    for lippa_id in run_models {
        let Some(sm) = deps.cfg.s_arms.models.get(&lippa_id) else {
            continue;
        };
        let Some(provider) = deps.vendors.get(&sm.vendor) else {
            continue;
        };
        let user = deps
            .single
            .render_user(&BTreeMap::from([("BUNDLE", bundle.as_str())]))
            .map_err(|e| e.to_string())?;
        let req = ModelRequest {
            model: sm.vendor_model.clone(),
            system: deps.single.system.clone(),
            user,
            max_output_tokens: deps.cfg.s_arms.output_cap,
            effort: Some(sm.effort.clone()),
            json_schema: None,
            seed: None,
        };
        let arm = Arm::S(lippa_id.clone());
        st.manifest.direct_call_settings.entry(arm.to_string()).or_insert_with(|| {
            json!({"vendor": sm.vendor, "vendor_model": sm.vendor_model, "effort": sm.effort, "output_cap": deps.cfg.s_arms.output_cap, "prompt": format!("single-v{}", deps.single.version)})
        });
        let prompt_sha = sha256_hex(format!("{}\u{0}{}", req.system, req.user).as_bytes());
        let budget = st.budget.as_mut().expect("budget");
        let resp = paid_call(
            budget,
            &deps.prices,
            provider,
            &deps.http,
            &req,
            &format!("{arm} {cid} r{repeat}"),
            &mut st.calls,
        )
        .await;
        let (status, titles, text) = match resp {
            Err(e) if e.is_budget_stop() => return Err(e.to_string()),
            Err(e) => (Status::Error(e.to_string()), Vec::new(), String::new()),
            Ok(r) => match parse_claims(&r.text) {
                Some(c) => (
                    Status::Completed,
                    cap_claims(c)
                        .into_iter()
                        .map(|c| cap_words(&c.text))
                        .collect(),
                    r.text,
                ),
                None => {
                    let mut ctx = Ctx {
                        budget: st.budget.as_mut().expect("budget"),
                        prices: &deps.prices,
                        http: &deps.http,
                        log: &mut st.calls,
                    };
                    match extract(
                        &mut ctx,
                        &deps.judge,
                        &deps.extraction,
                        &cid,
                        repeat,
                        &arm.to_string(),
                        &r.text,
                    )
                    .await
                    {
                        Ok(x) => {
                            let t = x.claims.clone();
                            st.extractions.push(x);
                            (Status::Completed, t, r.text)
                        }
                        Err(e) if e.is_budget_stop() => return Err(e.to_string()),
                        Err(e) => (
                            Status::Error(format!("claims fallback: {e}")),
                            Vec::new(),
                            r.text,
                        ),
                    }
                }
            },
        };
        write_json(
            &dir.join(format!("{arm}.json")),
            &json!({"response": text, "findings": titles}),
            &deps.scrubber,
        )
        .map_err(|e| e.to_string())?;
        let findings = titles
            .iter()
            .enumerate()
            .map(|(i, t)| RawFinding {
                arm: arm.clone(),
                case_id: cid.clone(),
                repeat,
                seat: Some(lippa_id.clone()),
                ordinal: i,
                title: t.clone(),
            })
            .collect();
        st.manifest.records.push(CallRecord {
            case_id: cid.clone(),
            arm: arm.to_string(),
            repeat,
            bundle_sha256: sha256_hex(bundle.as_bytes()),
            prompt_sha256: prompt_sha,
            model_returned: st.calls.last().and_then(|c| c.model_returned.clone()),
            latency_ms: st.calls.last().map(|c| c.latency_ms).unwrap_or(0),
            usd_micros: st.calls.last().map(|c| c.charged_micros).unwrap_or(0),
            outcome: format!("{status:?}"),
            ..Default::default()
        });
        st.outputs.push(ArmOutput {
            arm,
            case_id: cid.clone(),
            repeat,
            status,
            findings,
        });
    }

    // E- arms from this session's Round 0 ([S1a-ADJ]).
    let healthy: Vec<&Seat> = seats.iter().filter(|s| s.healthy).collect();
    if seats.len() < 3 || healthy.len() < 3 {
        st.c2_excluded.push((
            cid.clone(),
            repeat,
            format!("{} healthy Round 0 seats of {}", healthy.len(), seats.len()),
        ));
        return Ok(());
    }
    let mut items = Vec::new();
    for s in &healthy {
        let mut ctx = Ctx {
            budget: st.budget.as_mut().expect("budget"),
            prices: &deps.prices,
            http: &deps.http,
            log: &mut st.calls,
        };
        match extract(
            &mut ctx,
            &deps.judge,
            &deps.extraction,
            &cid,
            repeat,
            &s.profile_id,
            &s.answer,
        )
        .await
        {
            Ok(x) => {
                for (i, c) in x.claims.iter().enumerate() {
                    items.push(ClusterItem {
                        seat: s.profile_id.clone(),
                        ordinal: i,
                        title: c.clone(),
                    });
                }
                st.extractions.push(x);
            }
            Err(e) if e.is_budget_stop() => return Err(e.to_string()),
            Err(e) => {
                st.c2_excluded.push((
                    cid.clone(),
                    repeat,
                    format!("extraction failed for a seat: {e}"),
                ));
                return Ok(());
            }
        }
    }
    let (union, majority) = if items.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        let diff = String::from_utf8_lossy(&case.diff).into_owned();
        let mut ctx = Ctx {
            budget: st.budget.as_mut().expect("budget"),
            prices: &deps.prices,
            http: &deps.http,
            log: &mut st.calls,
        };
        match cluster(
            &mut ctx,
            &deps.judge,
            &deps.clustering,
            &cid,
            repeat,
            &diff,
            &items,
            deps.cfg.run.seed,
        )
        .await
        {
            Ok(c) => {
                let e = ensembles(&items, &c);
                st.clusterings.push(c);
                e
            }
            Err(e) if e.is_budget_stop() => return Err(e.to_string()),
            Err(e) => {
                st.c2_excluded
                    .push((cid.clone(), repeat, format!("clustering failed: {e}")));
                return Ok(());
            }
        }
    };
    st.cluster_items.insert((cid.clone(), repeat), items);
    for (arm, titles) in [(Arm::EUnion, union), (Arm::EMajority, majority)] {
        let findings = titles
            .iter()
            .enumerate()
            .map(|(i, t)| RawFinding {
                arm: arm.clone(),
                case_id: cid.clone(),
                repeat,
                seat: None,
                ordinal: i,
                title: t.clone(),
            })
            .collect();
        st.outputs.push(ArmOutput {
            arm,
            case_id: cid.clone(),
            repeat,
            status: Status::Completed,
            findings,
        });
    }
    Ok(())
}

/// The unique stripped findings of one case across every arm and repeat.
pub fn unique_findings(outputs: &[ArmOutput], case_id: &str) -> Vec<(String, StrippedFinding)> {
    let mut m: BTreeMap<String, StrippedFinding> = BTreeMap::new();
    for o in outputs.iter().filter(|o| o.case_id == case_id) {
        for f in &o.findings {
            let s = strip(&f.title);
            m.entry(finding_key(case_id, &s)).or_insert(s);
        }
    }
    m.into_iter().collect()
}

/// The judge phase: every case's unique findings.
pub async fn judge_all(deps: &Deps, st: &mut RunState, cases: &[&Case]) -> Result<(), StepError> {
    for case in cases {
        let f = unique_findings(&st.outputs, case.id());
        if f.is_empty() {
            continue;
        }
        let mut ctx = Ctx {
            budget: st.budget.as_mut().expect("budget"),
            prices: &deps.prices,
            http: &deps.http,
            log: &mut st.calls,
        };
        let ds = judge_case(
            &mut ctx,
            &deps.judge,
            &deps.rubric,
            case,
            &f,
            deps.cfg.judge.batch_size,
            deps.cfg.run.seed,
        )
        .await?;
        st.decisions.extend(ds);
    }
    for c in &st.calls {
        if c.purpose.starts_with("judge")
            || c.purpose.starts_with("extract")
            || c.purpose.starts_with("cluster")
        {
            let s = format!(
                "{} {}",
                c.model_returned.clone().unwrap_or_default(),
                c.fingerprint.clone().unwrap_or_default()
            )
            .trim()
            .to_string();
            if !s.is_empty() && !st.manifest.judge_models_returned.contains(&s) {
                st.manifest.judge_models_returned.push(s);
            }
        }
    }
    Ok(())
}

/// Run everything. A halt stops at once and is recorded; what ran is kept.
pub async fn run_all(deps: &Deps, st: &mut RunState, cases: &[(&Case, Materialized)]) {
    'outer: for repeat in 1..=deps.cfg.run.repeats {
        for (case, m) in cases {
            if let Err(e) = run_case_repeat(deps, st, case, m, repeat).await {
                st.halted = Some(e);
                break 'outer;
            }
        }
    }
    if st.halted.is_none() {
        let refs: Vec<&Case> = cases.iter().map(|(c, _)| *c).collect();
        if let Err(e) = judge_all(deps, st, &refs).await {
            st.halted = Some(format!("judging: {e}"));
        }
    }
}

/// Write the run's state files (scrubbed) to its directory.
pub fn write_state(deps: &Deps, st: &RunState) -> std::io::Result<()> {
    let s = &deps.scrubber;
    let d = &deps.run_dir;
    std::fs::create_dir_all(d)?;
    std::fs::write(d.join("manifest.json"), st.manifest.to_json(s))?;
    let v = serde_json::to_value(st).unwrap_or(Value::Null);
    write_json(&d.join("state.json"), &v, s)?;
    let b = st
        .budget
        .as_ref()
        .map(|b| serde_json::to_value(b.snapshot()).unwrap_or(Value::Null))
        .unwrap_or(Value::Null);
    write_json(&d.join("ledger.json"), &b, s)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::calls::testing::ok;
    use crate::fake_lippa::{FakeLippa, FakeSession};
    use crate::materialize::testrepo::*;

    pub const PANEL: [&str; 3] = ["claude-opus-5-5", "gemini-4-carbon", "gpt-6.2-sol"];

    fn tmpl(user: &str) -> Template {
        Template::parse(&format!("version: 1\n<!-- SYSTEM BEGIN -->\nsys\n<!-- SYSTEM END -->\n<!-- USER BEGIN -->\n{user}\n<!-- USER END -->\n"), "t").unwrap()
    }

    pub fn detail(fallback_on: Option<&str>) -> Value {
        let rounds: Vec<Value> = PANEL
            .iter()
            .map(|p| {
                json!({"round_index": 0, "model_profile_id": p,
                       "answered_model_id": if Some(*p) == fallback_on { json!("gpt-6-luna") } else { Value::Null },
                       "raw_response": format!("RECOMMENDATION: block\nThe add function subtracts. ({p})"), "error_text": null})
            })
            .collect();
        json!({"id": "s-1", "status": "converged", "credits_consumed": 3, "billing_state": "settled",
               "agreement": [{"claim_text": "add subtracts instead of adding", "confidence": 0.9, "supported_by": PANEL}],
               "divergence": [], "assumptions": [], "models": [], "rounds": rounds})
    }

    /// A mock that answers every role: S- calls with a claims block,
    /// extraction, clustering and judging with valid JSON.
    fn mock_models(seen: Arc<Mutex<Vec<ModelRequest>>>) -> Provider {
        Provider::Mock(Arc::new(move |r: &ModelRequest| {
            seen.lock().unwrap().push(r.clone());
            let between = |tag: &str| -> String {
                r.user
                    .split(&format!("<<<BENCH_{tag}_BEGIN>>>\n"))
                    .nth(1)
                    .and_then(|s| s.split(&format!("\n<<<BENCH_{tag}_END>>>")).next())
                    .unwrap_or("")
                    .to_string()
            };
            let text = match r.json_schema.as_ref().map(|s| s.0.as_str()) {
                None => "Looks wrong.\n---CLAIMS---\n{\"claims\": [{\"text\": \"add subtracts instead of adding\", \"confidence\": 0.9}]}\n---END CLAIMS---".to_string(),
                Some("extracted_claims") => json!({"claims": [{"text": "add subtracts instead of adding"}]}).to_string(),
                Some("clusters") => {
                    let ids: Vec<String> = between("FINDINGS").lines().map(|l| l.split(':').next().unwrap().to_string()).collect();
                    json!({"clusters": [{"members": ids}]}).to_string()
                }
                Some("decisions") => {
                    let ds: Vec<Value> = between("FINDINGS").lines().map(|l| json!({"finding": l.split(':').next().unwrap(), "class": "match", "defect": "D1", "uncertain": false, "reason": "same bug"})).collect();
                    json!({"decisions": ds}).to_string()
                }
                Some(other) => panic!("unexpected schema {other}"),
            };
            Ok(ok(&text))
        }))
    }

    pub async fn deps(td: &Path, lippa: &str, seen: Arc<Mutex<Vec<ModelRequest>>>) -> Deps {
        let mut cfg = BenchConfig::parse(crate::config::tests::SAMPLE).unwrap();
        cfg.run.tiers = vec!["high".into()];
        cfg.run.repeats = 1;
        cfg.lippa.project_id = "bench".into();
        cfg.s_arms.models.clear();
        for (id, vendor, effort) in [
            ("gpt-6.2-sol", "openai", "medium"),
            ("claude-opus-5-5", "anthropic", "low"),
            ("gemini-4-carbon", "google", "medium"),
        ] {
            cfg.s_arms.models.insert(
                id.into(),
                crate::config::SModel {
                    vendor: vendor.into(),
                    vendor_model: id.into(),
                    effort: effort.into(),
                },
            );
        }
        let prices = crate::prices::PriceTable::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("prices.toml"),
        )
        .unwrap();
        let m = mock_models(seen);
        let vendors = ["openai", "anthropic", "google"]
            .iter()
            .map(|v| (v.to_string(), m.clone()))
            .collect();
        Deps {
            cfg,
            prices,
            single: tmpl("Question:\n{{BUNDLE}}"),
            extraction: tmpl("{{ANSWER}}"),
            clustering: tmpl("{{DIFF}}\n{{FINDINGS}}"),
            rubric: tmpl("{{DIFF}}\n{{DEFECTS}}\n{{FINDINGS}}"),
            vendors,
            judge: Judge {
                provider: m,
                model: "grok-4.7".into(),
                output_cap: 1000,
                effort: Some("high".into()),
                seed: 1,
            },
            lippa: lippa.into(),
            session: Secret::new(format!("bench-test-cookie-{}", "5".repeat(16))),
            quorum_bin: crate::quorum_run::workspace_quorum_bin(),
            run_dir: td.join("runs/r1"),
            scrubber: Scrubber::new(vec![format!("bench-test-cookie-{}", "5".repeat(16))]),
            http: reqwest::Client::new(),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_full_case_repeat_runs_every_arm_and_judges() {
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
        let seen = Arc::new(Mutex::new(Vec::new()));
        let d = deps(td.path(), &fake.base_url(), seen.clone()).await;
        let mut st = RunState::new(100, 100_000_000);
        run_all(&d, &mut st, &[(&case, m)]).await;
        assert_eq!(st.halted, None);
        let arms: Vec<String> = st.outputs.iter().map(|o| o.arm.to_string()).collect();
        assert_eq!(
            arms,
            [
                "Q-high",
                "S-claude-opus-5-5",
                "S-gemini-4-carbon",
                "S-gpt-6.2-sol",
                "E-union",
                "E-majority"
            ]
        );
        assert!(
            st.outputs
                .iter()
                .all(|o| o.status == Status::Completed && !o.findings.is_empty()),
            "{:?}",
            st.outputs
        );
        assert_eq!(st.extractions.len(), 3, "one per Round 0 seat");
        assert_eq!(st.clusterings.len(), 1);
        assert!(st.uncontrolled.is_empty() && st.c2_excluded.is_empty());
        // Every arm raised the same title: one unique finding, one decision.
        assert_eq!(st.decisions.len(), 1);
        let s = st.budget.as_ref().unwrap().snapshot();
        assert_eq!(s.credits_spent, 3);
        assert!(s.usd_spent_micros > 0 && s.usd_spent_micros <= s.usd_cap_micros);
        // The S- arms got the exact Q-high bundle, wrapped.
        let reqs = seen.lock().unwrap();
        let s_req = reqs.iter().find(|r| r.json_schema.is_none()).unwrap();
        assert!(s_req
            .user
            .starts_with("Question:\n<<<QUORUM_REPO_CONTENT_BEGIN>>>"));
        assert_eq!(s_req.max_output_tokens, 16_384);
        // Scored end to end.
        let cases = BTreeMap::from([("rs-x-01".to_string(), &case)]);
        let scored = crate::score::score(
            &cases,
            &st.outputs,
            &crate::score::final_decisions(&st.decisions, &[]),
        )
        .unwrap();
        assert!(scored.obs.iter().all(|o| o.tp == 1 && o.fn_ == 0));
        write_state(&d, &st).unwrap();
        let all = std::fs::read_to_string(d.run_dir.join("state.json")).unwrap()
            + &std::fs::read_to_string(d.run_dir.join("cases/rs-x-01/r1/Q-high.json")).unwrap();
        assert!(
            !all.contains("bench-test-cookie"),
            "no cookie in any written file"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_fallback_seat_makes_the_case_repeat_uncontrolled() {
        let up = upstream();
        let td = tempfile::TempDir::new().unwrap();
        let case = case_for(&td.path().join("cases"), &up, "rs-x-01");
        let m =
            crate::materialize::materialize_from(&up.repo, &case, &td.path().join("repos/rs-x-01"))
                .unwrap();
        let fake = FakeLippa::start(FakeSession {
            detail: detail(Some("gpt-6.2-sol")),
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
        st.manifest.panel = PANEL
            .iter()
            .map(|p| (p.to_string(), p.to_string()))
            .collect();
        run_case_repeat(&d, &mut st, &case, &m, 1).await.unwrap();
        assert_eq!(st.uncontrolled, vec![("rs-x-01".to_string(), 1)]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn negative_control_the_run_halts_before_exceeding_the_usd_cap() {
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
        let seen = Arc::new(Mutex::new(Vec::new()));
        let d = deps(td.path(), &fake.base_url(), seen.clone()).await;
        // Enough for the Lippa session, not for one worst-case S- call.
        let mut st = RunState::new(100, 50_000);
        run_all(&d, &mut st, &[(&case, m)]).await;
        assert!(
            st.halted.as_deref().unwrap().contains("stopping"),
            "{:?}",
            st.halted
        );
        assert!(seen.lock().unwrap().is_empty(), "no direct call was made");
        assert_eq!(st.budget.as_ref().unwrap().snapshot().usd_spent_micros, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn negative_control_the_run_halts_before_exceeding_the_credit_cap() {
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
        let mut st = RunState::new(2, 100_000_000);
        run_all(&d, &mut st, &[(&case, m)]).await;
        assert!(st.halted.is_some());
        assert!(st.outputs.is_empty(), "no review ran");
    }

    #[test]
    fn round0_seat_parsing() {
        let mut d = detail(Some("gpt-6.2-sol"));
        d["rounds"][1]["error_text"] = json!("timeout");
        d["rounds"]
            .as_array_mut()
            .unwrap()
            .push(json!({"round_index": 1, "model_profile_id": "x", "raw_response": "later"}));
        let s = round0_seats(&d);
        assert_eq!(s.len(), 3);
        assert_eq!(
            s.iter()
                .find(|x| x.profile_id == "gpt-6.2-sol")
                .unwrap()
                .observed_model,
            "gpt-6-luna"
        );
        assert_eq!(s.iter().filter(|x| x.healthy).count(), 2);
    }
}
