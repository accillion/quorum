//! Scoring (AC 250a, AC 252): from each arm's findings and the final
//! decisions to TP / FP / FN per arm, case and repeat.
//!
//! - Matching is one-to-one per expected defect within an arm and repeat:
//!   the first finding (in the arm's own order) that matches a defect is
//!   the TP; later findings matching the same defect are `duplicate`.
//! - `false alarm` is an FP. `pre-existing issue` and `unexpected true
//!   positive` are neither, and are counted separately.
//! - A review that still ended `error` after its retry has zero findings:
//!   every expected defect of that case and repeat is a miss (AC 250a).
//! - Scoring refuses while any finding lacks a final decision or is still
//!   `unresolved`: the blind human queue must be finished first.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::corpus::{Case, Severity};
use crate::findings::{finding_key, strip, Arm, RawFinding};
use crate::pipeline::{Class, Decision};
use crate::stats::{DefectHit, Obs};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Completed,
    /// Ended in error after its one retry (AC 250a).
    Error(String),
}

/// One arm's result for one case and repeat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArmOutput {
    pub arm: Arm,
    pub case_id: String,
    pub repeat: u32,
    pub status: Status,
    pub findings: Vec<RawFinding>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SideCounts {
    pub duplicates: u32,
    pub pre_existing: u32,
    pub unexpected_tp: u32,
    pub no_issue: u32,
    pub errors: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Scored {
    pub obs: Vec<Obs>,
    pub hits: Vec<DefectHit>,
    pub side: BTreeMap<String, SideCounts>,
}

/// The final decision per (case, finding key): a human decision replaces
/// the judge's.
pub fn final_decisions(
    judge: &[Decision],
    human: &[Decision],
) -> BTreeMap<(String, String), Decision> {
    let mut m: BTreeMap<(String, String), Decision> = BTreeMap::new();
    for d in judge.iter().chain(human.iter()) {
        m.insert((d.case_id.clone(), d.key.clone()), d.clone());
    }
    m
}

pub fn score(
    cases: &BTreeMap<String, &Case>,
    outputs: &[ArmOutput],
    decisions: &BTreeMap<(String, String), Decision>,
) -> Result<Scored, Vec<String>> {
    let mut out = Scored::default();
    let mut missing = BTreeSet::new();
    for o in outputs {
        let Some(case) = cases.get(&o.case_id) else {
            missing.insert(format!("{}: unknown case", o.case_id));
            continue;
        };
        let arm = o.arm.to_string();
        let side = out.side.entry(arm.clone()).or_default();
        let mut matched: BTreeSet<&str> = BTreeSet::new();
        let (mut tp, mut fp) = (0, 0);
        let findings: &[RawFinding] = match &o.status {
            Status::Completed => &o.findings,
            Status::Error(_) => {
                side.errors += 1;
                &[]
            }
        };
        let mut ordered: Vec<&RawFinding> = findings.iter().collect();
        ordered.sort_by_key(|f| (f.seat.clone(), f.ordinal));
        for f in ordered {
            let key = finding_key(&o.case_id, &strip(&f.title));
            let Some(d) = decisions.get(&(o.case_id.clone(), key.clone())) else {
                missing.insert(format!(
                    "{}: no decision for finding {}",
                    o.case_id,
                    &key[..12]
                ));
                continue;
            };
            match d.class {
                Class::Match => {
                    let id = d.defect.as_deref().unwrap_or("");
                    if case.defects().iter().all(|x| x.id != id) {
                        missing
                            .insert(format!("{}: decision names unknown defect {id}", o.case_id));
                    } else if matched.insert(id) {
                        tp += 1;
                    } else {
                        side.duplicates += 1;
                    }
                }
                Class::FalseAlarm => fp += 1,
                Class::PreExisting => side.pre_existing += 1,
                Class::UnexpectedTp => side.unexpected_tp += 1,
                Class::NoIssue => side.no_issue += 1,
                Class::Unresolved => {
                    missing.insert(format!(
                        "{}: finding {} is still unresolved",
                        o.case_id,
                        &key[..12]
                    ));
                }
            }
        }
        let defects = case.defects();
        out.obs.push(Obs {
            arm: arm.clone(),
            case_id: o.case_id.clone(),
            repo: case.source_repo_key(),
            repeat: o.repeat,
            tp,
            fp,
            fn_: (defects.len() - matched.len()) as u32,
        });
        for d in defects {
            out.hits.push(DefectHit {
                arm: arm.clone(),
                case_id: o.case_id.clone(),
                defect_id: d.id.clone(),
                high: d.severity == Severity::High,
                repeat: o.repeat,
                found: matched.contains(d.id.as_str()),
            });
        }
    }
    if missing.is_empty() {
        Ok(out)
    } else {
        Err(missing.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case() -> (tempfile::TempDir, Case) {
        let td = tempfile::TempDir::new().unwrap();
        let d = crate::corpus::testcase::write_case(td.path(), "rs-x-01", "");
        let c = crate::corpus::load_case(&d).unwrap();
        (td, c)
    }

    fn raw(arm: &Arm, i: usize, title: &str) -> RawFinding {
        RawFinding {
            arm: arm.clone(),
            case_id: "rs-x-01".into(),
            repeat: 1,
            seat: None,
            ordinal: i,
            title: title.into(),
        }
    }

    fn dec(title: &str, class: Class, defect: Option<&str>, by: &str) -> Decision {
        Decision {
            case_id: "rs-x-01".into(),
            key: finding_key("rs-x-01", &strip(title)),
            class,
            defect: defect.map(str::to_string),
            uncertain: false,
            reason: String::new(),
            by: by.into(),
            model_returned: None,
        }
    }

    #[test]
    fn one_to_one_matching_duplicates_and_side_classes() {
        let (_td, c) = case();
        let cases = BTreeMap::from([("rs-x-01".to_string(), &c)]);
        let q = Arm::Q("high".into());
        let o = ArmOutput {
            arm: q.clone(),
            case_id: "rs-x-01".into(),
            repeat: 1,
            status: Status::Completed,
            findings: vec![
                raw(&q, 0, "add subtracts"),
                raw(&q, 1, "add uses minus"),
                raw(&q, 2, "style nit"),
                raw(&q, 3, "old bug"),
                raw(&q, 4, "real other bug"),
            ],
        };
        let ds = final_decisions(
            &[
                dec("add subtracts", Class::Match, Some("D1"), "judge"),
                dec("add uses minus", Class::Match, Some("D1"), "judge"),
                dec("style nit", Class::FalseAlarm, None, "judge"),
                dec("old bug", Class::PreExisting, None, "judge"),
                dec("real other bug", Class::Unresolved, None, "judge"),
            ],
            &[dec("real other bug", Class::UnexpectedTp, None, "human")],
        );
        let s = score(&cases, &[o], &ds).unwrap();
        assert_eq!((s.obs[0].tp, s.obs[0].fp, s.obs[0].fn_), (1, 1, 0));
        let side = &s.side["Q-high"];
        assert_eq!(
            (side.duplicates, side.pre_existing, side.unexpected_tp),
            (1, 1, 1)
        );
        assert!(s.hits[0].found && s.hits[0].high);
    }

    /// [S1a-ADJ-2] (proposed): an approval is neither TP nor FP.
    #[test]
    fn a_no_issue_finding_is_not_a_false_alarm() {
        let (_td, c) = case();
        let cases = BTreeMap::from([("rs-x-01".to_string(), &c)]);
        let q = Arm::Q("high".into());
        let o = ArmOutput {
            arm: q.clone(),
            case_id: "rs-x-01".into(),
            repeat: 1,
            status: Status::Completed,
            findings: vec![raw(&q, 0, "The change looks correct")],
        };
        let ds = final_decisions(
            &[dec(
                "The change looks correct",
                Class::NoIssue,
                None,
                "judge",
            )],
            &[],
        );
        let s = score(&cases, &[o], &ds).unwrap();
        assert_eq!((s.obs[0].tp, s.obs[0].fp, s.obs[0].fn_), (0, 0, 1));
        assert_eq!(s.side["Q-high"].no_issue, 1);
    }

    #[test]
    fn an_error_review_misses_every_defect() {
        let (_td, c) = case();
        let cases = BTreeMap::from([("rs-x-01".to_string(), &c)]);
        let o = ArmOutput {
            arm: Arm::EUnion,
            case_id: "rs-x-01".into(),
            repeat: 2,
            status: Status::Error("timeout".into()),
            findings: vec![],
        };
        let s = score(&cases, &[o], &BTreeMap::new()).unwrap();
        assert_eq!((s.obs[0].tp, s.obs[0].fp, s.obs[0].fn_), (0, 0, 1));
        assert_eq!(s.side["E-union"].errors, 1);
    }

    #[test]
    fn negative_control_scoring_refuses_open_decisions() {
        let (_td, c) = case();
        let cases = BTreeMap::from([("rs-x-01".to_string(), &c)]);
        let q = Arm::Q("high".into());
        let o = ArmOutput {
            arm: q.clone(),
            case_id: "rs-x-01".into(),
            repeat: 1,
            status: Status::Completed,
            findings: vec![raw(&q, 0, "a"), raw(&q, 1, "b")],
        };
        let ds = final_decisions(&[dec("a", Class::Unresolved, None, "judge")], &[]);
        let e = score(&cases, &[o], &ds).unwrap_err();
        assert_eq!(e.len(), 2, "{e:?}");
    }
}
