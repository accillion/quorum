//! Scoring statistics (AC 253, AC 254).
//!
//! **Primary endpoint:** micro-averaged defect-level F1. For each arm, TP,
//! FP and FN are averaged per case over its repeats, summed over cases,
//! and F1 = 2·TP / (2·TP + FP + FN). FP counts every adjudicated false
//! alarm, in bug and clean cases.
//!
//! **Interval:** a paired two-stage bootstrap, by default 10,000 draws.
//! Each draw resamples source repositories with replacement, then, within
//! each drawn repository, its cases with replacement; it recomputes every
//! arm's F1 on that multiset and takes the difference. The best `S-` arm
//! is re-chosen inside every draw. The interval is the 2.5th–97.5th
//! percentile of the differences (linear interpolation between order
//! statistics).
//!
//! **Undefined draws:** F1 is undefined when 2·TP + FP + FN = 0 (a draw
//! of clean cases only, where an arm raised nothing). Such a draw has no
//! difference; it is counted and left out of the percentiles. If more
//! than 5% of draws are undefined the claim is labelled *not tested*.
//!
//! **Labels:** *supported* (lower bound above zero), *contradicted*
//! (upper bound below zero), *inconclusive* (the interval contains zero)
//! or *not tested*.

use std::collections::BTreeMap;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};

pub const DEFAULT_DRAWS: usize = 10_000;
/// Above this share of undefined draws a comparison is not tested.
pub const MAX_UNDEFINED_SHARE: f64 = 0.05;

/// One arm's adjudicated counts for one case and one repeat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Obs {
    pub arm: String,
    pub case_id: String,
    /// The source repository (`owner/repo`): the bootstrap's cluster.
    pub repo: String,
    pub repeat: u32,
    pub tp: u32,
    pub fp: u32,
    #[serde(rename = "fn")]
    pub fn_: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Counts {
    pub tp: f64,
    pub fp: f64,
    #[serde(rename = "fn")]
    pub fn_: f64,
}

impl std::ops::AddAssign for Counts {
    fn add_assign(&mut self, o: Counts) {
        self.tp += o.tp;
        self.fp += o.fp;
        self.fn_ += o.fn_;
    }
}

