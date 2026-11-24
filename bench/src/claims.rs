//! Lippa's claims format, parsed the way the `S-` arms need it, and the
//! word and count caps every arm's findings share.
//!
//! A Round 0 seat ends its answer with
//!
//! ```text
//! ---CLAIMS---
//! { "verdict": "...", "claims": [{"text": "...", "confidence": 0.85}], ... }
//! ---END CLAIMS---
//! ```
//!
//! sometimes inside a code fence or with bold markers. The `S-` prompt
//! (`bench/prompts/single-v1.md`) asks for the same block.

use serde::{Deserialize, Serialize};

/// Lippa asks every seat for "exactly 3-5 claims maximum".
pub const MAX_CLAIMS_PER_SEAT: usize = 5;
/// Lippa asks for claims "under 20 words". Every arm's finding titles are
/// cut to this many words in the stripped format, so length favours no arm.
pub const MAX_WORDS: usize = 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    pub text: String,
    #[serde(default)]
    pub confidence: Option<f64>,
}

#[derive(Deserialize)]
struct Block {
    #[serde(default)]
    claims: Vec<serde_json::Value>,
}

/// The end of the JSON object that starts at `open` (a `{`), honouring
/// strings and escapes. Returns the index one past the closing brace.
fn object_end(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_str = false;
    let mut i = open;
    while i < bytes.len() {
        let b = bytes[i];
        if in_str {
            match b {
                b'\\' => i += 1,
                b'"' => in_str = false,
                _ => {}
            }
        } else {
            match b {
                b'"' => in_str = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// The claims in a response, or `None` when it has no parseable claims
/// block (the caller then falls back to the extraction rubric, as Lippa
/// falls back to its claim extractor).
pub fn parse_claims(text: &str) -> Option<Vec<Claim>> {
    // The start marker; `---END CLAIMS---` does not contain it.
    let marker = text.rfind("---CLAIMS---")?;
    let bytes = text.as_bytes();
    let open = text[marker..].find('{')? + marker;
    let end = object_end(bytes, open)?;
    let block: Block = serde_json::from_str(&text[open..end]).ok()?;
    let mut out = Vec::new();
    for c in block.claims {
        let claim = match c {
            serde_json::Value::String(s) => Claim {
                text: s,
                confidence: None,
            },
            serde_json::Value::Object(o) => Claim {
                text: o
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                confidence: o.get("confidence").and_then(|v| v.as_f64()),
            },
            _ => continue,
        };
        if !claim.text.trim().is_empty() {
            out.push(claim);
        }
    }
    Some(out)
}

/// Collapse whitespace and keep at most [`MAX_WORDS`] words.
pub fn cap_words(text: &str) -> String {
    text.split_whitespace()
        .take(MAX_WORDS)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The first [`MAX_CLAIMS_PER_SEAT`] claims.
pub fn cap_claims(mut claims: Vec<Claim>) -> Vec<Claim> {
    claims.truncate(MAX_CLAIMS_PER_SEAT);
    claims
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_block() {
        let t = "RECOMMENDATION: block\n\nprose\n---CLAIMS---\n{ \"verdict\": \"oppose\",\n  \"claims\": [{\"text\": \"Off-by-one in loop bound\", \"confidence\": 0.9}, {\"text\": \"Missing } brace in \\\"quote\\\"\"}],\n  \"assumptions\": [] }\n---END CLAIMS---\n";
        let c = parse_claims(t).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].text, "Off-by-one in loop bound");
        assert_eq!(c[0].confidence, Some(0.9));
        assert_eq!(c[1].text, "Missing } brace in \"quote\"");
    }

    #[test]
    fn fenced_and_bold_blocks() {
        let t =
            "x\n**---CLAIMS---**\n```json\n{\"claims\": [\"a b c\"]}\n```\n**---END CLAIMS---**";
        assert_eq!(parse_claims(t).unwrap()[0].text, "a b c");
    }

    #[test]
    fn missing_or_broken_block_is_none() {
        assert!(parse_claims("no block here").is_none());
        assert!(parse_claims("---CLAIMS---\n{\"claims\": [").is_none());
    }

    #[test]
    fn caps() {
        let long = (1..=30)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join("  ");
        let capped = cap_words(&long);
        assert_eq!(capped.split(' ').count(), MAX_WORDS);
        assert!(capped.starts_with("w1 w2"));
        let many: Vec<Claim> = (0..8)
            .map(|i| Claim {
                text: i.to_string(),
                confidence: None,
            })
            .collect();
        assert_eq!(cap_claims(many).len(), MAX_CLAIMS_PER_SEAT);
    }
}
