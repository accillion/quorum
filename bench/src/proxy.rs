//! The loopback recording proxy (Gate A ruling 5).
//!
//! Each case's `.quorum/config.toml` points `base_url` at this proxy
//! (`http://127.0.0.1:<port>`, which Quorum accepts because the host is
//! loopback). The proxy forwards every request to Lippa unchanged,
//! including the `Cookie` header Quorum sends, and records the exchange so
//! the harness learns what the archive does not hold: the exact bundle
//! bytes (the `prompt` form field), the observed model ids, each seat's
//! Round 0 answer and any fallback (`answered_model_id`).
//!
//! A recording keeps **bodies only**: no request or response header is
//! ever stored, so the cookie cannot be recorded from a header. Multipart
//! bodies are stored as their form fields, never as raw MIME. Recordings
//! are scrubbed again before they are written ([`Exchange::scrubbed`]).
//! The proxy binds to 127.0.0.1 only and never logs or prints a header.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use crate::scrub::Scrubber;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum RecordedBody {
    Empty,
    Json(Value),
    /// Multipart form fields, name to text value.
    Form(BTreeMap<String, String>),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Exchange {
    pub method: String,
    /// Path and query, as requested.
    pub path: String,
    pub status: u16,
    pub request: RecordedBody,
    pub response: RecordedBody,
    pub elapsed_ms: u64,
}

impl Exchange {
    /// The exchange with every string passed through the scrubber.
    pub fn scrubbed(&self, s: &Scrubber) -> Exchange {
        let body = |b: &RecordedBody| match b {
            RecordedBody::Empty => RecordedBody::Empty,
            RecordedBody::Json(v) => RecordedBody::Json(s.scrub_json(v)),
            RecordedBody::Form(f) => {
                RecordedBody::Form(f.iter().map(|(k, v)| (k.clone(), s.scrub(v))).collect())
            }
            RecordedBody::Text(t) => RecordedBody::Text(s.scrub(t)),
        };
        Exchange {
            method: self.method.clone(),
            path: s.scrub(&self.path),
            status: self.status,
            request: body(&self.request),
            response: body(&self.response),
            elapsed_ms: self.elapsed_ms,
        }
    }
}

const SESSIONS: &str = "/api/v1/consensus/sessions";

/// The bundle Quorum sent: the `prompt` field of the session create.
pub fn sent_bundle(exchanges: &[Exchange]) -> Option<&str> {
    exchanges.iter().find_map(
        |e| match (&*e.method, e.path.split('?').next(), &e.request) {
            ("POST", Some(SESSIONS), RecordedBody::Form(f)) => f.get("prompt").map(String::as_str),
            _ => None,
        },
    )
}

/// The create response status, if a create was sent.
pub fn create_status(exchanges: &[Exchange]) -> Option<u16> {
    exchanges
        .iter()
        .find(|e| e.method == "POST" && e.path.split('?').next() == Some(SESSIONS))
        .map(|e| e.status)
}

/// The last successful session-detail body (`GET /sessions/{id}`, not
/// `/status`).
pub fn session_detail(exchanges: &[Exchange]) -> Option<&Value> {
    exchanges.iter().rev().find_map(|e| {
        let p = e.path.split('?').next().unwrap_or("");
        let rest = p.strip_prefix(SESSIONS)?.strip_prefix('/')?;
        if e.method != "GET" || e.status != 200 || rest.is_empty() || rest.contains('/') {
            return None;
        }
        match &e.response {
            RecordedBody::Json(v) => Some(v),
            _ => None,
        }
    })
}

fn boundary(content_type: &str) -> Option<String> {
    content_type.split(';').map(str::trim).find_map(|p| {
        p.strip_prefix("boundary=")
            .map(|b| b.trim_matches('"').to_string())
    })
}

/// Text fields of a multipart/form-data body. File parts are kept by name
/// with their bytes decoded lossily.
pub fn parse_multipart(body: &[u8], content_type: &str) -> Option<BTreeMap<String, String>> {
    let b = boundary(content_type)?;
    let delim = format!("--{b}");
    let text = String::from_utf8_lossy(body);
    let mut out = BTreeMap::new();
    for part in text.split(delim.as_str()).skip(1) {
        if part.starts_with("--") {
            break;
        }
        let part = part.strip_prefix("\r\n").unwrap_or(part);
        let (head, value) = part.split_once("\r\n\r\n")?;
        let value = value.strip_suffix("\r\n").unwrap_or(value);
        let name = head.lines().find_map(|l| {
            let l = l.trim();
            if !l.to_ascii_lowercase().starts_with("content-disposition:") {
                return None;
            }
            let i = l.find("name=\"")? + 6;
            let j = l[i..].find('"')? + i;
            Some(l[i..j].to_string())
        })?;
        out.insert(name, value.to_string());
    }
    Some(out)
}