/// F1 from pooled counts; `None` when 2·TP + FP + FN = 0.
pub fn micro_f1(c: Counts) -> Option<f64> {
    let d = 2.0 * c.tp + c.fp + c.fn_;
    if d <= 0.0 {
        None
    } else {
        Some(2.0 * c.tp / d)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CaseRow {
    pub case_id: String,
    pub repo: String,
    /// Repeat-averaged counts per arm.
    pub arms: BTreeMap<String, Counts>,
}

/// The paired table for one comparison: only cases where every listed arm
/// has the same non-empty set of eligible repeats.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub rows: Vec<CaseRow>,
    /// Cases dropped because an arm lacked an eligible repeat.
    pub dropped: Vec<String>,
}

/// Build the paired table. `eligible(case, repeat)` filters case-repeats
/// (for example, uncontrolled panels for C1 and C2, or case-repeats with
/// fewer than three healthy Round 0 seats for C2). Within a case only the
/// repeats every arm has are used, so each arm averages the same repeats.
pub fn build_table(obs: &[Obs], arms: &[String], eligible: &dyn Fn(&str, u32) -> bool) -> Table {
    /// Per arm, per repeat.
    type ByArm<'a> = BTreeMap<&'a str, BTreeMap<u32, &'a Obs>>;
    let mut by_case: BTreeMap<&str, (&str, ByArm)> = BTreeMap::new();
    for o in obs
        .iter()
        .filter(|o| arms.contains(&o.arm) && eligible(&o.case_id, o.repeat))
    {
        let e = by_case
            .entry(&o.case_id)
            .or_insert((&o.repo, BTreeMap::new()));
        e.1.entry(&o.arm).or_default().insert(o.repeat, o);
    }
    let all_cases: std::collections::BTreeSet<&str> =
        obs.iter().map(|o| o.case_id.as_str()).collect();
    let mut rows = Vec::new();
    let mut dropped = Vec::new();
    for case in all_cases {
        let Some((repo, per_arm)) = by_case.get(case) else {
            dropped.push(case.to_string());
            continue;
        };
        let mut common: Option<std::collections::BTreeSet<u32>> = None;
        for a in arms {
            let reps: std::collections::BTreeSet<u32> = per_arm
                .get(a.as_str())
                .map(|m| m.keys().copied().collect())
                .unwrap_or_default();
            common = Some(match common {
                None => reps,
                Some(c) => c.intersection(&reps).copied().collect(),
            });
        }
        let common = common.unwrap_or_default();
        if common.is_empty() {
            dropped.push(case.to_string());
            continue;
        }
        let n = common.len() as f64;
        let mut arm_counts = BTreeMap::new();
        for a in arms {
            let m = &per_arm[a.as_str()];
            let mut c = Counts::default();
            for r in &common {
                let o = m[r];
                c.tp += o.tp as f64;
                c.fp += o.fp as f64;
                c.fn_ += o.fn_ as f64;
            }
            arm_counts.insert(
                a.clone(),
                Counts {
                    tp: c.tp / n,
                    fp: c.fp / n,
                    fn_: c.fn_ / n,
                },
            );
        }
        rows.push(CaseRow {
            case_id: case.to_string(),
            repo: repo.to_string(),
            arms: arm_counts,
        });
    }
    Table { rows, dropped }
}

/// The other side of a comparison.
#[derive(Debug, Clone, PartialEq)]
pub enum Contrast {
    Arm(String),
    /// The best of these arms by F1, chosen on the sample at hand.
    BestOf(Vec<String>),
}

fn f1_on(table: &Table, arm: &str, idx: &[usize]) -> Option<f64> {
    let mut c = Counts::default();
    for &i in idx {
        c += table.rows[i].arms[arm];
    }
    micro_f1(c)
}

/// The difference F1(a) − F1(contrast) on one multiset of case indices,
/// and the arm the contrast resolved to. Among `BestOf` arms with an
/// undefined F1 none can be best; ties go to the first listed.
pub fn diff_on(
    table: &Table,
    a: &str,
    b: &Contrast,
    idx: &[usize],
) -> (Option<f64>, Option<String>) {
    let fa = f1_on(table, a, idx);
    let (fb, chosen) = match b {
        Contrast::Arm(x) => (f1_on(table, x, idx), Some(x.clone())),
        Contrast::BestOf(xs) => {
            let mut best: Option<(f64, &String)> = None;
            for x in xs {
                if let Some(v) = f1_on(table, x, idx) {
                    if best.map_or(true, |(bv, _)| v > bv) {
                        best = Some((v, x));
                    }
                }
            }
            (best.map(|b| b.0), best.map(|b| b.1.clone()))
        }
    };
    match (fa, fb) {
        (Some(x), Some(y)) => (Some(x - y), chosen),
        _ => (None, chosen),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "label", content = "reason", rename_all = "snake_case")]
pub enum Label {
    Supported,
    Inconclusive,
    Contradicted,
    NotTested(String),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Interval {
    pub point: Option<f64>,
    /// The contrast arm on the full sample (the best `S-` arm for C1).
    pub point_contrast: Option<String>,
    pub lo: Option<f64>,
    pub hi: Option<f64>,
    pub draws: usize,
    pub undefined_draws: usize,
    pub cases: usize,
    pub repos: usize,
    pub label: Label,
}

/// Linear-interpolation percentile (`q` in 0..=1) of sorted values.
pub fn percentile(sorted: &[f64], q: f64) -> f64 {
    let h = (sorted.len() - 1) as f64 * q;
    let lo = h.floor() as usize;
    let hi = h.ceil() as usize;
    sorted[lo] + (h - lo as f64) * (sorted[hi] - sorted[lo])
}

/// One two-stage resample: repositories with replacement, then each drawn
/// repository's cases with replacement.
pub fn resample(groups: &[Vec<usize>], rng: &mut StdRng) -> Vec<usize> {
    let mut idx = Vec::new();
    for _ in 0..groups.len() {
        let g = &groups[rng.gen_range(0..groups.len())];
        for _ in 0..g.len() {
            idx.push(g[rng.gen_range(0..g.len())]);
        }
    }
    idx
}

pub fn repo_groups(table: &Table) -> Vec<Vec<usize>> {
    let mut m: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, r) in table.rows.iter().enumerate() {
        m.entry(&r.repo).or_default().push(i);
    }
    m.into_values().collect()
}

pub fn paired_bootstrap(table: &Table, a: &str, b: &Contrast, draws: usize, seed: u64) -> Interval {
    let groups = repo_groups(table);
    let all: Vec<usize> = (0..table.rows.len()).collect();
    let (point, point_contrast) = if all.is_empty() {
        (None, None)
    } else {
        diff_on(table, a, b, &all)
    };
    let mut base = Interval {
        point,
        point_contrast,
        lo: None,
        hi: None,
        draws,
        undefined_draws: 0,
        cases: table.rows.len(),
        repos: groups.len(),
        label: Label::NotTested(String::new()),
    };
    if groups.len() < 2 {
        base.label = Label::NotTested(format!(
            "{} source repositories; the bootstrap needs at least 2",
            groups.len()
        ));
        return base;
    }
    let mut rng = StdRng::seed_from_u64(seed);
    let mut diffs = Vec::with_capacity(draws);
    for _ in 0..draws {
        let idx = resample(&groups, &mut rng);
        match diff_on(table, a, b, &idx).0 {
            Some(d) => diffs.push(d),
            None => base.undefined_draws += 1,
        }
    }
    if diffs.is_empty() || base.undefined_draws as f64 > MAX_UNDEFINED_SHARE * draws as f64 {
        base.label = Label::NotTested(format!(
            "F1 undefined in {} of {draws} draws",
            base.undefined_draws
        ));
        return base;
    }
    diffs.sort_by(|x, y| x.partial_cmp(y).expect("finite"));
    let lo = percentile(&diffs, 0.025);
    let hi = percentile(&diffs, 0.975);
    base.lo = Some(lo);
    base.hi = Some(hi);
    base.label = if lo > 0.0 {
        Label::Supported
    } else if hi < 0.0 {
        Label::Contradicted
    } else {
        Label::Inconclusive
    };
    base
}

/// C2's label: supported only when both differences are supported
/// (AC 254); contradicted only when both are; not tested if either is.
pub fn c2_label(vs_majority: &Label, vs_union: &Label) -> Label {
    match (vs_majority, vs_union) {
        (Label::NotTested(r), _) | (_, Label::NotTested(r)) => Label::NotTested(r.clone()),
        (Label::Supported, Label::Supported) => Label::Supported,
        (Label::Contradicted, Label::Contradicted) => Label::Contradicted,
        _ => Label::Inconclusive,
    }
}

// ── Descriptive metrics (AC 253; they authorise no claim) ─────────────────

/// One expected defect's outcome for one arm and repeat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DefectHit {
    pub arm: String,
    pub case_id: String,
    pub defect_id: String,
    pub high: bool,
    pub repeat: u32,
    pub found: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Descriptive {
    pub recall_all: Option<f64>,
    pub recall_high: Option<f64>,
    pub precision: Option<f64>,
    pub false_alarms_per_clean_case: Option<f64>,
    /// Share of expected defects found in every repeat.
    pub stability: Option<f64>,
}

fn ratio(n: f64, d: f64) -> Option<f64> {
    (d > 0.0).then(|| n / d)
}

pub fn descriptive(
    arm: &str,
    table: &Table,
    clean_cases: &[String],
    hits: &[DefectHit],
) -> Descriptive {
    let mut c = Counts::default();
    let mut clean_fp = 0.0;
    let mut clean_n = 0.0;
    for r in &table.rows {
        if let Some(x) = r.arms.get(arm) {
            c += *x;
            if clean_cases.contains(&r.case_id) {
                clean_fp += x.fp;
                clean_n += 1.0;
            }
        }
    }
    let in_table = |case: &str| table.rows.iter().any(|r| r.case_id == case);
    let mine: Vec<&DefectHit> = hits
        .iter()
        .filter(|h| h.arm == arm && in_table(&h.case_id))
        .collect();
    let high: Vec<&&DefectHit> = mine.iter().filter(|h| h.high).collect();
    let mut per_defect: BTreeMap<(&str, &str), (u32, u32)> = BTreeMap::new();
    for h in &mine {
        let e = per_defect.entry((&h.case_id, &h.defect_id)).or_default();
        e.0 += 1;
        e.1 += h.found as u32;
    }
    Descriptive {
        recall_all: ratio(c.tp, c.tp + c.fn_),
        recall_high: ratio(
            high.iter().filter(|h| h.found).count() as f64,
            high.len() as f64,
        ),
        precision: ratio(c.tp, c.tp + c.fp),
        false_alarms_per_clean_case: ratio(clean_fp, clean_n),
        stability: ratio(
            per_defect
                .values()
                .filter(|(n, f)| n == f && *n > 0)
                .count() as f64,
            per_defect.len() as f64,
        ),
    }
}

pub fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o(arm: &str, case: &str, repo: &str, repeat: u32, tp: u32, fp: u32, fn_: u32) -> Obs {
        Obs {
            arm: arm.into(),
            case_id: case.into(),
            repo: repo.into(),
            repeat,
            tp,
            fp,
            fn_,
        }
    }

    fn arms(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn all(_: &str, _: u32) -> bool {
        true
    }

    #[test]
    fn micro_f1_known_values() {
        assert_eq!(
            micro_f1(Counts {
                tp: 3.0,
                fp: 1.0,
                fn_: 1.0
            }),
            Some(0.75)
        );
        assert_eq!(
            micro_f1(Counts {
                tp: 0.0,
                fp: 2.0,
                fn_: 0.0
            }),
            Some(0.0)
        );
        assert_eq!(micro_f1(Counts::default()), None);
    }

    #[test]
    fn repeats_are_averaged_per_case_and_pooled_over_cases() {
        // Case c1: TP 1,0,1 -> 2/3; FN 0,1,0 -> 1/3. Case c2: FP 3 once in each repeat.
        let mut obs = Vec::new();
        for (r, tp, fn_) in [(1, 1, 0), (2, 0, 1), (3, 1, 0)] {
            obs.push(o("Q", "c1", "a/x", r, tp, 0, fn_));
        }
        for r in 1..=3 {
            obs.push(o("Q", "c2", "a/y", r, 0, 3, 0));
        }
        let t = build_table(&obs, &arms(&["Q"]), &all);
        let c1 = t.rows[0].arms["Q"];
        assert!((c1.tp - 2.0 / 3.0).abs() < 1e-12 && (c1.fn_ - 1.0 / 3.0).abs() < 1e-12);
        // F1 = 2*(2/3) / (2*(2/3) + 3 + 1/3) = (4/3) / (14/3) = 2/7.
        let f = f1_on(&t, "Q", &[0, 1]).unwrap();
        assert!((f - 2.0 / 7.0).abs() < 1e-12, "{f}");
    }

    #[test]
    fn pairing_uses_only_repeats_every_arm_has() {
        let obs = vec![
            o("Q", "c1", "r", 1, 1, 0, 0),
            o("Q", "c1", "r", 2, 0, 0, 1),
            o("E", "c1", "r", 2, 0, 0, 1),
            o("Q", "c2", "r", 1, 1, 0, 0),
        ];
        let t = build_table(&obs, &arms(&["Q", "E"]), &all);
        assert_eq!(t.rows.len(), 1);
        assert_eq!(
            t.rows[0].arms["Q"],
            Counts {
                tp: 0.0,
                fp: 0.0,
                fn_: 1.0
            },
            "only repeat 2 is paired"
        );
        assert_eq!(t.dropped, vec!["c2".to_string()]);
        // An eligibility filter drops a case-repeat for every arm.
        let t = build_table(&obs, &arms(&["Q"]), &|c, r| !(c == "c1" && r == 1));
        assert_eq!(t.rows[0].arms["Q"].fn_, 1.0);
    }

    fn corpus(n_repos: usize, per_repo: usize, f: impl Fn(usize, usize) -> Vec<Obs>) -> Vec<Obs> {
        let mut v = Vec::new();
        for r in 0..n_repos {
            for c in 0..per_repo {
                v.extend(f(r, c));
            }
        }
        v
    }

    #[test]
    fn known_answer_supported_contradicted_and_inconclusive() {
        // Q finds every defect; S misses every defect: every draw's
        // difference is exactly 1 - 0.
        let obs = corpus(4, 3, |r, c| {
            let case = format!("r{r}c{c}");
            let repo = format!("o/r{r}");
            vec![
                o("Q", &case, &repo, 1, 1, 0, 0),
                o("S", &case, &repo, 1, 0, 0, 1),
            ]
        });
        let t = build_table(&obs, &arms(&["Q", "S"]), &all);
        let i = paired_bootstrap(&t, "Q", &Contrast::Arm("S".into()), 2_000, 1);
        assert_eq!((i.point, i.lo, i.hi), (Some(1.0), Some(1.0), Some(1.0)));
        assert_eq!(i.label, Label::Supported);
        let i = paired_bootstrap(&t, "S", &Contrast::Arm("Q".into()), 2_000, 1);
        assert_eq!(i.label, Label::Contradicted);
        let i = paired_bootstrap(&t, "Q", &Contrast::Arm("Q".into()), 2_000, 1);
        assert_eq!((i.lo, i.hi), (Some(0.0), Some(0.0)));
        assert_eq!(
            i.label,
            Label::Inconclusive,
            "an interval touching zero is inconclusive"
        );
    }

    #[test]
    fn a_noisy_difference_spanning_zero_is_inconclusive() {
        // Q wins on repo 0, loses on repo 1, ties elsewhere.
        let obs = corpus(4, 4, |r, c| {
            let case = format!("r{r}c{c}");
            let repo = format!("o/r{r}");
            let (q, s) = match r {
                0 => ((1, 0, 0), (0, 0, 1)),
                1 => ((0, 0, 1), (1, 0, 0)),
                _ => ((1, 0, 0), (1, 0, 0)),
            };
            vec![
                o("Q", &case, &repo, 1, q.0, q.1, q.2),
                o("S", &case, &repo, 1, s.0, s.1, s.2),
            ]
        });
        let t = build_table(&obs, &arms(&["Q", "S"]), &all);
        let i = paired_bootstrap(&t, "Q", &Contrast::Arm("S".into()), DEFAULT_DRAWS, 7);
        assert_eq!(i.point, Some(0.0));
        assert!(i.lo.unwrap() < 0.0 && i.hi.unwrap() > 0.0);
        assert_eq!(i.label, Label::Inconclusive);
        // Seeded: the same seed reproduces the same interval.
        assert_eq!(
            i,
            paired_bootstrap(&t, "Q", &Contrast::Arm("S".into()), DEFAULT_DRAWS, 7)
        );
    }

    #[test]
    fn the_best_single_arm_is_rechosen_in_every_draw() {
        // S1 is perfect on repo A and blind on repo B; S2 the opposite.
        let obs = corpus(2, 2, |r, c| {
            let case = format!("r{r}c{c}");
            let repo = format!("o/r{r}");
            let (s1, s2) = if r == 0 {
                ((1, 0, 0), (0, 0, 1))
            } else {
                ((0, 0, 1), (1, 0, 0))
            };
            vec![
                o("Q", &case, &repo, 1, 1, 1, 0),
                o("S1", &case, &repo, 1, s1.0, s1.1, s1.2),
                o("S2", &case, &repo, 1, s2.0, s2.1, s2.2),
            ]
        });
        let t = build_table(&obs, &arms(&["Q", "S1", "S2"]), &all);
        let best = Contrast::BestOf(arms(&["S1", "S2"]));
        let repo_a: Vec<usize> = t
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.repo == "o/r0")
            .map(|(i, _)| i)
            .collect();
        let repo_b: Vec<usize> = t
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.repo == "o/r1")
            .map(|(i, _)| i)
            .collect();
        let (da, ca) = diff_on(&t, "Q", &best, &repo_a);
        let (db, cb) = diff_on(&t, "Q", &best, &repo_b);
        assert_eq!(ca.as_deref(), Some("S1"));
        assert_eq!(cb.as_deref(), Some("S2"));
        // Q: F1 = 2/(2+1) on any sample; the per-draw best is perfect.
        assert!((da.unwrap() - (2.0 / 3.0 - 1.0)).abs() < 1e-12);
        assert_eq!(da, db);
        // Pooled, Q, S1 and S2 all have F1 2/3. Re-chosen per draw, the
        // best single arm is never worse than Q, so no draw favours Q. A
        // fixed choice of S1 would let repo-B-only draws (S1's F1 = 0)
        // push the interval up to +2/3.
        let i = paired_bootstrap(&t, "Q", &best, 2_000, 3);
        assert!(i.point.unwrap().abs() < 1e-12);
        assert!(i.hi.unwrap() <= 1e-12 && i.lo.unwrap() < 0.0, "{i:?}");
        let fixed = paired_bootstrap(&t, "Q", &Contrast::Arm("S1".into()), 2_000, 3);
        assert!(fixed.hi.unwrap() > 0.5, "{fixed:?}");
    }

    #[test]
    fn clean_only_resamples_are_undefined_and_counted() {
        // Repo "clean" holds only clean cases where no arm raised anything;
        // a draw that picks it twice has F1 undefined for both arms.
        let mut obs = Vec::new();
        for c in 0..3 {
            obs.push(o("Q", &format!("clean{c}"), "o/clean", 1, 0, 0, 0));
            obs.push(o("S", &format!("clean{c}"), "o/clean", 1, 0, 0, 0));
            obs.push(o("Q", &format!("bug{c}"), "o/bug", 1, 1, 0, 0));
            obs.push(o("S", &format!("bug{c}"), "o/bug", 1, 0, 0, 1));
        }
        let t = build_table(&obs, &arms(&["Q", "S"]), &all);
        let clean_idx: Vec<usize> = (0..t.rows.len())
            .filter(|&i| t.rows[i].repo == "o/clean")
            .collect();
        assert_eq!(
            diff_on(&t, "Q", &Contrast::Arm("S".into()), &clean_idx).0,
            None
        );
        let i = paired_bootstrap(&t, "Q", &Contrast::Arm("S".into()), 4_000, 11);
        // P(both repo draws are "clean") = 1/4.
        let share = i.undefined_draws as f64 / 4_000.0;
        assert!((share - 0.25).abs() < 0.03, "{share}");
        assert!(matches!(i.label, Label::NotTested(_)), "{:?}", i.label);

        // A clean-only draw where one arm raised a false alarm is defined:
        // that arm's F1 is 0, the silent arm's undefined, so no difference.
        let mut obs2 = obs.clone();
        for x in obs2
            .iter_mut()
            .filter(|x| x.arm == "S" && x.repo == "o/clean")
        {
            x.fp = 1;
        }
        let t2 = build_table(&obs2, &arms(&["Q", "S"]), &all);
        assert_eq!(f1_on(&t2, "S", &clean_idx), Some(0.0));
        assert_eq!(
            diff_on(&t2, "Q", &Contrast::Arm("S".into()), &clean_idx).0,
            None
        );
    }

    #[test]
    fn one_repository_is_not_tested() {
        let obs = vec![
            o("Q", "c", "o/r", 1, 1, 0, 0),
            o("S", "c", "o/r", 1, 0, 0, 1),
        ];
        let t = build_table(&obs, &arms(&["Q", "S"]), &all);
        assert!(matches!(
            paired_bootstrap(&t, "Q", &Contrast::Arm("S".into()), 100, 1).label,
            Label::NotTested(_)
        ));
    }

    #[test]
    fn two_stage_resample_respects_repository_sizes() {
        let groups = vec![vec![0, 1, 2], vec![3]];
        let mut rng = StdRng::seed_from_u64(5);
        for _ in 0..200 {
            let idx = resample(&groups, &mut rng);
            // Two repo draws: sizes 3+3, 3+1 or 1+1.
            assert!(matches!(idx.len(), 6 | 4 | 2), "{idx:?}");
        }
    }

    #[test]
    fn c2_needs_both_supported() {
        use Label::*;
        assert_eq!(c2_label(&Supported, &Supported), Supported);
        assert_eq!(c2_label(&Supported, &Inconclusive), Inconclusive);
        assert_eq!(c2_label(&Contradicted, &Contradicted), Contradicted);
        assert_eq!(c2_label(&Supported, &Contradicted), Inconclusive);
        assert!(matches!(
            c2_label(&Supported, &NotTested("x".into())),
            NotTested(_)
        ));
    }

    #[test]
    fn percentile_and_median() {
        let v = [0.0, 1.0, 2.0, 3.0, 4.0];
        assert_eq!(percentile(&v, 0.5), 2.0);
        assert_eq!(percentile(&v, 0.025), 0.1);
        assert_eq!(median(vec![3.0, 1.0, 2.0, 10.0]), Some(2.5));
        assert_eq!(median(vec![]), None);
    }

    #[test]
    fn descriptive_metrics() {
        let obs = vec![
            o("Q", "b1", "r", 1, 1, 1, 0),
            o("Q", "b1", "r", 2, 0, 0, 1),
            o("Q", "k1", "r", 1, 0, 2, 0),
            o("Q", "k1", "r", 2, 0, 0, 0),
        ];
        let t = build_table(&obs, &arms(&["Q"]), &all);
        let hits = vec![
            DefectHit {
                arm: "Q".into(),
                case_id: "b1".into(),
                defect_id: "D1".into(),
                high: true,
                repeat: 1,
                found: true,
            },
            DefectHit {
                arm: "Q".into(),
                case_id: "b1".into(),
                defect_id: "D1".into(),
                high: true,
                repeat: 2,
                found: false,
            },
        ];
        let d = descriptive("Q", &t, &["k1".to_string()], &hits);
        assert_eq!(d.recall_all, Some(0.5));
        assert_eq!(d.recall_high, Some(0.5));
        assert_eq!(d.false_alarms_per_clean_case, Some(1.0));
        assert_eq!(d.stability, Some(0.0));
        // precision: TP 0.5 / (0.5 + FP 0.5 + 1.0) = 0.25
        assert_eq!(d.precision, Some(0.25));
    }
}
