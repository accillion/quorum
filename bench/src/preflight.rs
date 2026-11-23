//! Run-1b preflight: the bench Lippa project must stay empty (Gate A
//! "Also" ruling). Lippa seeds every Consensus prompt of a project-linked
//! session with that project's confirmed and pinned memory items
//! (`consensus_service._generate_seeding_context`); the `S-` arms would
//! not get them. `GET /api/v1/projects/{id}/memory` counts confirmed and
//! pinned, active or challenged, unarchived items, a superset of what
//! seeding reads, so the run refuses to start unless its `total` is 0.

use crate::secret::Secret;

#[derive(thiserror::Error, Debug, PartialEq)]
pub enum PreflightError {
    #[error("the bench project has {0} confirmed or pinned memory item(s); Lippa would seed them into every Q- prompt. Empty the project first")]
    ProjectHasMemory(u64),
    #[error("project memory check failed: HTTP {0}")]
    Http(u16),
    #[error("project memory check failed: {0}")]
    Other(String),
    #[error("no Lippa project id configured: set lippa_project_id in bench/bench.toml")]
    NoProject,
}

pub async fn check_project_empty(
    http: &reqwest::Client,
    base_url: &str,
    project_id: &str,
    session: &Secret,
) -> Result<(), PreflightError> {
    if project_id.trim().is_empty() {
        return Err(PreflightError::NoProject);
    }
    let url = format!(
        "{}/api/v1/projects/{project_id}/memory?limit=1",
        base_url.trim_end_matches('/')
    );
    let resp = http
        .get(url)
        .header("cookie", format!("session={}", session.expose()))
        .send()
        .await
        .map_err(|e| PreflightError::Other(e.without_url().to_string()))?;
    let status = resp.status().as_u16();
    if status != 200 {
        return Err(PreflightError::Http(status));
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| PreflightError::Other(e.to_string()))?;
    let total = v["total"]
        .as_u64()
        .ok_or_else(|| PreflightError::Other("no `total` in the response".into()))?;
    if total > 0 {
        return Err(PreflightError::ProjectHasMemory(total));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn check(total: u64) -> Result<(), PreflightError> {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/v1/projects/p1/memory?limit=1")
            .match_header(
                "cookie",
                format!("{}=cookie-value-for-test", "session").as_str(),
            )
            .with_status(200)
            .with_body(format!(
                "{{\"items\": [], \"total\": {total}, \"limit\": 1, \"offset\": 0}}"
            ))
            .create_async()
            .await;
        check_project_empty(
            &reqwest::Client::new(),
            &server.url(),
            "p1",
            &Secret::new("cookie-value-for-test"),
        )
        .await
    }

    #[tokio::test]
    async fn negative_control_a_project_with_memory_is_refused() {
        assert_eq!(check(2).await, Err(PreflightError::ProjectHasMemory(2)));
    }

    #[tokio::test]
    async fn an_empty_project_passes() {
        assert_eq!(check(0).await, Ok(()));
    }

    #[tokio::test]
    async fn no_project_id_is_refused_before_any_request() {
        let r = check_project_empty(
            &reqwest::Client::new(),
            "https://unused.invalid",
            " ",
            &Secret::new("x"),
        )
        .await;
        assert_eq!(r, Err(PreflightError::NoProject));
    }
}
