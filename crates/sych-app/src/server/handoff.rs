//! `POST /v1/runs/:id/handoff`: the run's own findings as a package
//! for a separate author workflow. It only reads the committed result: no model, no change to
//! the result or its findings, and nothing is published.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use sych_core::api::HandoffRequest;
use sych_core::config::ApiRole;
use sych_core::handoff::{self, Handoff, HandoffError};

use super::App;
use super::api::{Caller, parse, status, visible_run};
use super::error::ApiError;
use super::tours::run_of;

pub async fn handoff(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<Handoff>, ApiError> {
    caller.need(ApiRole::Reader)?;
    let request: HandoffRequest = parse(&body)?;
    let run = run_of(&visible_run(&app, &caller, &id, ApiRole::Reader).await?)?;
    let result = app
        .storage
        .load_result(&caller.tenant_id, run)
        .await
        .map_err(|e| status(StatusCode::CONFLICT, "NO_RESULT", e.to_string()))?;
    handoff::package(
        &result,
        &request.finding_ids,
        request.source_snapshot_id.as_ref(),
    )
    .map(Json)
    .map_err(|e| match e {
        HandoffError::SnapshotMismatch { .. } => {
            status(StatusCode::CONFLICT, "SNAPSHOT_MISMATCH", e.to_string())
        }
        HandoffError::Foreign(_) => {
            status(StatusCode::BAD_REQUEST, "FOREIGN_FINDING", e.to_string())
        }
        HandoffError::NoFindings => {
            status(StatusCode::BAD_REQUEST, "INVALID_REQUEST", e.to_string())
        }
    })
}
