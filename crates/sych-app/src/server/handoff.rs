//! `POST /v1/runs/:id/handoff`: the run's own findings as a `handoff@2` package
//! for a separate author workflow. Reads the committed result and stored source snapshot metadata
//! without serving source blobs over `/v1`; calls no model, mutates nothing and publishes nothing.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use sych_core::api::HandoffRequest;
use sych_core::config::ApiRole;
use sych_core::handoff::{self, Handoff, HandoffError, HandoffSelection};
use sych_core::{ReportView, SourceSnapshot};
use sych_storage::StorageError;

use super::App;
use super::api::{Caller, parse, status, storage_error, visible_run};
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
    let source: Option<SourceSnapshot> = match app
        .storage
        .load_artifact(&caller.tenant_id, run, "source_snapshot")
        .await
    {
        Ok(snapshot) => Some(snapshot),
        Err(StorageError::ArtifactUnavailable(_)) => None,
        Err(e) => return Err(storage_error(&e)),
    };
    if source
        .as_ref()
        .is_some_and(|s| s.id != result.source_snapshot_id)
    {
        return Err(status(
            StatusCode::CONFLICT,
            "SNAPSHOT_MISMATCH",
            "the run's pinned source snapshot does not match its result",
        ));
    }

    let view = ReportView::from_result(&result, source.as_ref());
    let selection = HandoffSelection {
        finding_ids: request.finding_ids,
        all_open: request.all,
        min_severity: request.min_severity,
        categories: request.categories,
    };
    handoff::package(
        &result,
        &view,
        &selection,
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
        HandoffError::NoSelection
        | HandoffError::ConflictingSelection
        | HandoffError::NoFindings => {
            status(StatusCode::BAD_REQUEST, "INVALID_REQUEST", e.to_string())
        }
    })
}
