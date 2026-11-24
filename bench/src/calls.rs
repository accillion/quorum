//! One paid direct call under the budget (AC 250): reserve the worst case,
//! call, retry at most once, settle. Every call is logged for the manifest.

use serde::{Deserialize, Serialize};

use crate::budget::{Budget, BudgetError, Settle, Unit};
use crate::prices::PriceTable;
use crate::vendors::{CallError, ModelRequest, ModelResponse, Provider};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CallLog {
    pub purpose: String,
    pub model: String,
    pub attempts: u32,
    pub reserved_micros: u64,
    pub charged_micros: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub model_returned: Option<String>,
    pub fingerprint: Option<String>,
    pub latency_ms: u64,
    pub error: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum StepError {
    #[error(transparent)]
    Budget(#[from] BudgetError),
    #[error("price: {0}")]
    Price(#[from] crate::prices::PriceError),
    #[error("call failed after its retry: {0}")]
    Failed(CallError),
    #[error("invalid model output: {0}")]
    Invalid(String),
}

impl StepError {
    /// A budget stop ends the run; everything else fails one step.
    pub fn is_budget_stop(&self) -> bool {
        matches!(self, StepError::Budget(_))
    }
}

/// Reserve, call, retry once on a retryable failure, settle. The
/// reservation is the worst case: every prompt byte a token, plus the full
/// output cap, at list price.
pub async fn paid_call(
    budget: &mut Budget,
    prices: &PriceTable,
    provider: &Provider,
    http: &reqwest::Client,
    req: &ModelRequest,
    purpose: &str,
    log: &mut Vec<CallLog>,
) -> Result<ModelResponse, StepError> {
    let bytes = req.system.len() + req.user.len();
    let worst = prices.worst_case_micros(&req.model, bytes, req.max_output_tokens)?;
    let mut entry = CallLog {
        purpose: purpose.into(),
        model: req.model.clone(),
        attempts: 0,
        reserved_micros: 0,
        charged_micros: 0,
        input_tokens: 0,
        output_tokens: 0,
        model_returned: None,
        fingerprint: None,
        latency_ms: 0,
        error: None,
    };
    let mut last_err = None;
    for attempt in 0..2 {
        let r = match budget.reserve(Unit::MicroUsd, worst) {
            Ok(r) => r,
            Err(e) => {
                entry.error = Some(e.to_string());
                log.push(entry);
                return Err(e.into());
            }
        };
        entry.attempts += 1;
        entry.reserved_micros += worst;
        match provider.call(http, req).await {
            Ok(resp) => {
                let cost = prices.cost_micros(&req.model, resp.input_tokens, resp.output_tokens)?;
                entry.charged_micros += budget.settle(r, Settle::Billed(cost))?;
                entry.input_tokens += resp.input_tokens;
                entry.output_tokens += resp.output_tokens;
                entry.model_returned = resp.model_returned.clone();
                entry.fingerprint = resp.fingerprint.clone();
                entry.latency_ms = resp.latency_ms;
                log.push(entry);
                return Ok(resp);
            }
            Err(e) => {
                entry.charged_micros += budget.settle(r, e.settle())?;
                let retry = attempt == 0 && e.retryable();
                last_err = Some(e);
                if !retry {
                    break;
                }
            }
        }
    }
    let e = last_err.expect("at least one attempt");
    entry.error = Some(e.to_string());
    log.push(entry);
    Err(StepError::Failed(e))
}

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    use super::*;

    pub const PRICES: &str = r#"
read_on = "2026-11-23"
[models.m]
vendor = "mock"
input = 1.0
output = 1.0
source = "test"
"#;

    pub fn prices() -> PriceTable {
        PriceTable::parse(PRICES).unwrap()
    }

    pub fn ok(text: &str) -> ModelResponse {
        ModelResponse {
            text: text.into(),
            input_tokens: 10,
            output_tokens: 5,
            model_returned: Some("m-snapshot".into()),
            fingerprint: Some("fp".into()),
            stop_reason: None,
            latency_ms: 1,
        }
    }

    /// A mock that fails `fail_first` times with `err`, then answers `text`.
    pub fn flaky(
        fail_first: u32,
        err: CallError,
        text: &'static str,
    ) -> (Provider, Arc<AtomicU32>) {
        let n = Arc::new(AtomicU32::new(0));
        let n2 = n.clone();
        let p = Provider::Mock(Arc::new(move |_r: &ModelRequest| {
            let i = n2.fetch_add(1, Ordering::SeqCst);
            if i < fail_first {
                Err(err.clone())
            } else {
                Ok(ok(text))
            }
        }));
        (p, n)
    }

    pub fn req() -> ModelRequest {
        ModelRequest {
            model: "m".into(),
            system: "s".repeat(100),
            user: "u".repeat(100),
            max_output_tokens: 100,
            effort: None,
            json_schema: None,
            seed: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    #[tokio::test]
    async fn success_settles_the_billed_cost() {
        let (p, _) = flaky(0, CallError::Parse(String::new()), "ok");
        let mut b = Budget::new(0, 10_000);
        let mut log = Vec::new();
        paid_call(
            &mut b,
            &prices(),
            &p,
            &reqwest::Client::new(),
            &req(),
            "t",
            &mut log,
        )
        .await
        .unwrap();
        assert_eq!(b.snapshot().usd_spent_micros, 15);
        assert_eq!(log[0].reserved_micros, 200 + 256 + 100);
        assert_eq!(log[0].model_returned.as_deref(), Some("m-snapshot"));
    }

    #[tokio::test]
    async fn one_retry_then_uncertain_failures_count_as_spent() {
        let (p, n) = flaky(5, CallError::Transport("reset".into()), "x");
        let mut b = Budget::new(0, 10_000);
        let mut log = Vec::new();
        let e = paid_call(
            &mut b,
            &prices(),
            &p,
            &reqwest::Client::new(),
            &req(),
            "t",
            &mut log,
        )
        .await
        .unwrap_err();
        assert!(matches!(e, StepError::Failed(_)));
        assert_eq!(
            n.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "at most one retry"
        );
        assert_eq!(
            b.snapshot().usd_spent_micros,
            2 * 556,
            "uncertain billing counts the reservation"
        );
    }

    #[tokio::test]
    async fn a_rejected_request_is_not_retried_or_charged() {
        let (p, n) = flaky(
            5,
            CallError::Http {
                status: 400,
                excerpt: String::new(),
            },
            "x",
        );
        let mut b = Budget::new(0, 10_000);
        let mut log = Vec::new();
        assert!(paid_call(
            &mut b,
            &prices(),
            &p,
            &reqwest::Client::new(),
            &req(),
            "t",
            &mut log
        )
        .await
        .is_err());
        assert_eq!(n.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(b.snapshot().usd_spent_micros, 0);
    }

    #[tokio::test]
    async fn negative_control_no_call_without_budget() {
        let (p, n) = flaky(0, CallError::Parse(String::new()), "ok");
        let mut b = Budget::new(0, 555);
        let mut log = Vec::new();
        let e = paid_call(
            &mut b,
            &prices(),
            &p,
            &reqwest::Client::new(),
            &req(),
            "t",
            &mut log,
        )
        .await
        .unwrap_err();
        assert!(e.is_budget_stop());
        assert_eq!(
            n.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the model is never called"
        );
    }
}
