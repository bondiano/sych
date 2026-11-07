//! `POST /v1/changes/:id/playground` and `GET /v1/playground`: a maintainer's try of a review
//! that publishes nothing and stays out of lineage and analytics. Every try costs money, so it
//! needs `budgets.playground_daily` and maintainer on the repository; it is audited.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde::Deserialize;
use sych_core::api::{Accepted, PlaygroundRequest, RunStatus};
use sych_core::config::ApiRole;

use super::App;
use super::api::{
    Caller, engine, idempotent, parse, request_error, run_status, status, storage_error, to_value,
};
use super::error::ApiError;
use super::views::{readable, visibility};

/// `POST /v1/changes/:id/playground`.
pub async fn start(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<i64>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    caller.need(ApiRole::Maintainer)?;
    if app.budgets.playground_daily.max_cost_micro_usd.is_none() {
        return Err(status(
            StatusCode::CONFLICT,
            "PLAYGROUND_DISABLED",
            "set budgets.playground_daily.max_cost_micro_usd to allow playground runs",
        ));
    }

    let request: PlaygroundRequest = if body.is_empty() {
        PlaygroundRequest::default()
    } else {
        parse(&body)?
    };
    let not_found = || status(StatusCode::NOT_FOUND, "NOT_FOUND", "no such object");
    let change = app.storage.change(id).await.map_err(|_| not_found())?;
    if change.partition.tenant_id != caller.tenant_id {
        return Err(not_found());
    }

    caller
        .scope(
            &app,
            Some(&change.connection_id),
            Some(&change.full_name),
            ApiRole::Maintainer,
        )
        .await?;
    let deep = request.profile.as_deref() == Some("deep");
    idempotent(
        &app,
        &caller,
        "POST /v1/changes/:id/playground",
        &id.to_string(),
        &headers,
        &body,
        || async {
            let run = engine(&app)?
                .playground(id, &caller.actor, deep)
                .await
                .map_err(|e| request_error(&e))?;
            let accepted = Accepted {
                request_id: run.id.to_string(),
                run_id: Some(run.id.to_string()),
                status_url: format!("/v1/runs/{}", run.id),
                reused: false,
            };
            Ok((StatusCode::ACCEPTED, to_value(&accepted)?))
        },
    )
    .await
}

#[derive(Deserialize)]
pub struct PlaygroundQuery {
    repository_id: Option<i64>,
}

/// `GET /v1/playground`: the 50 newest of readable repositories.
pub async fn list(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(q): Query<PlaygroundQuery>,
) -> Result<Json<Vec<RunStatus>>, ApiError> {
    caller.need(ApiRole::Reader)?;
    let ids = readable(&app, &caller).await?;
    let runs = app
        .storage
        .playground_runs(visibility(&caller, ids.as_deref()), q.repository_id, 50)
        .await
        .map_err(|e| storage_error(&e))?;
    Ok(Json(runs.into_iter().map(run_status).collect()))
}
