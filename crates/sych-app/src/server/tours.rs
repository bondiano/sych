//! `/v1` tour reading (spec §21.8, roadmap M5.2): a stored tour and the caller's own session
//! through it. No model is called, answers stay hidden until revealed, and a session never
//! changes the result, obligations or an approval, and is never published.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde::Deserialize;
use sych_core::api::{RevealRequest, TourView};
use sych_core::config::ApiRole;
use sych_core::tour::{self, SessionState, TourArtifact, TourSession};
use sych_core::{RunId, TourId};
use sych_storage::{RunView, StorageError, tour_role};

use super::App;
use super::api::{Caller, idempotent, now, parse, status, storage_error, to_value, visible_run};
use super::error::ApiError;

#[derive(Deserialize)]
pub struct TourQuery {
    tour: Option<String>,
}

pub(super) fn not_found(message: &str) -> ApiError {
    status(StatusCode::NOT_FOUND, "NOT_FOUND", message)
}

pub(super) fn run_of(view: &RunView) -> Result<RunId, ApiError> {
    Ok(RunId(
        view.run_id
            .parse()
            .map_err(|_| not_found("no such object"))?,
    ))
}

/// The named tour of the run, or its newest one.
pub(super) async fn tour_id(
    app: &App,
    caller: &Caller,
    run: RunId,
    named: Option<&str>,
) -> Result<TourId, ApiError> {
    let storage = &app.storage;
    let tenant = &caller.0.tenant_id;
    match named {
        Some(t) => {
            let id = TourId(t.parse().map_err(|_| {
                status(
                    StatusCode::BAD_REQUEST,
                    "INVALID_REQUEST",
                    format!("`{t}` is not a tour id"),
                )
            })?);
            if storage
                .has_tour(tenant, run, id)
                .await
                .map_err(|e| storage_error(&e))?
            {
                Ok(id)
            } else {
                Err(not_found("the run has no such tour"))
            }
        }
        None => storage
            .latest_tour(tenant, run)
            .await
            .map_err(|e| storage_error(&e))?
            .ok_or_else(|| not_found("the run has no stored tour")),
    }
}

/// The tour and the caller's session through it (a new one when absent).
pub(super) async fn open(
    app: &App,
    caller: &Caller,
    run: RunId,
    id: TourId,
) -> Result<(TourArtifact, TourSession), ApiError> {
    let tenant = &caller.0.tenant_id;
    let tour = match app.storage.load_artifact(tenant, run, &tour_role(id)).await {
        Err(StorageError::ArtifactUnavailable(_)) => {
            return Err(status(
                StatusCode::CONFLICT,
                "TOUR_UNAVAILABLE",
                "the tour artifact was pruned; generate a new one",
            ));
        }
        loaded => loaded.map_err(|e| storage_error(&e))?,
    };
    let session = app
        .storage
        .tour_session(tenant, id, &caller.0.name)
        .await
        .map_err(|e| storage_error(&e))?
        .unwrap_or_else(|| TourSession::start(id, &caller.0.name));
    Ok((tour, session))
}

/// `STALE` once the run's result moved to another revision or a newer generation of the change
/// committed inputs; the tour still shows the code the review saw.
pub(super) async fn view(
    app: &App,
    caller: &Caller,
    run_view: &RunView,
    tour: TourArtifact,
    session: TourSession,
) -> Result<TourView, ApiError> {
    let run = run_of(run_view)?;
    let result = app
        .storage
        .load_result(&caller.0.tenant_id, run)
        .await
        .map_err(|e| status(StatusCode::CONFLICT, "NO_RESULT", e.to_string()))?;
    let newer = app
        .storage
        .latest(run_view.change_id)
        .await
        .map_err(|e| storage_error(&e))?
        .is_some_and(|l| l.run_id != run_view.run_id);
    let exchanges = app
        .storage
        .exchanges(&caller.0.tenant_id, tour.tour_id, &caller.0.name)
        .await
        .map_err(|e| storage_error(&e))?;
    Ok(TourView {
        exchanges,
        stale: newer || tour::stale(&tour, result.review_revision),
        tour: tour::for_session(tour, &session),
        session,
    })
}

/// `GET /v1/runs/:id/tour[?tour=]`.
pub async fn get_tour(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    Query(q): Query<TourQuery>,
) -> Result<Json<TourView>, ApiError> {
    caller.need(ApiRole::Reader)?;
    let run_view = visible_run(&app, &caller, &id).await?;
    let run = run_of(&run_view)?;
    let tour = tour_id(&app, &caller, run, q.tour.as_deref()).await?;
    let (t, session) = open(&app, &caller, run, tour).await?;
    Ok(Json(view(&app, &caller, &run_view, t, session).await?))
}

/// One change to the caller's session, saved and answered with the new view.
async fn change_session(
    app: &App,
    caller: &Caller,
    (id, tour): (&str, &str),
    endpoint: &'static str,
    headers: &HeaderMap,
    body: &[u8],
    change: impl FnOnce(&TourArtifact, TourSession) -> Result<TourSession, ApiError>,
) -> Result<Response, ApiError> {
    // A personal session is a reader's own progress, not a decision about the change.
    caller.need(ApiRole::Reader)?;
    let run_view = visible_run(app, caller, id).await?;
    let run = run_of(&run_view)?;
    let tour = tour_id(app, caller, run, Some(tour)).await?;
    idempotent(app, caller, endpoint, headers, body, || async {
        let (t, session) = open(app, caller, run, tour).await?;
        let session = change(&t, session)?;
        app.storage
            .save_tour_session(&caller.0.tenant_id, run, &session, now(app))
            .await
            .map_err(|e| storage_error(&e))?;
        Ok((
            StatusCode::OK,
            to_value(&view(app, caller, &run_view, t, session).await?)?,
        ))
    })
    .await
}

/// `POST /v1/runs/:id/tours/:tour/reveal`.
pub async fn reveal(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path((id, tour)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let request: RevealRequest = parse(&body)?;
    change_session(
        &app,
        &caller,
        (&id, &tour),
        "POST /v1/runs/:id/tours/:tour/reveal",
        &headers,
        &body,
        |t, s| {
            tour::question(t, &request.step_id)
                .map_err(|m| status(StatusCode::BAD_REQUEST, "INVALID_REQUEST", m))?;
            Ok(s.reveal(&request.step_id))
        },
    )
    .await
}

fn finish(s: TourSession, to: SessionState) -> Result<TourSession, ApiError> {
    s.finish(to)
        .map_err(|m| status(StatusCode::CONFLICT, "SESSION_FINISHED", m))
}

/// `POST /v1/runs/:id/tours/:tour/complete`: personal progress, never an approval.
pub async fn complete(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path((id, tour)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    change_session(
        &app,
        &caller,
        (&id, &tour),
        "POST /v1/runs/:id/tours/:tour/complete",
        &headers,
        &body,
        |_, s| finish(s, SessionState::Completed),
    )
    .await
}

/// `POST /v1/runs/:id/tours/:tour/dismiss`.
pub async fn dismiss(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path((id, tour)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    change_session(
        &app,
        &caller,
        (&id, &tour),
        "POST /v1/runs/:id/tours/:tour/dismiss",
        &headers,
        &body,
        |_, s| finish(s, SessionState::Dismissed),
    )
    .await
}
