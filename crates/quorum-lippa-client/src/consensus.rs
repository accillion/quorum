//! `SessionCreateRequest` + `SessionStatus` (re-exported from `lib.rs`).

#[derive(Debug, Clone)]
pub struct SessionCreateRequest {
    /// Prompt body (Quorum's assembled bundle). Min 20 chars or Lippa
    /// returns 400.
    pub prompt: String,
    pub project_id: Option<String>,
    pub debate_mode: String, // "standard" | "document_critique" | "red_blue_team"
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    /// `pending` or `running` — keep polling.
    InProgress,
    Converged,
    Failed(String),
    Cancelled,
    Error(String),
    /// 401 mid-poll — session terminated by server.
    Unauthorized,
}

impl SessionStatus {
    pub fn is_terminal(&self) -> bool {
        !matches!(self, SessionStatus::InProgress)
    }

    pub(crate) fn from_status_string(s: &str) -> Self {
        match s {
            "converged" => Self::Converged,
            "pending" | "running" | "in_progress" => Self::InProgress,
            "failed" => Self::Failed(s.to_string()),
            "cancelled" => Self::Cancelled,
            "error" => Self::Error(s.to_string()),
            other => Self::Error(format!("unknown_status:{other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_classification() {
        assert!(!SessionStatus::InProgress.is_terminal());
        assert!(SessionStatus::Converged.is_terminal());
        assert!(SessionStatus::Failed("x".into()).is_terminal());
        assert!(SessionStatus::Cancelled.is_terminal());
        assert!(SessionStatus::Error("x".into()).is_terminal());
        assert!(SessionStatus::Unauthorized.is_terminal());
    }
    #[test]
    fn from_status_string_maps_known_values() {
        assert_eq!(
            SessionStatus::from_status_string("converged"),
            SessionStatus::Converged
        );
        assert_eq!(
            SessionStatus::from_status_string("pending"),
            SessionStatus::InProgress
        );
        assert_eq!(
            SessionStatus::from_status_string("running"),
            SessionStatus::InProgress
        );
        assert!(matches!(
            SessionStatus::from_status_string("failed"),
            SessionStatus::Failed(_)
        ));
        assert_eq!(
            SessionStatus::from_status_string("cancelled"),
            SessionStatus::Cancelled
        );
        assert!(matches!(
            SessionStatus::from_status_string("error"),
            SessionStatus::Error(_)
        ));
        assert!(matches!(
            SessionStatus::from_status_string("weird"),
            SessionStatus::Error(_)
        ));
    }
}
