//! `/v1`: a configured client identity on every request, repository authorization
//! by the client's scope, `Idempotency-Key` on every mutating call, and the public error
//! envelope. 202 means a durable request, not a finished review.

use std::fmt::Display;
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use sych_core::api::{
    Accepted, CancelOutcome, FeedbackRequest, PublicationStatus, RerunRequest, ReviewRequest,
    RunStatus, RunUsage, WaiverRequest, WaiverTerms,
};
use sych_core::config::{ApiRole, RuleException};
use sych_core::scm::ScmErrorKind;
use sych_core::{RunId, Timestamp, sha256};
use sych_storage::{Idempotency, RunView, StorageError};

use super::App;
pub use super::caller::Caller;
use super::error::ApiError;
use super::requests::RequestError;

pub(super) fn status(code: StatusCode, name: &'static str, message: impl Into<String>) -> ApiError {
    ApiError::new(code, name, message)
}

pub(super) fn now(app: &App) -> Timestamp {
    (app.now)()
}

pub(super) fn engine(app: &App) -> Result<&super::Engine, ApiError> {
    app.engine.get().map(AsRef::as_ref).ok_or_else(|| {
        status(
            StatusCode::SERVICE_UNAVAILABLE,
            "DEPENDENCY_UNAVAILABLE",
            "no engine in this role",
        )
    })
}

pub(super) fn storage_error(e: &dyn Display) -> ApiError {
    status(
        StatusCode::SERVICE_UNAVAILABLE,
        "DEPENDENCY_UNAVAILABLE",
        e.to_string(),
    )
}

fn request_error(e: &RequestError) -> ApiError {
    match e {
        RequestError::NotFound(m) => status(StatusCode::NOT_FOUND, "NOT_FOUND", m.clone()),
        RequestError::Refused(m) => status(StatusCode::CONFLICT, "CHANGE_CLOSED", m.clone()),
        RequestError::Scm(s) if s.kind == ScmErrorKind::RateLimited => status(
            StatusCode::TOO_MANY_REQUESTS,
            "RATE_LIMITED",
            s.message.clone(),
        ),
        RequestError::Scm(s) if s.kind == ScmErrorKind::NotFound => {
            status(StatusCode::NOT_FOUND, "NOT_FOUND", s.message.clone())
        }
        other => storage_error(other),
    }
}

/// Runs a mutating handler once per `Idempotency-Key`; a repeat replays the stored answer and a
/// reused key with another target `resource` (the path parameters) or body is 409.
pub(super) async fn idempotent<F, Fut>(
    app: &App,
    caller: &Caller,
    endpoint: &'static str,
    resource: &str,
    headers: &HeaderMap,
    body: &[u8],
    handle: F,
) -> Result<Response, ApiError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(StatusCode, Value), ApiError>>,
{
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|k| !k.is_empty() && k.len() <= 200)
        .ok_or_else(|| {
            status(
                StatusCode::BAD_REQUEST,
                "INVALID_REQUEST",
                "Idempotency-Key is required",
            )
        })?;
    let scope = (caller.tenant_id.as_str(), caller.actor.as_str(), endpoint);
    let digest = sha256(&[resource.as_bytes(), b"\0", body].concat()).to_string();
    match app
        .storage
        .idempotency_begin(scope, key, &digest, now(app))
        .await
        .map_err(|e| storage_error(&e))?
    {
        Idempotency::Conflict => Err(status(
            StatusCode::CONFLICT,
            "IDEMPOTENCY_CONFLICT",
            "the key was used with another resource or body",
        )),
        Idempotency::InProgress => Err(status(
            StatusCode::CONFLICT,
            "REQUEST_IN_PROGRESS",
            "the first request with this key is still running",
        )),
        Idempotency::Replay { status: code, body } => {
            let mut value: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            if let Some(map) = value.as_object_mut() {
                map.insert("reused".into(), json!(true));
            }

            Ok((
                StatusCode::from_u16(code).unwrap_or(StatusCode::OK),
                Json(value),
            )
                .into_response())
        }
        Idempotency::New(id) => {
            let (code, value) = handle().await?;
            app.storage
                .idempotency_finish(id, code.as_u16(), &value.to_string())
                .await
                .map_err(|e| storage_error(&e))?;
            Ok((code, Json(value)).into_response())
        }
    }
}

pub(super) fn parse<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, ApiError> {
    serde_json::from_slice(body)
        .map_err(|e| status(StatusCode::BAD_REQUEST, "INVALID_REQUEST", e.to_string()))
}

