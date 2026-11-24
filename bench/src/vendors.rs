//! Direct model calls: the `S-` arms (OpenAI, Anthropic, Google) and the
//! judge (Mistral or xAI), which also runs extraction and clustering.
//!
//! Every call goes through [`Provider::call`]. Tests use
//! [`Provider::Mock`]; no test in this crate calls a vendor. The request
//! shapes follow each vendor's API reference as read on 2026-11-23; run 1b
//! starts with one minimal call per provider to confirm them (DESIGN.md).

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::budget::Settle;
use crate::secret::Secret;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelRequest {
    pub model: String,
    pub system: String,
    pub user: String,
    pub max_output_tokens: u64,
    /// The vendor's effort / reasoning / thinking level, verbatim.
    #[serde(default)]
    pub effort: Option<String>,
    /// Strict JSON-schema output: (name, schema).
    #[serde(default)]
    pub json_schema: Option<(String, Value)>,
    /// Seed for vendors that take one (Mistral `random_seed`).
    #[serde(default)]
    pub seed: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelResponse {
    pub text: String,
    pub input_tokens: u64,
    /// Output tokens including reasoning / thinking tokens.
    pub output_tokens: u64,
    /// The model string the response reports, when it reports one.
    pub model_returned: Option<String>,
    /// `system_fingerprint` where the vendor reports one (xAI, OpenAI
    /// chat): xAI's `grok-4.7` is an alias with no dated snapshot, so the
    /// manifest records this with every judge response.
    #[serde(default)]
    pub fingerprint: Option<String>,
    pub stop_reason: Option<String>,
    pub latency_ms: u64,
}

#[derive(thiserror::Error, Debug, Clone, PartialEq)]
pub enum CallError {
    #[error("request was not sent: {0}")]
    NotSent(String),
    #[error("transport error after sending: {0}")]
    Transport(String),
    #[error("HTTP {status}: {excerpt}")]
    Http { status: u16, excerpt: String },
    #[error("unparseable response: {0}")]
    Parse(String),
}

impl CallError {
    /// How a failed call settles its reservation (AC 250): a request that
    /// never left is free; a 4xx rejection is not billed; anything else
    /// (5xx, timeout, broken connection, a 2xx we could not parse) is
    /// uncertain and counts as spent.
    pub fn settle(&self) -> Settle {
        match self {
            CallError::NotSent(_) => Settle::NeverSent,
            CallError::Http { status, .. } if (400..500).contains(status) => Settle::Billed(0),
            _ => Settle::Uncertain,
        }
    }

    /// Worth the one retry AC 250 allows.
    pub fn retryable(&self) -> bool {
        match self {
            CallError::NotSent(_) | CallError::Transport(_) => true,
            CallError::Http { status, .. } => *status == 429 || *status >= 500,
            CallError::Parse(_) => false,
        }
    }
}

pub type MockFn = dyn Fn(&ModelRequest) -> Result<ModelResponse, CallError> + Send + Sync;

#[derive(Clone)]
pub enum Provider {
    OpenAi { base: String, key: Secret },
    Anthropic { base: String, key: Secret },
    Google { base: String, key: Secret },
    Mistral { base: String, key: Secret },
    Xai { base: String, key: Secret },
    Mock(Arc<MockFn>),
}

impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.vendor())
    }
}

pub const OPENAI_BASE: &str = "https://api.openai.com";
pub const ANTHROPIC_BASE: &str = "https://api.anthropic.com";
pub const GOOGLE_BASE: &str = "https://generativelanguage.googleapis.com";
pub const MISTRAL_BASE: &str = "https://api.mistral.ai";
pub const XAI_BASE: &str = "https://api.x.ai";

/// The environment variable each vendor's key is read from.
pub fn key_env(vendor: &str) -> Option<&'static str> {
    Some(match vendor {
        "openai" => "OPENAI_API_KEY",
        "anthropic" => "ANTHROPIC_API_KEY",
        "google" => "GEMINI_API_KEY",
        "mistral" => "MISTRAL_API_KEY",
        "xai" => "XAI_API_KEY",
        _ => return None,
    })
}

