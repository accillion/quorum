//! The blind human check (AC 252, AC 248a, [S1a-ADJ]).
//!
//! The queue holds three kinds of item:
//! - **finding**: one stripped finding (title, severity, body) with its
//!   candidate defect: every uncertain or unresolved judge proposal, every
//!   unexpected true positive, and a seeded random share (20%) of the rest;
//! - **clustering**: one case-repeat's grouping, as groups of stripped
//!   titles: a seeded random 20% of clusterings;
//! - **extraction**: one Round 0 answer with the claims extracted from it:
//!   a seeded random 20% of extractions.
//!
//! Nothing shown, and nothing in the queue file, names an arm, model,
//! seat or agreement. Findings are already unique per case (identical
//! stripped findings share one decision, Gate A ruling 7) and the copy
//! count is never shown. Items are presented in a seeded shuffle.
//! Decisions are appended to `adjudication/decisions.jsonl`, so a session
//! can stop and resume.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, Write};
use std::path::Path;

use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::SeedableRng;
use serde::{Deserialize, Serialize};

use crate::corpus::Case;
use crate::findings::{finding_key, strip, StrippedFinding};
use crate::pipeline::{sub_seed, Class, Clustering, Decision, Extraction};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DefectView {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Item {
    Finding {
        id: String,
        case_id: String,
        key: String,
        finding: StrippedFinding,
        candidate: Option<DefectView>,
        defects: Vec<DefectView>,
        why: String,
    },
    Clustering {
        id: String,
        case_id: String,
        repeat: u32,
        groups: Vec<Vec<String>>,
    },
    Extraction {
        id: String,
        case_id: String,
        answer: String,
        claims: Vec<String>,
    },
}

impl Item {
    pub fn id(&self) -> &str {
        match self {
            Item::Finding { id, .. }
            | Item::Clustering { id, .. }
            | Item::Extraction { id, .. } => id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HumanDecision {
    pub item: String,
    /// finding: `match:<defect>`, `false_alarm`, `pre_existing`,
    /// `unexpected_tp`; clustering / extraction: `ok`, `wrong`.
    pub decision: String,
    #[serde(default)]
    pub note: String,
}

/// Inputs for one queue: the judge's decisions and the stored clustering
/// and extraction decisions, each extraction with the answer it read.
pub struct QueueInputs<'a> {
    pub cases: &'a [Case],
    pub decisions: &'a [Decision],
    pub clusterings: &'a [(Clustering, Vec<String>)],
    pub extractions: &'a [(Extraction, String)],
    pub share: f64,
    pub seed: u64,
}

fn sample<T: Clone>(mut v: Vec<T>, share: f64, seed: u64) -> Vec<T> {
    v.shuffle(&mut StdRng::seed_from_u64(seed));
    let n = (v.len() as f64 * share).ceil() as usize;
    v.truncate(n);
    v
}

pub fn build_queue(q: &QueueInputs<'_>) -> Vec<Item> {
    let by_id: BTreeMap<&str, &Case> = q.cases.iter().map(|c| (c.id(), c)).collect();
    let views = |c: &Case| {
        c.defects()
            .iter()
            .map(|d| DefectView {
                id: d.id.clone(),
                title: d.title.clone(),
            })
            .collect::<Vec<_>>()
    };
    let mut must = Vec::new();
    let mut rest = Vec::new();
    for d in q.decisions {
        let why = if d.uncertain || d.class == Class::Unresolved {
            "uncertain"
        } else if d.class == Class::UnexpectedTp {
            "unexpected_tp"
        } else {
            ""
        };
        if why.is_empty() {
            rest.push((d, "sample"));
        } else {
            must.push((d, why));
        }
    }
    let mut chosen = must;
    chosen.extend(sample(
        rest,
        q.share,
        sub_seed(q.seed, &["adjudicate", "findings"]),
    ));
    let mut items = Vec::new();
    for (d, why) in chosen {
        let Some(case) = by_id.get(d.case_id.as_str()) else {
            continue;
        };
        let defects = views(case);
        let candidate = d
            .defect
            .as_ref()
            .and_then(|x| defects.iter().find(|v| &v.id == x).cloned());
        // The stripped finding is re-derived from the key's source title by
        // the caller; here it is carried in the decision's case scope.
        items.push(Item::Finding {
            id: String::new(),
            case_id: d.case_id.clone(),
            key: d.key.clone(),
            finding: StrippedFinding {
                title: String::new(),
                severity: crate::findings::UNRATED.into(),
                body: String::new(),
            },
            candidate,
            defects,
            why: why.into(),
        });
    }
    for (c, titles) in sample(
        q.clusterings.to_vec(),
        q.share,
        sub_seed(q.seed, &["adjudicate", "clusterings"]),
    ) {
        let groups = c
            .clusters
            .iter()
            .map(|m| m.iter().map(|&i| titles[i].clone()).collect())
            .collect();
        items.push(Item::Clustering {
            id: String::new(),
            case_id: c.case_id.clone(),
            repeat: c.repeat,
            groups,
        });
    }
    for (x, answer) in sample(
        q.extractions.to_vec(),
        q.share,
        sub_seed(q.seed, &["adjudicate", "extractions"]),
    ) {
        items.push(Item::Extraction {
            id: String::new(),
            case_id: x.case_id.clone(),
            answer,
            claims: x.claims.clone(),
        });
    }
    items.shuffle(&mut StdRng::seed_from_u64(sub_seed(
        q.seed,
        &["adjudicate", "order"],
    )));
    for (n, it) in items.iter_mut().enumerate() {
        let new = format!("item-{:04}", n + 1);
        match it {
            Item::Finding { id, .. }
            | Item::Clustering { id, .. }
            | Item::Extraction { id, .. } => *id = new,
        }
    }
    items
}

/// Fill each finding item's stripped finding from the titles that produced
/// its key (any copy: they are identical by construction).
pub fn attach_titles(items: &mut [Item], titles: &BTreeMap<(String, String), StrippedFinding>) {
    for it in items {
        if let Item::Finding {
            case_id,
            key,
            finding,
            ..
        } = it
        {
            if let Some(s) = titles.get(&(case_id.clone(), key.clone())) {
                *finding = s.clone();
            }
        }
    }
}

/// Every (case, key) to its stripped finding, from all arms' raw titles.
pub fn titles_by_key(
    outputs: &[crate::score::ArmOutput],
) -> BTreeMap<(String, String), StrippedFinding> {
    let mut m = BTreeMap::new();
    for o in outputs {
        for f in &o.findings {
            let s = strip(&f.title);
            m.entry((o.case_id.clone(), finding_key(&o.case_id, &s)))
                .or_insert(s);
        }
    }
    m
}

pub fn queue_path(run_dir: &Path) -> std::path::PathBuf {
    run_dir.join("adjudication").join("queue.json")
}

pub fn decisions_path(run_dir: &Path) -> std::path::PathBuf {
    run_dir.join("adjudication").join("decisions.jsonl")
}

pub fn load_queue(run_dir: &Path) -> std::io::Result<Vec<Item>> {
    let t = std::fs::read_to_string(queue_path(run_dir))?;
    serde_json::from_str(&t).map_err(std::io::Error::other)
}

pub fn load_human(run_dir: &Path) -> std::io::Result<Vec<HumanDecision>> {
    match std::fs::read_to_string(decisions_path(run_dir)) {
        Ok(t) => t
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).map_err(std::io::Error::other))
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Human finding decisions as [`Decision`]s that replace the judge's.
pub fn human_decisions(run_dir: &Path) -> std::io::Result<Vec<Decision>> {
    let queue = match load_queue(run_dir) {
        Ok(q) => q,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let items: BTreeMap<&str, &Item> = queue.iter().map(|i| (i.id(), i)).collect();
    let mut out = Vec::new();
    for h in load_human(run_dir)? {
        let Some(Item::Finding { case_id, key, .. }) = items.get(h.item.as_str()) else {
            continue;
        };
        let (class, defect) = match h.decision.split_once(':') {
            Some(("match", d)) => (Class::Match, Some(d.to_string())),
            _ => match h.decision.as_str() {
                "false_alarm" => (Class::FalseAlarm, None),
                "pre_existing" => (Class::PreExisting, None),
                "unexpected_tp" => (Class::UnexpectedTp, None),
                _ => continue,
            },
        };
        out.push(Decision {
            case_id: case_id.clone(),
            key: key.clone(),
            class,
            defect,
            uncertain: false,
            reason: h.note.clone(),
            by: "human".into(),
            model_returned: None,
        });
    }
    Ok(out)
}

fn show(it: &Item, out: &mut dyn Write) -> std::io::Result<()> {
    writeln!(out, "\n──── {} ────", it.id())?;
    match it {
        Item::Finding {
            finding,
            candidate,
            defects,
            ..
        } => {
            writeln!(
                out,
                "FINDING\n  title:    {}\n  severity: {}\n  body:     {}",
                finding.title,
                finding.severity,
                if finding.body.is_empty() {
                    "(none)"
                } else {
                    &finding.body
                }
            )?;
            match candidate {
                Some(c) => writeln!(out, "CANDIDATE DEFECT\n  {}: {}", c.id, c.title)?,
                None => writeln!(out, "CANDIDATE DEFECT\n  (none proposed)")?,
            }
            if defects.is_empty() {
                writeln!(out, "EXPECTED DEFECTS\n  (none: a clean case)")?;
            } else {
                writeln!(out, "EXPECTED DEFECTS")?;
                for (n, d) in defects.iter().enumerate() {
                    writeln!(out, "  d{}  {}: {}", n + 1, d.id, d.title)?;
                }
            }
            writeln!(out, "[m] matches the candidate  [d<N>] matches defect N  [f] false alarm  [p] pre-existing issue  [u] unexpected true positive  [s] skip  [q] quit")?;
        }
        Item::Clustering { groups, .. } => {
            writeln!(out, "CLUSTERING: is each group one underlying concern, and are different groups different concerns?")?;
            for (n, g) in groups.iter().enumerate() {
                writeln!(out, "  group {}:", n + 1)?;
                for t in g {
                    writeln!(out, "    - {t}")?;
                }
            }
            writeln!(out, "[y] correct  [n] wrong  [s] skip  [q] quit")?;
        }
        Item::Extraction { answer, claims, .. } => {
            writeln!(out, "ANSWER\n{answer}\nEXTRACTED CLAIMS")?;
            for c in claims {
                writeln!(out, "  - {c}")?;
            }
            writeln!(out, "Are the claims a faithful summary of the answer's review findings?\n[y] yes  [n] no  [s] skip  [q] quit")?;
        }
    }
    write!(out, "> ")?;
    out.flush()
}

fn parse(it: &Item, line: &str) -> Option<Result<String, ()>> {
    let l = line.trim();
    if l == "q" {
        return None;
    }
    let d = match (it, l) {
        (_, "s") => return Some(Err(())),
        (
            Item::Finding {
                candidate: Some(c), ..
            },
            "m",
        ) => format!("match:{}", c.id),
        (Item::Finding { defects, .. }, x) if x.starts_with('d') => {
            let picked = x[1..]
                .parse::<usize>()
                .ok()
                .and_then(|n| n.checked_sub(1))
                .and_then(|i| defects.get(i));
            match picked {
                Some(d) => format!("match:{}", d.id),
                None => return Some(Err(())),
            }
        }
        (Item::Finding { .. }, "f") => "false_alarm".into(),
        (Item::Finding { .. }, "p") => "pre_existing".into(),
        (Item::Finding { .. }, "u") => "unexpected_tp".into(),
        (Item::Clustering { .. } | Item::Extraction { .. }, "y") => "ok".into(),
        (Item::Clustering { .. } | Item::Extraction { .. }, "n") => "wrong".into(),
        _ => return Some(Err(())),
    };
    Some(Ok(d))
}

/// The interactive loop: one undecided item at a time. Returns how many
/// items remain undecided.
pub fn run_session(
    run_dir: &Path,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> std::io::Result<usize> {
    let queue = load_queue(run_dir)?;
    let done: BTreeSet<String> = load_human(run_dir)?.into_iter().map(|h| h.item).collect();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(decisions_path(run_dir))?;
    let mut remaining = queue.iter().filter(|i| !done.contains(i.id())).count();
    writeln!(
        out,
        "{remaining} item(s) to decide. Arms, models and agreement are hidden."
    )?;
    for it in queue.iter().filter(|i| !done.contains(i.id())) {
        loop {
            show(it, out)?;
            let mut line = String::new();
            if input.read_line(&mut line)? == 0 {
                return Ok(remaining);
            }
            match parse(it, &line) {
                None => return Ok(remaining),
                Some(Err(())) if line.trim() == "s" => break,
                Some(Err(())) => writeln!(out, "not an option here")?,
                Some(Ok(decision)) => {
                    let mut note = String::new();
                    if decision == "wrong" {
                        write!(out, "what is wrong? > ")?;
                        out.flush()?;
                        input.read_line(&mut note)?;
                    }
                    let h = HumanDecision {
                        item: it.id().into(),
                        decision,
                        note: note.trim().into(),
                    };
                    writeln!(
                        file,
                        "{}",
                        serde_json::to_string(&h).map_err(std::io::Error::other)?
                    )?;
                    remaining -= 1;
                    break;
                }
            }
        }
    }
    Ok(remaining)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case() -> (tempfile::TempDir, Case) {
        let td = tempfile::TempDir::new().unwrap();
        let d = crate::corpus::testcase::write_case(td.path(), "rs-x-01", "");
        (td, crate::corpus::load_case(&d).unwrap())
    }

    fn dec(i: usize, class: Class, uncertain: bool) -> Decision {
        let s = strip(&format!("finding {i}"));
        Decision {
            case_id: "rs-x-01".into(),
            key: finding_key("rs-x-01", &s),
            class,
            defect: (class == Class::Match).then(|| "D1".to_string()),
            uncertain,
            reason: String::new(),
            by: "judge".into(),
            model_returned: Some("grok-4.7".into()),
        }
    }

    fn queue_for(c: &Case, ds: &[Decision]) -> Vec<Item> {
        let x = Extraction {
            case_id: "rs-x-01".into(),
            repeat: 1,
            seat: "claude-opus-5-5".into(),
            input_sha256: String::new(),
            claims: vec!["add subtracts".into()],
            rubric_version: 1,
            rubric_sha256: String::new(),
            model_returned: None,
        };
        let cl = Clustering {
            case_id: "rs-x-01".into(),
            repeat: 1,
            clusters: vec![vec![0, 1]],
            shown_ids: vec![],
            rubric_version: 1,
            rubric_sha256: String::new(),
            model_returned: None,
        };
        build_queue(&QueueInputs {
            cases: std::slice::from_ref(c),
            decisions: ds,
            clusterings: &[(cl, vec!["a".into(), "b".into()])],
            extractions: &[(x, "The add function subtracts.".into())],
            share: 0.2,
            seed: 5,
        })
    }

    #[test]
    fn mandatory_items_and_a_seeded_twenty_percent() {
        let (_td, c) = case();
        let mut ds = vec![
            dec(0, Class::Match, true),
            dec(1, Class::UnexpectedTp, false),
            dec(2, Class::Unresolved, false),
        ];
        ds.extend((3..13).map(|i| dec(i, Class::FalseAlarm, false)));
        let q = queue_for(&c, &ds);
        let findings: Vec<&Item> = q
            .iter()
            .filter(|i| matches!(i, Item::Finding { .. }))
            .collect();
        assert_eq!(findings.len(), 3 + 2, "three mandatory + 20% of ten");
        assert_eq!(
            q.iter()
                .filter(|i| matches!(i, Item::Clustering { .. }))
                .count(),
            1,
            "ceil(20% of 1)"
        );
        assert_eq!(q, queue_for(&c, &ds), "seeded: the same queue every time");
    }

    #[test]
    fn nothing_in_the_queue_names_an_arm_model_or_agreement() {
        let (_td, c) = case();
        let q = queue_for(&c, &[dec(0, Class::Match, true)]);
        let text = serde_json::to_string(&q).unwrap().to_ascii_lowercase();
        for leak in [
            "q-high",
            "s-gpt",
            "s-claude",
            "s-gemini",
            "e-union",
            "e-majority",
            "grok",
            "claude",
            "gemini",
            "gpt",
            "seat",
            "supported_by",
            "agreement",
            "confidence",
        ] {
            assert!(!text.contains(leak), "queue leaks `{leak}`: {text}");
        }
    }

    #[test]
    fn an_interactive_session_records_decisions_and_resumes() {
        let (_td, c) = case();
        let ds = vec![
            dec(0, Class::Match, true),
            dec(1, Class::UnexpectedTp, false),
        ];
        let run = tempfile::TempDir::new().unwrap();
        let mut q = queue_for(&c, &ds);
        let mut titles = BTreeMap::new();
        for d in &ds {
            titles.insert(
                (d.case_id.clone(), d.key.clone()),
                strip(&format!(
                    "finding {}",
                    if d.class == Class::Match { 0 } else { 1 }
                )),
            );
        }
        attach_titles(&mut q, &titles);
        std::fs::create_dir_all(run.path().join("adjudication")).unwrap();
        std::fs::write(queue_path(run.path()), serde_json::to_string(&q).unwrap()).unwrap();
        // Answer only the first item, then quit.
        let first = &q[0];
        let answer = match first {
            Item::Finding { .. } => "f\n",
            _ => "y\n",
        };
        let mut out = Vec::new();
        let left = run_session(
            run.path(),
            &mut std::io::Cursor::new(format!("{answer}q\n")),
            &mut out,
        )
        .unwrap();
        assert_eq!(left, q.len() - 1);
        let shown = String::from_utf8(out).unwrap();
        assert!(!shown.contains("grok") && !shown.contains("Q-high"));
        // Resume: the decided item is not shown again.
        let mut out2 = Vec::new();
        run_session(run.path(), &mut std::io::Cursor::new("q\n"), &mut out2).unwrap();
        assert!(!String::from_utf8(out2)
            .unwrap()
            .contains(&format!("──── {} ────", first.id())));
        // Answer everything that is left with a finding-or-clustering-safe reply.
        let mut script = String::new();
        for it in &q[1..] {
            script.push_str(match it {
                Item::Finding {
                    candidate: Some(_), ..
                } => "m\n",
                Item::Finding { .. } => "u\n",
                _ => "y\n",
            });
        }
        assert_eq!(
            run_session(
                run.path(),
                &mut std::io::Cursor::new(script),
                &mut Vec::new()
            )
            .unwrap(),
            0
        );
        let human = human_decisions(run.path()).unwrap();
        assert_eq!(
            human.len(),
            q.iter()
                .filter(|i| matches!(i, Item::Finding { .. }))
                .count()
        );
        assert!(human.iter().all(|h| h.by == "human"));
    }

    #[test]
    fn invalid_keys_are_refused_per_item_kind() {
        let it = Item::Clustering {
            id: "i".into(),
            case_id: "c".into(),
            repeat: 1,
            groups: vec![],
        };
        assert_eq!(parse(&it, "f"), Some(Err(())));
        assert_eq!(parse(&it, "y"), Some(Ok("ok".into())));
        let f = Item::Finding {
            id: "i".into(),
            case_id: "c".into(),
            key: "k".into(),
            finding: strip("x"),
            candidate: None,
            defects: vec![DefectView {
                id: "D1".into(),
                title: "t".into(),
            }],
            why: String::new(),
        };
        assert_eq!(parse(&f, "m"), Some(Err(())), "no candidate to match");
        assert_eq!(parse(&f, "d1"), Some(Ok("match:D1".into())));
        assert_eq!(
            parse(&f, "d2"),
            Some(Err(())),
            "no defect 2: not an option, not quit"
        );
        assert_eq!(parse(&f, "q"), None);
    }
}