fn record_body(bytes: &[u8], content_type: Option<&str>) -> RecordedBody {
    if bytes.is_empty() {
        return RecordedBody::Empty;
    }
    let ct = content_type.unwrap_or("").to_ascii_lowercase();
    if ct.starts_with("multipart/form-data") {
        if let Some(f) = parse_multipart(bytes, content_type.unwrap_or("")) {
            return RecordedBody::Form(f);
        }
    }
    if let Ok(v) = serde_json::from_slice::<Value>(bytes) {
        return RecordedBody::Json(v);
    }
    RecordedBody::Text(String::from_utf8_lossy(bytes).into_owned())
}

/// Headers never forwarded in either direction (hop-by-hop, or rewritten
/// by the client libraries). `accept-encoding` is dropped so Lippa answers
/// uncompressed and the recorded body is readable.
const DROP: &[&str] = &[
    "host",
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authorization",
    "proxy-authenticate",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "content-length",
    "accept-encoding",
    "content-encoding",
];

struct State {
    upstream: String,
    http: reqwest::Client,
    log: Mutex<Vec<Exchange>>,
}

#[derive(thiserror::Error, Debug)]
pub enum ProxyError {
    #[error("upstream `{0}` must be https (plain http only on 127.0.0.1 or localhost, for tests)")]
    InsecureUpstream(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub struct RecordingProxy {
    addr: SocketAddr,
    state: Arc<State>,
    stop: Option<oneshot::Sender<()>>,
}

impl RecordingProxy {
    pub async fn start(upstream: &str) -> Result<Self, ProxyError> {
        let upstream = upstream.trim_end_matches('/').to_string();
        let loopback_http =
            upstream.starts_with("http://127.0.0.1") || upstream.starts_with("http://localhost");
        if !upstream.starts_with("https://") && !loopback_http {
            return Err(ProxyError::InsecureUpstream(upstream));
        }
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(900))
            .build()
            .expect("reqwest client");
        let state = Arc::new(State {
            upstream,
            http,
            log: Mutex::new(Vec::new()),
        });
        let (tx, mut rx) = oneshot::channel::<()>();
        let st = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut rx => break,
                    acc = listener.accept() => {
                        let Ok((stream, _)) = acc else { continue };
                        let st = st.clone();
                        tokio::spawn(async move {
                            let svc = service_fn(move |req| handle(req, st.clone()));
                            let _ = http1::Builder::new().serve_connection(TokioIo::new(stream), svc).await;
                        });
                    }
                }
            }
        });
        Ok(Self {
            addr,
            state,
            stop: Some(tx),
        })
    }

    /// The `base_url` to give Quorum.
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.addr.port())
    }

    /// Take the exchanges recorded so far (unscrubbed, in memory only).
    pub fn take(&self) -> Vec<Exchange> {
        std::mem::take(&mut *self.state.log.lock().expect("proxy log"))
    }
}

impl Drop for RecordingProxy {
    fn drop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
    }
}

