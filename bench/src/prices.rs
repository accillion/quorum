//! The pinned price table (AC 250, AC 251). `bench/prices.toml` is part of
//! the frozen design; its SHA-256 goes in every run manifest.
//!
//! Prices are USD per million tokens, so a price is also micro-USD per
//! token, and every cost here is an integer number of micro-USD, rounded
//! up.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct PriceTable {
    /// The date the prices were read from the vendors' pages.
    pub read_on: String,
    pub models: BTreeMap<String, ModelPrice>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelPrice {
    pub vendor: String,
    /// USD per 1M input tokens.
    pub input: f64,
    /// USD per 1M output tokens (reasoning and thinking tokens included).
    pub output: f64,
    /// Above this many input tokens the long-context prices apply to the
    /// whole call.
    #[serde(default)]
    pub long_context_threshold: Option<u64>,
    #[serde(default)]
    pub long_input: Option<f64>,
    #[serde(default)]
    pub long_output: Option<f64>,
    pub source: String,
}

#[derive(thiserror::Error, Debug)]
pub enum PriceError {
    #[error("price table: {0}")]
    Parse(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("no price for model `{0}`: a model without a pinned price cannot be called")]
    Unknown(String),
}

impl PriceTable {
    pub fn parse(text: &str) -> Result<Self, PriceError> {
        let t: PriceTable = toml::from_str(text).map_err(|e| PriceError::Parse(e.to_string()))?;
        for (id, p) in &t.models {
            let ok = p.input >= 0.0
                && p.output >= 0.0
                && (p.long_context_threshold.is_some()
                    == (p.long_input.is_some() && p.long_output.is_some()));
            if !ok {
                return Err(PriceError::Parse(format!(
                    "model `{id}`: negative price, or long-context threshold without both long prices"
                )));
            }
        }
        Ok(t)
    }

    pub fn load(path: &Path) -> Result<Self, PriceError> {
        Self::parse(&std::fs::read_to_string(path)?)
    }

    pub fn get(&self, model: &str) -> Result<&ModelPrice, PriceError> {
        self.models
            .get(model)
            .ok_or_else(|| PriceError::Unknown(model.into()))
    }

    /// Cost of a call in micro-USD at list price, rounded up.
    pub fn cost_micros(
        &self,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Result<u64, PriceError> {
        let p = self.get(model)?;
        let (pin, pout) = match (p.long_context_threshold, p.long_input, p.long_output) {
            (Some(th), Some(li), Some(lo)) if input_tokens > th => (li, lo),
            _ => (p.input, p.output),
        };
        let micros = input_tokens as f64 * pin + output_tokens as f64 * pout;
        Ok(micros.ceil() as u64)
    }

    /// The reservation before a call (AC 250): the worst-case input tokens
    /// plus the full output cap, at list price.
    pub fn worst_case_micros(
        &self,
        model: &str,
        input_bytes: usize,
        output_cap: u64,
    ) -> Result<u64, PriceError> {
        self.cost_micros(model, worst_case_input_tokens(input_bytes), output_cap)
    }
}

/// An upper bound on input tokens for `bytes` of prompt text. A BPE token
/// is at least one byte, so the byte count bounds the token count; 256
/// tokens are added for message framing.
pub fn worst_case_input_tokens(bytes: usize) -> u64 {
    bytes as u64 + 256
}

pub fn format_usd(micros: u64) -> String {
    format!("${}.{:06}", micros / 1_000_000, micros % 1_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = r#"
read_on = "2026-11-23"
[models.m]
vendor = "v"
input = 2.0
output = 10.0
long_context_threshold = 1000
long_input = 4.0
long_output = 15.0
source = "https://example.invalid/pricing"
[models.flat]
vendor = "v"
input = 0.7
output = 2.1
source = "https://example.invalid/pricing"
"#;

    #[test]
    fn costs_round_up_in_micro_usd() {
        let t = PriceTable::parse(T).unwrap();
        // 1,000 in at $2/M = 2,000 µ$; 100 out at $10/M = 1,000 µ$.
        assert_eq!(t.cost_micros("m", 1000, 100).unwrap(), 3000);
        // 3 tokens at $0.7/M = 2.1 µ$ -> 3.
        assert_eq!(t.cost_micros("flat", 3, 0).unwrap(), 3);
    }

    #[test]
    fn long_context_prices_apply_to_the_whole_call() {
        let t = PriceTable::parse(T).unwrap();
        assert_eq!(t.cost_micros("m", 1001, 0).unwrap(), 4004);
    }

    #[test]
    fn worst_case_uses_the_byte_bound_and_full_cap() {
        let t = PriceTable::parse(T).unwrap();
        let w = t.worst_case_micros("flat", 1000, 16_384).unwrap();
        assert_eq!(w, t.cost_micros("flat", 1256, 16_384).unwrap());
    }

    #[test]
    fn unknown_model_is_an_error_not_free() {
        let t = PriceTable::parse(T).unwrap();
        assert!(matches!(
            t.cost_micros("nope", 1, 1),
            Err(PriceError::Unknown(_))
        ));
    }

    #[test]
    fn half_specified_long_context_is_rejected() {
        let bad = T.replace("long_output = 15.0\n", "");
        assert!(PriceTable::parse(&bad).is_err());
    }

    #[test]
    fn the_pinned_table_parses_and_prices_every_model() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("prices.toml");
        let t = PriceTable::load(&path).unwrap();
        assert!(!t.models.is_empty());
        for id in t.models.keys() {
            assert!(t.cost_micros(id, 10_000, 1_000).unwrap() > 0, "{id}");
        }
    }

    #[test]
    fn usd_formatting() {
        assert_eq!(format_usd(1_234_567), "$1.234567");
        assert_eq!(format_usd(5), "$0.000005");
    }
}