impl Provider {
    pub fn for_vendor(vendor: &str, key: Secret) -> Option<Self> {
        Some(match vendor {
            "openai" => Provider::OpenAi {
                base: OPENAI_BASE.into(),
                key,
            },
            "anthropic" => Provider::Anthropic {
                base: ANTHROPIC_BASE.into(),
                key,
            },
            "google" => Provider::Google {
                base: GOOGLE_BASE.into(),
                key,
            },
            "mistral" => Provider::Mistral {
                base: MISTRAL_BASE.into(),
                key,
            },
            "xai" => Provider::Xai {
                base: XAI_BASE.into(),
                key,
            },
            _ => return None,
        })
    }

    pub fn vendor(&self) -> &'static str {
        match self {
            Provider::OpenAi { .. } => "openai",
            Provider::Anthropic { .. } => "anthropic",
            Provider::Google { .. } => "google",
            Provider::Mistral { .. } => "mistral",
            Provider::Xai { .. } => "xai",
            Provider::Mock(_) => "mock",
        }
    }

    /// The request body exactly as sent (headers aside), for the manifest's
    /// direct-call settings and for tests.
    pub fn body(&self, r: &ModelRequest) -> Value {
        match self {
            Provider::OpenAi { .. } => {
                let mut b = json!({
                    "model": r.model,
                    "instructions": r.system,
                    "input": r.user,
                    "max_output_tokens": r.max_output_tokens,
                    "store": false,
                });
                if let Some(e) = &r.effort {
                    b["reasoning"] = json!({ "effort": e });
                }
                if let Some((name, schema)) = &r.json_schema {
                    b["text"] = json!({"format": {"type": "json_schema", "name": name, "schema": schema, "strict": true}});
                }
                b
            }
            Provider::Anthropic { .. } => {
                let mut b = json!({
                    "model": r.model,
                    "max_tokens": r.max_output_tokens,
                    "system": r.system,
                    "messages": [{"role": "user", "content": r.user}],
                });
                if let Some(e) = &r.effort {
                    b["output_config"] = json!({ "effort": e });
                }
                b
            }
            Provider::Google { .. } => {
                let mut gc = json!({ "maxOutputTokens": r.max_output_tokens });
                if let Some(e) = &r.effort {
                    gc["thinkingConfig"] = json!({ "thinkingLevel": e });
                }
                json!({
                    "systemInstruction": {"parts": [{"text": r.system}]},
                    "contents": [{"role": "user", "parts": [{"text": r.user}]}],
                    "generationConfig": gc,
                })
            }
            Provider::Mistral { .. } | Provider::Xai { .. } => {
                let is_mistral = matches!(self, Provider::Mistral { .. });
                let mut b = json!({
                    "model": r.model,
                    "messages": [
                        {"role": "system", "content": r.system},
                        {"role": "user", "content": r.user},
                    ],
                    "temperature": 0,
                });
                b[if is_mistral {
                    "max_tokens"
                } else {
                    "max_completion_tokens"
                }] = json!(r.max_output_tokens);
                if let Some(seed) = r.seed {
                    b[if is_mistral { "random_seed" } else { "seed" }] = json!(seed);
                }
                if let (false, Some(e)) = (is_mistral, &r.effort) {
                    b["reasoning_effort"] = json!(e);
                }
                if let Some((name, schema)) = &r.json_schema {
                    b["response_format"] = json!({"type": "json_schema", "json_schema": {"name": name, "schema": schema, "strict": true}});
                }
                b
            }
            Provider::Mock(_) => serde_json::to_value(r).unwrap_or(Value::Null),
        }
    }

    pub async fn call(
        &self,
        http: &reqwest::Client,
        r: &ModelRequest,
    ) -> Result<ModelResponse, CallError> {
        if let Provider::Mock(f) = self {
            return f(r);
        }
        let started = Instant::now();
        let body = self.body(r);
        let req = match self {
            Provider::OpenAi { base, key } => http
                .post(format!("{base}/v1/responses"))
                .bearer_auth(key.expose()),
            Provider::Anthropic { base, key } => http
                .post(format!("{base}/v1/messages"))
                .header("x-api-key", key.expose())
                .header("anthropic-version", "2023-06-01"),
            Provider::Google { base, key } => http
                .post(format!("{base}/v1beta/models/{}:generateContent", r.model))
                .header("x-goog-api-key", key.expose()),
            Provider::Mistral { base, key } | Provider::Xai { base, key } => http
                .post(format!("{base}/v1/chat/completions"))
                .bearer_auth(key.expose()),
            Provider::Mock(_) => unreachable!(),
        };
        let resp = req.json(&body).send().await.map_err(|e| {
            if e.is_connect() || e.is_builder() {
                CallError::NotSent(e.without_url().to_string())
            } else {
                CallError::Transport(e.without_url().to_string())
            }
        })?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| CallError::Transport(e.without_url().to_string()))?;
        if !(200..300).contains(&status) {
            let excerpt: String = text.chars().take(300).collect();
            return Err(CallError::Http { status, excerpt });
        }
        let v: Value = serde_json::from_str(&text).map_err(|e| CallError::Parse(e.to_string()))?;
        let mut out = parse_response(self.vendor(), &v)?;
        out.latency_ms = started.elapsed().as_millis() as u64;
        Ok(out)
    }
}

