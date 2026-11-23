//! A local stand-in for Lippa's three Consensus routes, for zero-cost dry
//! runs (bundle-fit and leakage checks, ACs 243–244) and tests. It answers
//! every session with no findings and never contacts anything.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

/// What the fake returns for a session detail.
#[derive(Clone)]
pub struct FakeSession {
    pub detail: Value,
}

impl Default for FakeSession {
    fn default() -> Self {
        Self {
            detail: json!({
                "id": "dry-run",
                "status": "converged",
                "summary_text": "dry run",
                "final_agreement_score": 1.0,
                "credits_consumed": 0,
                "agreement": [], "divergence": [], "assumptions": [],
                "models": [],
                "rounds": []
            }),
        }
    }
}

pub struct FakeLippa {
    addr: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
}

impl FakeLippa {
    pub async fn start(session: FakeSession) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        let session = Arc::new(session);
        let (tx, mut rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut rx => break,
                    acc = listener.accept() => {
                        let Ok((stream, _)) = acc else { continue };
                        let s = session.clone();
                        tokio::spawn(async move {
                            let svc = service_fn(move |req| answer(req, s.clone()));
                            let _ = http1::Builder::new().serve_connection(TokioIo::new(stream), svc).await;
                        });
                    }
                }
            }
        });
        Ok(Self {
            addr,
            stop: Some(tx),
        })
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.addr.port())
    }
}

impl Drop for FakeLippa {
    fn drop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
    }
}

async fn answer(
    req: Request<Incoming>,
    s: Arc<FakeSession>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();
    let _ = req.into_body().collect().await;
    let sessions = "/api/v1/consensus/sessions";
    let id = s.detail["id"].as_str().unwrap_or("dry-run").to_string();
    let (status, body) = if method == "POST" && path == sessions {
        (
            201,
            json!({"id": id, "status": "pending", "credits_reserved": 0, "credits_remaining": 0}),
        )
    } else if method == "GET" && path == format!("{sessions}/{id}/status") {
        (
            200,
            json!({"status": "converged", "rounds_completed": 1, "rounds_total": 1, "models_active": 3, "models_dropped": 0, "elapsed_seconds": 0.0}),
        )
    } else if method == "GET" && path == format!("{sessions}/{id}") {
        (200, s.detail.clone())
    } else {
        (404, json!({"detail": "not found"}))
    };
    let resp = Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(body.to_string())))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::new())));
    Ok(resp)
}
