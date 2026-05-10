//! Private wire types. Only `SessionStatus` crosses the crate boundary
//! (re-exported from `consensus.rs` because the polling loop branches on it).
//! `SessionDetail` is intentionally not deserialized into a typed struct —
//! detail responses pass through as `serde_json::Value` to `quorum-core`
//! for parsing (see preflight notes §D2, criterion 36).

use serde::Deserialize;

/// `/api/v1/consensus/sessions/{id}/status` body shape.
#[derive(Deserialize, Debug)]
pub(crate) struct StatusBody {
    pub status: String,
    #[allow(dead_code)]
    pub rounds_completed: Option<u32>,
    #[allow(dead_code)]
    pub rounds_total: Option<u32>,
    #[allow(dead_code)]
    pub models_active: Option<u32>,
    #[allow(dead_code)]
    pub models_dropped: Option<u32>,
    #[allow(dead_code)]
    pub elapsed_seconds: Option<f64>,
}

/// 201 `POST /sessions` body.
#[derive(Deserialize, Debug)]
pub(crate) struct CreateOkBody {
    pub id: String,
    #[allow(dead_code)]
    pub status: Option<String>,
    #[allow(dead_code)]
    pub credits_reserved: Option<u32>,
    #[allow(dead_code)]
    pub credits_remaining: Option<u32>,
}

/// 409 duplicate idempotency-key body.
#[derive(Deserialize, Debug)]
pub(crate) struct CreateDuplicateBody {
    #[allow(dead_code)]
    pub error: String,
    pub existing_session_id: String,
    #[allow(dead_code)]
    pub status: String,
}
