//! `GET /v1/runs/:id/snapshots`: the source and policy snapshots a completed result is bound
//! to. It only reads the run's pinned artifacts and refuses ones the result does not name.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use sych_core::api::RunSnapshots;
use sych_core::config::ApiRole;
use sych_core::{PolicySnapshot, SourceSnapshot};
use sych_storage::StorageError;

use super::App;
use super::api::{Caller, status, storage_error, visible_run};
use super::error::ApiError;
use super::tours::run_of;

pub async fn snapshots(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<Json<RunSnapshots>, ApiError> {
    caller.need(ApiRole::Reader)?;
    let view = visible_run(&app, &caller, &id, ApiRole::Reader).await?;
    let run = run_of(&view)?;
    let storage = &app.storage;
    let tenant = &caller.tenant_id;
    let pruned = |e: StorageError| match e {
        StorageError::NotCompleted { .. } => {
            status(StatusCode::CONFLICT, "NO_RESULT", e.to_string())
        }
        StorageError::ArtifactUnavailable(_) => {
            status(StatusCode::CONFLICT, "RESULT_PRUNED", e.to_string())
        }
        e => storage_error(&e),
    };
    let result = storage.load_result(tenant, run).await.map_err(pruned)?;
    let policy: PolicySnapshot = storage
        .load_artifact(tenant, run, "policy_snapshot")
        .await
        .map_err(pruned)?;
    // The source snapshot has a shorter retention than the result.
    let source: Option<SourceSnapshot> =
        match storage.load_artifact(tenant, run, "source_snapshot").await {
            Ok(source) => Some(source),
            Err(StorageError::ArtifactUnavailable(_)) => None,
            Err(e) => return Err(storage_error(&e)),
        };
    let foreign_source = source
        .as_ref()
        .is_some_and(|s| s.id != result.source_snapshot_id);
    if foreign_source || policy.id != result.policy_snapshot_id {
        let message = "the run's pinned snapshots are not the ones its result names";
        return Err(status(StatusCode::CONFLICT, "SNAPSHOT_MISMATCH", message));
    }

    Ok(Json(RunSnapshots {
        run_id: view.run_id,
        source,
        policy,
    }))
}
