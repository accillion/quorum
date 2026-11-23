//! Arms, raw findings and the one stripped format (AC 252).
//!
//! A [`RawFinding`] keeps its provenance (arm, repeat, seat) for scoring.
//! A [`StrippedFinding`] is what the judge and the human see: the title cut
//! to the shared word cap, `severity = "unrated"` and an empty body. No
//! arm, model or agreement survives the stripping.
//!
//! Severity is uniform on purpose: Quorum's severity is a function of
//! agreement (divergence is `high`, strong agreement `medium`), so showing
//! it would reveal agreement; no arm has a model-assigned severity until
//! Lippa's `M-ConsensusFindings`. Quorum's body is likewise only its
//! confidence and supporting models. (Recorded in `bench/DESIGN.md`.)

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::claims::cap_words;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Arm {
    Q(String),
    S(String),
    EUnion,
    EMajority,
}

impl fmt::Display for Arm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Arm::Q(t) => write!(f, "Q-{t}"),
            Arm::S(m) => write!(f, "S-{m}"),
            Arm::EUnion => f.write_str("E-union"),
            Arm::EMajority => f.write_str("E-majority"),
        }
    }
}

impl FromStr for Arm {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "E-union" => Ok(Arm::EUnion),
            "E-majority" => Ok(Arm::EMajority),
            _ => {
                if let Some(t) = s.strip_prefix("Q-") {
                    if matches!(t, "high" | "medium" | "low") {
                        return Ok(Arm::Q(t.into()));
                    }
                } else if let Some(m) = s.strip_prefix("S-") {
                    if !m.is_empty() {
                        return Ok(Arm::S(m.into()));
                    }
                }
                Err(format!("unknown arm `{s}`"))
            }
        }
    }
}

impl Serialize for Arm {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Arm {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// One finding as an arm produced it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawFinding {
    pub arm: Arm,
    pub case_id: String,
    /// 1-based repeat.
    pub repeat: u32,
    /// The seat (model id) for `S-` findings and Round 0 extractions.
    #[serde(default)]
    pub seat: Option<String>,
    /// Position within the arm's output for this case and repeat.
    pub ordinal: usize,
    pub title: String,
}

/// The only shape the judge and the human ever see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrippedFinding {
    pub title: String,
    pub severity: String,
    pub body: String,
}

pub const UNRATED: &str = "unrated";

pub fn strip(title: &str) -> StrippedFinding {
    StrippedFinding {
        title: cap_words(title),
        severity: UNRATED.into(),
        body: String::new(),
    }
}

/// The de-duplication key (Gate A ruling 7): identical stripped findings
/// for the same case share one decision.
pub fn finding_key(case_id: &str, stripped: &StrippedFinding) -> String {
    let mut h = Sha256::new();
    h.update(case_id.as_bytes());
    h.update([0u8]);
    h.update(stripped.title.as_bytes());
    h.update([0u8]);
    h.update(stripped.severity.as_bytes());
    h.update([0u8]);
    h.update(stripped.body.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Findings from a Quorum archive (`.quorum/reviews/*.json`): every active
/// finding's title, in archive order. Dismissed findings are not in
/// `findings`; the bench repos have no dismissals.
pub fn from_archive(
    arm: &Arm,
    case_id: &str,
    repeat: u32,
    archive: &serde_json::Value,
) -> Vec<RawFinding> {
    archive
        .get("findings")
        .and_then(|f| f.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|f| f.get("title").and_then(|t| t.as_str()))
                .enumerate()
                .map(|(i, t)| RawFinding {
                    arm: arm.clone(),
                    case_id: case_id.into(),
                    repeat,
                    seat: None,
                    ordinal: i,
                    title: t.to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm_names_round_trip() {
        for s in [
            "Q-high",
            "Q-medium",
            "Q-low",
            "S-gpt-6.2-sol",
            "E-union",
            "E-majority",
        ] {
            let a: Arm = s.parse().unwrap();
            assert_eq!(a.to_string(), s);
            let j = serde_json::to_string(&a).unwrap();
            assert_eq!(serde_json::from_str::<Arm>(&j).unwrap(), a);
        }
        assert!("Q-ultra".parse::<Arm>().is_err());
        assert!("S-".parse::<Arm>().is_err());
    }

    #[test]
    fn stripping_removes_everything_but_a_capped_title() {
        let archive = serde_json::json!({"findings": [{
            "title": "Null dereference when config is missing in loader path today and again and again and again and again and more",
            "body": "Confidence: 0.90. Supported by: openai, claude.",
            "severity": "high", "source": "divergence", "supported_by": ["openai", "claude"], "confidence": 0.9
        }]});
        let raw = from_archive(&Arm::Q("high".into()), "c1", 1, &archive);
        let s = strip(&raw[0].title);
        let json = serde_json::to_string(&s).unwrap();
        for leak in ["openai", "claude", "divergence", "0.9", "high", "Supported"] {
            assert!(
                !json.contains(leak),
                "stripped finding leaks `{leak}`: {json}"
            );
        }
        assert_eq!(s.title.split(' ').count(), crate::claims::MAX_WORDS);
    }

    #[test]
    fn finding_key_is_per_case_and_whitespace_insensitive() {
        let a = strip("Off by  one");
        let b = strip("Off by one ");
        assert_eq!(finding_key("c1", &a), finding_key("c1", &b));
        assert_ne!(finding_key("c1", &a), finding_key("c2", &a));
    }
}