fn u(v: &Value, path: &[&str]) -> u64 {
    let mut cur = v;
    for p in path {
        cur = &cur[*p];
    }
    cur.as_u64().unwrap_or(0)
}

fn s(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(str::to_string)
}

/// Parse a vendor's success body into text and usage.
pub fn parse_response(vendor: &str, v: &Value) -> Result<ModelResponse, CallError> {
    let (text, input, output, stop) = match vendor {
        "openai" => {
            let mut text = String::new();
            for item in v["output"].as_array().into_iter().flatten() {
                if item["type"] == "message" {
                    for c in item["content"].as_array().into_iter().flatten() {
                        if c["type"] == "output_text" {
                            text.push_str(c["text"].as_str().unwrap_or(""));
                        }
                    }
                }
            }
            (
                text,
                u(v, &["usage", "input_tokens"]),
                u(v, &["usage", "output_tokens"]),
                s(v, "status"),
            )
        }
        "anthropic" => {
            let text = v["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|c| c["type"] == "text")
                .filter_map(|c| c["text"].as_str())
                .collect::<String>();
            (
                text,
                u(v, &["usage", "input_tokens"]),
                u(v, &["usage", "output_tokens"]),
                s(v, "stop_reason"),
            )
        }
        "google" => {
            let cand = &v["candidates"][0];
            let text = cand["content"]["parts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|p| p["thought"] != true)
                .filter_map(|p| p["text"].as_str())
                .collect::<String>();
            let out = u(v, &["usageMetadata", "candidatesTokenCount"])
                + u(v, &["usageMetadata", "thoughtsTokenCount"]);
            (
                text,
                u(v, &["usageMetadata", "promptTokenCount"]),
                out,
                cand["finishReason"].as_str().map(str::to_string),
            )
        }
        "mistral" | "xai" => {
            let choice = &v["choices"][0];
            let text = choice["message"]["content"]
                .as_str()
                .unwrap_or("")
                .to_string();
            let mut out = u(v, &["usage", "completion_tokens"]);
            if vendor == "xai" {
                // Whether xAI counts reasoning inside completion_tokens is not
                // stated; adding it can only overstate spend.
                out += u(
                    v,
                    &["usage", "completion_tokens_details", "reasoning_tokens"],
                );
            }
            (
                text,
                u(v, &["usage", "prompt_tokens"]),
                out,
                choice["finish_reason"].as_str().map(str::to_string),
            )
        }
        other => return Err(CallError::Parse(format!("unknown vendor {other}"))),
    };
    if input == 0 && output == 0 {
        return Err(CallError::Parse(format!("{vendor} response has no usage")));
    }
    Ok(ModelResponse {
        text,
        input_tokens: input,
        output_tokens: output,
        model_returned: s(v, "model").or_else(|| s(v, "modelVersion")),
        fingerprint: s(v, "system_fingerprint"),
        stop_reason: stop,
        latency_ms: 0,
    })
}

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .expect("reqwest client")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> ModelRequest {
        ModelRequest {
            model: "m-1".into(),
            system: "sys".into(),
            user: "usr".into(),
            max_output_tokens: 16_384,
            effort: Some("medium".into()),
            json_schema: None,
            seed: None,
        }
    }

    #[test]
    fn request_bodies_carry_cap_and_effort_per_vendor() {
        let k = Secret::new("k-not-a-real-key");
        let b = Provider::for_vendor("openai", k.clone())
            .unwrap()
            .body(&req());
        assert_eq!(b["max_output_tokens"], 16_384);
        assert_eq!(b["reasoning"]["effort"], "medium");
        let b = Provider::for_vendor("anthropic", k.clone())
            .unwrap()
            .body(&req());
        assert_eq!(b["max_tokens"], 16_384);
        assert_eq!(b["output_config"]["effort"], "medium");
        assert_eq!(b["system"], "sys");
        let b = Provider::for_vendor("google", k.clone())
            .unwrap()
            .body(&req());
        assert_eq!(b["generationConfig"]["maxOutputTokens"], 16_384);
        assert_eq!(
            b["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "medium"
        );
        let mut jr = req();
        jr.json_schema = Some(("verdicts".into(), json!({"type": "object"})));
        jr.seed = Some(7);
        jr.effort = None;
        let b = Provider::for_vendor("mistral", k.clone())
            .unwrap()
            .body(&jr);
        assert_eq!(b["response_format"]["json_schema"]["strict"], true);
        assert_eq!(b["random_seed"], 7);
        assert_eq!(b["temperature"], 0);
        assert_eq!(b["max_tokens"], 16_384);
        let b = Provider::for_vendor("xai", k).unwrap().body(&jr);
        assert_eq!(b["seed"], 7);
        assert_eq!(b["max_completion_tokens"], 16_384);
    }

    #[test]
    fn usage_parsing_per_vendor() {
        let o = json!({"model": "gpt-6.2-sol-2026-11-01", "status": "completed",
            "output": [{"type": "reasoning"}, {"type": "message", "content": [{"type": "output_text", "text": "hi"}]}],
            "usage": {"input_tokens": 10, "output_tokens": 5}});
        let r = parse_response("openai", &o).unwrap();
        assert_eq!(
            (r.text.as_str(), r.input_tokens, r.output_tokens),
            ("hi", 10, 5)
        );
        assert_eq!(r.model_returned.as_deref(), Some("gpt-6.2-sol-2026-11-01"));

        let a = json!({"model": "claude-opus-5-5", "stop_reason": "end_turn",
            "content": [{"type": "thinking", "thinking": "x"}, {"type": "text", "text": "ok"}],
            "usage": {"input_tokens": 3, "output_tokens": 4}});
        assert_eq!(parse_response("anthropic", &a).unwrap().text, "ok");

        let g = json!({"modelVersion": "gemini-4-carbon",
            "candidates": [{"finishReason": "STOP", "content": {"parts": [{"text": "t", "thought": true}, {"text": "answer"}]}}],
            "usageMetadata": {"promptTokenCount": 7, "candidatesTokenCount": 2, "thoughtsTokenCount": 9}});
        let r = parse_response("google", &g).unwrap();
        assert_eq!((r.text.as_str(), r.output_tokens), ("answer", 11));

        let x = json!({"model": "grok-4.7", "system_fingerprint": "fp_1", "choices": [{"finish_reason": "stop", "message": {"content": "{}"}}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 2, "completion_tokens_details": {"reasoning_tokens": 3}}});
        let r = parse_response("xai", &x).unwrap();
        assert_eq!(r.output_tokens, 5);
        assert_eq!(r.fingerprint.as_deref(), Some("fp_1"));
        assert!(
            parse_response("mistral", &json!({"choices": []})).is_err(),
            "no usage is an error"
        );
    }

    #[test]
    fn failure_settlement_and_retry_rules() {
        assert_eq!(CallError::NotSent("x".into()).settle(), Settle::NeverSent);
        assert_eq!(
            CallError::Http {
                status: 400,
                excerpt: String::new()
            }
            .settle(),
            Settle::Billed(0)
        );
        assert_eq!(
            CallError::Http {
                status: 503,
                excerpt: String::new()
            }
            .settle(),
            Settle::Uncertain
        );
        assert_eq!(
            CallError::Transport("reset".into()).settle(),
            Settle::Uncertain
        );
        assert_eq!(CallError::Parse("x".into()).settle(), Settle::Uncertain);
        assert!(CallError::Http {
            status: 429,
            excerpt: String::new()
        }
        .retryable());
        assert!(!CallError::Http {
            status: 400,
            excerpt: String::new()
        }
        .retryable());
    }

    #[tokio::test]
    async fn http_call_against_a_local_mock_sends_auth_and_parses() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/v1/messages")
            .match_header("x-api-key", "k-not-a-real-key")
            .match_header("anthropic-version", "2023-06-01")
            .with_status(200)
            .with_body(r#"{"model":"claude-opus-5-5","content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":1,"output_tokens":1}}"#)
            .create_async()
            .await;
        let p = Provider::Anthropic {
            base: server.url(),
            key: Secret::new("k-not-a-real-key"),
        };
        let r = p.call(&http_client(), &req()).await.unwrap();
        assert_eq!(r.text, "ok");
        m.assert_async().await;
    }
}
