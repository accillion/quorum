//! From a finished run (and its finished human queue) to the claims of
//! AC 254 and the descriptive metrics of AC 253. Every result is labelled
//! **pilot — exploratory** (AC 243).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::corpus::{Case, Kind};
use crate::pipeline::Decision;
use crate::score::{final_decisions, score, ArmOutput, SideCounts};
use crate::stats::{
    build_table, c2_label, descriptive, paired_bootstrap, Contrast, Descriptive, Interval, Label,
};

pub const PILOT_LABEL: &str = "pilot — exploratory";

/// What `results` reads back from a run directory's `state.json`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SavedState {
    pub outputs: Vec<ArmOutput>,
    pub decisions: Vec<Decision>,
    #[serde(default)]
    pub uncontrolled: Vec<(String, u32)>,
    #[serde(default)]
    pub c2_excluded: Vec<(String, u32, String)>,
    #[serde(default)]
    pub extractions: Vec<crate::pipeline::Extraction>,
    #[serde(default)]
    pub clusterings: Vec<crate::pipeline::Clustering>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Results {
    pub label: &'static str,
    pub c1: Interval,
    pub c2_vs_majority: Interval,
    pub c2_vs_union: Interval,
    pub c2: Label,
    pub descriptive: BTreeMap<String, Descriptive>,
    pub side: BTreeMap<String, SideCounts>,
    pub uncontrolled: Vec<(String, u32)>,
    pub c2_excluded: Vec<(String, u32, String)>,
}

pub fn compute(
    cases: &[Case],
    saved: &SavedState,
    human: &[Decision],
    draws: usize,
    seed: u64,
) -> Result<Results, Vec<String>> {
    let by_id: BTreeMap<String, &Case> = cases.iter().map(|c| (c.id().to_string(), c)).collect();
    let decisions = final_decisions(&saved.decisions, human);
    let scored = score(&by_id, &saved.outputs, &decisions)?;
    let uncontrolled: BTreeSet<(String, u32)> = saved.uncontrolled.iter().cloned().collect();
    let c2_out: BTreeSet<(String, u32)> = saved
        .c2_excluded
        .iter()
        .map(|(c, r, _)| (c.clone(), *r))
        .collect();
    let arms: BTreeSet<String> = scored.obs.iter().map(|o| o.arm.clone()).collect();
    let s_arms: Vec<String> = arms
        .iter()
        .filter(|a| a.starts_with("S-"))
        .cloned()
        .collect();

    let controlled = |c: &str, r: u32| !uncontrolled.contains(&(c.to_string(), r));
    let mut c1_arms = vec!["Q-high".to_string()];
    c1_arms.extend(s_arms.iter().cloned());
    let c1_table = build_table(&scored.obs, &c1_arms, &controlled);
    let c1 = paired_bootstrap(
        &c1_table,
        "Q-high",
        &Contrast::BestOf(s_arms.clone()),
        draws,
        seed,
    );

    let c2_ok = |c: &str, r: u32| controlled(c, r) && !c2_out.contains(&(c.to_string(), r));
    let c2_arms = vec![
        "Q-high".to_string(),
        "E-union".to_string(),
        "E-majority".to_string(),
    ];
    let c2_table = build_table(&scored.obs, &c2_arms, &c2_ok);
    let vs_m = paired_bootstrap(
        &c2_table,
        "Q-high",
        &Contrast::Arm("E-majority".into()),
        draws,
        seed,
    );
    let vs_u = paired_bootstrap(
        &c2_table,
        "Q-high",
        &Contrast::Arm("E-union".into()),
        draws,
        seed,
    );
    let c2 = c2_label(&vs_m.label, &vs_u.label);

    let clean: Vec<String> = cases
        .iter()
        .filter(|c| c.meta.kind == Kind::Clean)
        .map(|c| c.id().to_string())
        .collect();
    let mut desc = BTreeMap::new();
    for a in &arms {
        let t = build_table(&scored.obs, std::slice::from_ref(a), &controlled);
        desc.insert(a.clone(), descriptive(a, &t, &clean, &scored.hits));
    }
    Ok(Results {
        label: PILOT_LABEL,
        c1,
        c2_vs_majority: vs_m,
        c2_vs_union: vs_u,
        c2,
        descriptive: desc,
        side: scored.side,
        uncontrolled: saved.uncontrolled.clone(),
        c2_excluded: saved.c2_excluded.clone(),
    })
}
