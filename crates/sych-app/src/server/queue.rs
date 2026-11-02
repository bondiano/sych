//! `/v1` operations: jobs with the runs they serve, publications that wait for a decision, and
//! an operator's decision to create an `UNCERTAIN` one again, recorded in audit.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde::Deserialize;
use sych_core::api::{JobEntry, Page, PublicationStatus, ReconcileRequest, UnresolvedPublication};
use sych_core::config::ApiRole;
use sych_storage::JobFilter;

use super::App;
use super::api::{Caller, idempotent, now, parse, status, storage_error, to_value};
use super::error::ApiError;
use super::views::{grants, invalid, limit, not_found, readable, visibility};

#[derive(Deserialize)]
pub struct JobQuery {
    repository_id: Option<i64>,
    state: Option<String>,
    before: Option<i64>,
    limit: Option<i64>,
}

/// `GET /v1/jobs`: over the repositories the caller may write to.
pub async fn jobs(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(q): Query<JobQuery>,
) -> Result<Json<Page<JobEntry>>, ApiError> {
    caller.need(ApiRole::Writer)?;
    if !matches!(
        q.state.as_deref(),
        None | Some("QUEUED" | "CLAIMED" | "DONE" | "FAILED" | "CANCELLED")
    ) {
        return Err(invalid(
            "state is QUEUED, CLAIMED, DONE, FAILED or CANCELLED",
        ));
    }

    let limit = limit(q.limit)?;
    let written: Option<Vec<i64>> = if caller.reads_everything() {
        None
    } else {
        Some(
            grants(&app, &caller)
                .await?
                .into_iter()
                .filter(|g| g.role >= ApiRole::Writer)
                .map(|g| g.repository.id)
                .collect(),
        )
    };
    let filter = JobFilter {
        repository_id: q.repository_id,
        state: q.state,
        before: q.before,
        limit,
    };
    let page = app
        .storage
        .jobs(visibility(&caller, written.as_deref()), &filter)
        .await
        .map_err(|e| storage_error(&e))?;
    Ok(Json(page))
}

#[derive(Deserialize)]
pub struct PublicationQuery {
    repository_id: Option<i64>,
}

/// `GET /v1/publications`: `PUBLISHING` and `UNCERTAIN` operations, oldest first.
pub async fn publications(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(q): Query<PublicationQuery>,
) -> Result<Json<Vec<UnresolvedPublication>>, ApiError> {
    caller.need(ApiRole::Reader)?;
    let ids = readable(&app, &caller).await?;
    let unresolved = app
        .storage
        .unresolved(visibility(&caller, ids.as_deref()), q.repository_id)
        .await
        .map_err(|e| storage_error(&e))?;
    Ok(Json(unresolved))
}

/// `POST /v1/publications/:id/reconcile`: the operator decision `sych reconcile --allow-create`
/// records, with its reason and the duplicate risk it accepts.
pub async fn reconcile(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<i64>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    caller.need(ApiRole::Writer)?;
    let request: ReconcileRequest = parse(&body)?;
    let reason = request.reason.trim();
    if reason.is_empty() {
        return Err(invalid("a reason is required"));
    }

    let (_, change, ..) = app
        .storage
        .publication_view(id)
        .await
        .map_err(|e| storage_error(&e))?
        .filter(|v| v.0 == caller.tenant_id)
        .ok_or_else(not_found)?;
    let row = app
        .storage
        .change(change)
        .await
        .map_err(|e| storage_error(&e))?;
    caller
        .scope(
            &app,
            Some(&row.connection_id),
            Some(&row.full_name),
            ApiRole::Writer,
        )
        .await?;
    idempotent(
        &app,
        &caller,
        "POST /v1/publications/:id/reconcile",
        &id.to_string(),
        &headers,
        &body,
        || async {
            let recreated = app
                .storage
                .authorize_recreate(id, &caller.tenant_id, &caller.actor, reason, now(&app))
                .await
                .map_err(|e| storage_error(&e))?;
            if !recreated {
                return Err(status(
                    StatusCode::CONFLICT,
                    "NOT_RECREATABLE",
                    "only an UNCERTAIN publication with no known remote object is created again",
                ));
            }

            let (_, _, delivery, relevance, remote_id, last_error) = app
                .storage
                .publication_view(id)
                .await
                .map_err(|e| storage_error(&e))?
                .ok_or_else(not_found)?;
            let status = PublicationStatus {
                publication_id: id,
                delivery,
                relevance,
                remote_id,
                last_error,
            };
            Ok((StatusCode::OK, to_value(&status)?))
        },
    )
    .await
}