async fn handle(
    req: Request<Incoming>,
    st: Arc<State>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let started = std::time::Instant::now();
    let method = req.method().clone();
    let path = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "/".into());
    let req_ct = req
        .headers()
        .get(hyper::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let mut fwd = reqwest::header::HeaderMap::new();
    for (k, v) in req.headers() {
        if DROP.contains(&k.as_str()) {
            continue;
        }
        if let (Ok(name), Ok(val)) = (
            reqwest::header::HeaderName::from_bytes(k.as_str().as_bytes()),
            reqwest::header::HeaderValue::from_bytes(v.as_bytes()),
        ) {
            fwd.append(name, val);
        }
    }
    let body = match req.into_body().collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => Bytes::new(),
    };
    let rmethod =
        reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);
    let upstream = st
        .http
        .request(rmethod, format!("{}{}", st.upstream, path))
        .headers(fwd)
        .body(body.clone())
        .send()
        .await;
    let (status, resp_headers, resp_body) = match upstream {
        Ok(r) => {
            let status = r.status().as_u16();
            let headers = r.headers().clone();
            let bytes = r.bytes().await.unwrap_or_default();
            (status, headers, bytes)
        }
        Err(_) => (
            502,
            reqwest::header::HeaderMap::new(),
            Bytes::from_static(b"{\"error\":\"bench proxy: upstream unreachable\"}"),
        ),
    };
    let resp_ct = resp_headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let exchange = Exchange {
        method: method.to_string(),
        path,
        status,
        request: record_body(&body, req_ct.as_deref()),
        response: record_body(&resp_body, resp_ct.as_deref()),
        elapsed_ms: started.elapsed().as_millis() as u64,
    };
    st.log.lock().expect("proxy log").push(exchange);

    let mut out =
        Response::builder().status(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY));
    for (k, v) in &resp_headers {
        if DROP.contains(&k.as_str()) {
            continue;
        }
        out = out.header(k.as_str(), v.as_bytes());
    }
    Ok(out
        .body(Full::new(resp_body))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::new()))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipart_fields_are_parsed() {
        let body = b"--XyZ\r\nContent-Disposition: form-data; name=\"prompt\"\r\n\r\nline1\r\nline2\r\n--XyZ\r\nContent-Disposition: form-data; name=\"tier\"\r\n\r\nhigh\r\n--XyZ--\r\n";
        let f = parse_multipart(body, "multipart/form-data; boundary=XyZ").unwrap();
        assert_eq!(f["prompt"], "line1\r\nline2");
        assert_eq!(f["tier"], "high");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn non_loopback_http_upstream_is_refused() {
        assert!(matches!(
            RecordingProxy::start("http://example.com").await,
            Err(ProxyError::InsecureUpstream(_))
        ));
    }

    /// Gate A ruling 5: a recording never contains the cookie value or any
    /// request or response header, while the cookie still reaches Lippa.
    #[tokio::test(flavor = "multi_thread")]
    async fn recordings_hold_bodies_only_and_never_the_cookie() {
        let cookie = format!("{}=COOKIEVALUE{}", "session", "9".repeat(24));
        let mut upstream = mockito::Server::new_async().await;
        let create = upstream
            .mock("POST", SESSIONS)
            .match_header("cookie", cookie.as_str())
            .match_header("x-req-canary", "REQHEADERCANARY")
            .match_body(mockito::Matcher::Regex("PROMPT-BODY-MARKER".into()))
            .with_status(201)
            .with_header("content-type", "application/json")
            .with_header(
                "set-cookie",
                &format!("{}=ROTATEDCOOKIEVALUE123456; Path=/", "session"),
            )
            .with_header("x-resp-canary", "RESPHEADERCANARY")
            .with_body(r#"{"id":"s1","status":"pending","credits_reserved":3}"#)
            .create_async()
            .await;
        let detail = upstream
            .mock("GET", format!("{SESSIONS}/s1").as_str())
            .match_header("cookie", cookie.as_str())
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id":"s1","models":[{"id":"gpt-6.2-sol"}],"rounds":[{"round_index":0,"model_profile_id":"gpt-6.2-sol","raw_response":"r0","answered_model_id":null}]}"#)
            .create_async()
            .await;

        let proxy = RecordingProxy::start(&upstream.url()).await.unwrap();
        let client = reqwest::Client::new();
        // A multipart body as reqwest writes it for Quorum's create request.
        let b = "BNDRY";
        let body = format!("--{b}\r\nContent-Disposition: form-data; name=\"prompt\"\r\n\r\nPROMPT-BODY-MARKER bundle bytes\r\n--{b}\r\nContent-Disposition: form-data; name=\"tier\"\r\n\r\nhigh\r\n--{b}--\r\n");
        let r = client
            .post(format!("{}{SESSIONS}", proxy.base_url()))
            .header("cookie", cookie.as_str())
            .header(
                "authorization",
                format!("{} AUTHHEADERCANARY0123456789", "Bearer"),
            )
            .header("x-req-canary", "REQHEADERCANARY")
            .header("content-type", format!("multipart/form-data; boundary={b}"))
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 201);
        assert_eq!(
            r.headers().get("x-resp-canary").unwrap(),
            "RESPHEADERCANARY",
            "response headers pass through to Quorum"
        );
        let r2 = client
            .get(format!("{}{SESSIONS}/s1", proxy.base_url()))
            .header("cookie", cookie.as_str())
            .send()
            .await
            .unwrap();
        assert_eq!(r2.status().as_u16(), 200);
        create.assert_async().await;
        detail.assert_async().await;

        let ex = proxy.take();
        assert_eq!(ex.len(), 2);
        assert_eq!(sent_bundle(&ex), Some("PROMPT-BODY-MARKER bundle bytes"));
        assert_eq!(create_status(&ex), Some(201));
        assert_eq!(
            session_detail(&ex).unwrap()["rounds"][0]["raw_response"],
            "r0"
        );

        // Raw (unscrubbed) recording: bodies only, so no header can be in it.
        let raw = serde_json::to_string(&ex).unwrap();
        let scrubbed = serde_json::to_string(
            &ex.iter()
                .map(|e| e.scrubbed(&Scrubber::new(vec![cookie.clone()])))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        for rec in [&raw, &scrubbed] {
            for needle in [
                "COOKIEVALUE",
                "ROTATEDCOOKIEVALUE",
                "AUTHHEADERCANARY",
                "REQHEADERCANARY",
                "RESPHEADERCANARY",
                "x-req-canary",
                "x-resp-canary",
                "set-cookie",
                "Content-Disposition",
                "multipart/form-data",
                "authorization",
            ] {
                assert!(
                    !rec.to_ascii_lowercase()
                        .contains(&needle.to_ascii_lowercase()),
                    "recording contains `{needle}`"
                );
            }
        }
    }
}