/// `POST /v1/reviews`.
pub async fn create_review(
    State(app): State<Arc<App>>,
    caller: Caller,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    caller.need(ApiRole::Writer)?;
    let request: ReviewRequest = parse(&body)?;
    let invalid = |m: &str| status(StatusCode::BAD_REQUEST, "INVALID_REQUEST", m.to_owned());
    if request.schema_version != 1 {
        return Err(invalid("schema_version must be 1"));
    }

    let number: u64 = request
        .change
        .external_number
        .parse()
        .map_err(|_| invalid("external_number is not a number"))?;
    let deep = match request.profile.as_str() {
        "economical" => false,
        "deep" => true,
        _ => return Err(invalid("profile is `economical` or `deep`")),
    };
    let c = &request.change;
    caller
        .scope(
            &app,
            Some(&c.connection_id),
            Some(&c.repository),
            ApiRole::Writer,
        )
        .await?;
    idempotent(
        &app,
        &caller,
        "POST /v1/reviews",
        "",
        &headers,
        &body,
        || async {
            let run = engine(&app)?
                .request_review(
                    &c.connection_id,
                    &c.repository,
                    number,
                    &caller.actor,
                    request.publish,
                    deep,
                    request.checks.clone(),
                )
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

pub(super) fn to_value<T: serde::Serialize>(v: &T) -> Result<Value, ApiError> {
    serde_json::to_value(v).map_err(|e| storage_error(&e))
}

pub(super) fn run_status(v: RunView) -> RunStatus {
    RunStatus {
        result_url: format!("/v1/runs/{}/result", v.run_id),
        run_id: v.run_id,
        change_id: v.change_id,
        generation: v.generation,
        state: v.state,
        stage: v.stage,
        coverage: v.coverage,
        usage: RunUsage {
            model_attempts: v.model_attempts,
            cost_known_micro_usd: v.cost_known_micro_usd,
            unknown_cost_attempts: v.unknown_cost_attempts,
        },
        failure: v.failure,
        created_at: v.created_at,
        finished_at: v.finished_at,
        parent_run_id: v.parent_run_id,
        unknown_obligations: v.unknown_obligations,
    }
}

/// A run of a repository the caller holds `role` on.
pub(super) async fn visible_run(
    app: &App,
    caller: &Caller,
    id: &str,
    role: ApiRole,
) -> Result<RunView, ApiError> {
    let view = app
        .storage
        .run_view(&caller.tenant_id, id)
        .await
        .map_err(|e| storage_error(&e))?
        .ok_or_else(|| status(StatusCode::NOT_FOUND, "NOT_FOUND", "no such object"))?;
    caller
        .scope(
            app,
            view.connection_id.as_deref(),
            view.full_name.as_deref(),
            role,
        )
        .await?;
    Ok(view)
}

/// `GET /v1/runs/:id`.
pub async fn get_run(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<Json<RunStatus>, ApiError> {
    caller.need(ApiRole::Reader)?;
    Ok(Json(run_status(
        visible_run(&app, &caller, &id, ApiRole::Reader).await?,
    )))
}

/// `GET /v1/runs/:id/result`: the immutable result, never recomputed.
pub async fn get_result(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    caller.need(ApiRole::Reader)?;
    let view = visible_run(&app, &caller, &id, ApiRole::Reader).await?;
    let run = RunId(
        view.run_id
            .parse()
            .map_err(|_| status(StatusCode::NOT_FOUND, "NOT_FOUND", "no such object"))?,
    );
    let result = app
        .storage
        .load_result(&caller.tenant_id, run)
        .await
        .map_err(|e| match e {
            StorageError::NotCompleted { .. } => {
                status(StatusCode::CONFLICT, "NO_RESULT", e.to_string())
            }
            StorageError::ArtifactUnavailable(_) => {
                status(StatusCode::CONFLICT, "RESULT_PRUNED", e.to_string())
            }
            e => storage_error(&e),
        })?;
    Ok(Json(
        serde_json::to_value(result).map_err(|e| storage_error(&e))?,
    ))
}

/// `POST /v1/runs/:id/cancel`: an active run is cancelled; a committed one keeps its state.
pub async fn cancel_run(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    caller.need(ApiRole::Writer)?;
    visible_run(&app, &caller, &id, ApiRole::Writer).await?;
    idempotent(
        &app,
        &caller,
        "POST /v1/runs/:id/cancel",
        &id,
        &headers,
        &body,
        || async {
            let state = app
                .storage
                .cancel_run_by_id(&caller.tenant_id, &id, &caller.actor, now(&app))
                .await
                .map_err(|e| storage_error(&e))?;
            Ok((
                StatusCode::OK,
                to_value(&CancelOutcome {
                    run_id: id.clone(),
                    state,
                })?,
            ))
        },
    )
    .await
}

/// `POST /v1/changes/:id/reruns`: a new generation, explicitly.
pub async fn rerun(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<i64>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    caller.need(ApiRole::Writer)?;
    let request: RerunRequest = if body.is_empty() {
        RerunRequest::default()
    } else {
        parse(&body)?
    };
    let change = app
        .storage
        .change(id)
        .await
        .map_err(|_| status(StatusCode::NOT_FOUND, "NOT_FOUND", "no such object"))?;
    if change.partition.tenant_id != caller.tenant_id {
        return Err(status(StatusCode::NOT_FOUND, "NOT_FOUND", "no such object"));
    }

    caller
        .scope(
            &app,
            Some(&change.connection_id),
            Some(&change.full_name),
            ApiRole::Writer,
        )
        .await?;
    let deep = request.profile.as_deref() == Some("deep");
    idempotent(
        &app,
        &caller,
        "POST /v1/changes/:id/reruns",
        &id.to_string(),
        &headers,
        &body,
        || async {
            let run = engine(&app)?
                .manual(id, &caller.actor, request.publish, deep, Vec::new())
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

/// `POST /v1/findings/:id/feedback`.
pub async fn feedback(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<i64>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    caller.need(ApiRole::Triage)?;
    let request: FeedbackRequest = parse(&body)?;
    if !matches!(
        request.judgement.as_str(),
        "USEFUL" | "NOT_USEFUL" | "FALSE_POSITIVE" | "WONT_FIX"
    ) {
        return Err(status(
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
            "unknown judgement",
        ));
    }

    let tenant = caller.tenant_id.clone();
    let change = app
        .storage
        .finding_change(&tenant, id)
        .await
        .map_err(|e| storage_error(&e))?
        .ok_or_else(|| status(StatusCode::NOT_FOUND, "NOT_FOUND", "no such object"))?;
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
            ApiRole::Triage,
        )
        .await?;
    idempotent(
        &app,
        &caller,
        "POST /v1/findings/:id/feedback",
        &id.to_string(),
        &headers,
        &body,
        || async {
            let at = now(&app).as_second();
            app.storage
                .record_feedback(
                    &tenant,
                    id,
                    &caller.actor,
                    &request.judgement,
                    &request.scope,
                    &request.rationale,
                    at,
                )
                .await
                .map_err(|e| storage_error(&e))?;
            Ok((
                StatusCode::CREATED,
                json!({"finding_id": id, "judgement": request.judgement}),
            ))
        },
    )
    .await
}

/// Terms a waiver can be recorded with on `change` by this caller: paths and an end, a change
/// of the caller's tenant and repositories.
pub(super) async fn check_waiver(
    app: &App,
    caller: &Caller,
    change: i64,
    terms: &WaiverTerms,
) -> Result<(), ApiError> {
    caller.need(ApiRole::Maintainer)?;
    if terms.expires_at.is_none() && terms.until.is_none() || terms.paths.is_empty() {
        return Err(status(
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
            "paths and an expiry or completion condition are required",
        ));
    }

    let row = app
        .storage
        .change(change)
        .await
        .map_err(|_| status(StatusCode::NOT_FOUND, "NOT_FOUND", "no such object"))?;
    if row.partition.tenant_id != caller.tenant_id {
        return Err(status(StatusCode::NOT_FOUND, "NOT_FOUND", "no such object"));
    }

    caller
        .scope(
            app,
            Some(&row.connection_id),
            Some(&row.full_name),
            ApiRole::Maintainer,
        )
        .await
}

/// Records a waiver [`check_waiver`] accepted; the review's statuses are never rewritten.
pub(super) async fn record_waiver(
    app: &App,
    caller: &Caller,
    change: i64,
    terms: &WaiverTerms,
) -> Result<i64, ApiError> {
    let exception = RuleException {
        paths: terms.paths.clone(),
        reason: terms.reason.clone(),
        owner: terms.owner.clone(),
        author: caller.actor.clone(),
        expires_at: terms.expires_at,
        until: terms.until.clone(),
    };
    let policy = app
        .storage
        .latest_policy(change)
        .await
        .map_err(|e| storage_error(&e))?;
    app.storage
        .add_waiver(
            &caller.tenant_id,
            change,
            &terms.rule_id,
            &exception,
            policy.as_deref(),
            now(app).as_second(),
        )
        .await
        .map_err(|e| storage_error(&e))
}

/// `POST /v1/waivers`: a scoped risk decision; statuses are never rewritten.
pub async fn waiver(
    State(app): State<Arc<App>>,
    caller: Caller,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    caller.need(ApiRole::Maintainer)?;
    let r: WaiverRequest = parse(&body)?;
    let terms = WaiverTerms {
        rule_id: r.rule_id,
        paths: r.paths,
        reason: r.reason,
        owner: r.owner,
        expires_at: r.expires_at,
        until: r.until,
    };
    check_waiver(&app, &caller, r.change_id, &terms).await?;
    idempotent(
        &app,
        &caller,
        "POST /v1/waivers",
        "",
        &headers,
        &body,
        || async {
            let id = record_waiver(&app, &caller, r.change_id, &terms).await?;
            Ok((StatusCode::CREATED, json!({"waiver_id": id})))
        },
    )
    .await
}

/// `GET /v1/publications/:id`: delivery, relevance and reconciliation kept apart.
pub async fn publication(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<i64>,
) -> Result<Json<PublicationStatus>, ApiError> {
    caller.need(ApiRole::Reader)?;
    let (tenant, change, state, relevance, remote, error) = app
        .storage
        .publication_view(id)
        .await
        .map_err(|e| storage_error(&e))?
        .filter(|v| v.0 == caller.tenant_id)
        .ok_or_else(|| status(StatusCode::NOT_FOUND, "NOT_FOUND", "no such object"))?;
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
            ApiRole::Reader,
        )
        .await?;
    let _ = tenant;
    Ok(Json(PublicationStatus {
        publication_id: id,
        delivery: state,
        relevance,
        remote_id: remote,
        last_error: error,
    }))
}
