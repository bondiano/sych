//! `/v1` reads for the dashboard: lists newest first with an id cursor, a change and a finding
//! with their history, a run's model attempts and period totals. Each one is limited to the
//! repositories the caller may read; another repository is absent from lists and 404 by id.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde::Deserialize;
use sych_core::api::{
    Attempt, ChangeDetail, ChangeSummary, FindingDetail, FindingSummary, Page, Repository, Summary,
    Usage, Waiver,
};
use sych_core::config::ApiRole;
use sych_core::{RunId, Timestamp};
use sych_storage::{ChangeFilter, FindingFilter, StorageError, Visibility};

use super::App;
use super::api::{
    Caller, idempotent, now, run_status, status, storage_error, to_value, visible_run,
};
use super::error::ApiError;

const DAY: i64 = 86_400;

/// Repository ids the caller may read; `None` for every repository of its tenant.
async fn readable(app: &App, caller: &Caller) -> Result<Option<Vec<i64>>, ApiError> {
    if caller.0.repositories.iter().any(|r| r == "*") {
        return Ok(None);
    }

    Ok(Some(
        visible_repositories(app, caller)
            .await?
            .into_iter()
            .map(|r| r.id)
            .collect(),
    ))
}

async fn visible_repositories(app: &App, caller: &Caller) -> Result<Vec<Repository>, ApiError> {
    let all = app
        .storage
        .repositories(&caller.0.tenant_id)
        .await
        .map_err(|e| storage_error(&e))?;
    Ok(all
        .into_iter()
        .filter(|r| caller.0.allows(&r.connection_id, &r.full_name))
        .collect())
}

fn visibility<'a>(caller: &'a Caller, ids: Option<&'a [i64]>) -> Visibility<'a> {
    Visibility {
        tenant_id: &caller.0.tenant_id,
        repositories: ids,
    }
}

fn invalid(message: &str) -> ApiError {
    status(
        StatusCode::BAD_REQUEST,
        "INVALID_REQUEST",
        message.to_owned(),
    )
}

fn not_found() -> ApiError {
    status(StatusCode::NOT_FOUND, "NOT_FOUND", "no such object")
}

fn limit(requested: Option<i64>) -> Result<i64, ApiError> {
    match requested {
        None => Ok(50),
        Some(l) if (1..=200).contains(&l) => Ok(l),
        Some(_) => Err(invalid("limit is 1 to 200")),
    }
}

/// `GET /v1/repositories`.
pub async fn repositories(
    State(app): State<Arc<App>>,
    caller: Caller,
) -> Result<Json<Vec<Repository>>, ApiError> {
    caller.need(ApiRole::Reader)?;
    Ok(Json(visible_repositories(&app, &caller).await?))
}

#[derive(Deserialize)]
pub struct ChangeQuery {
    repository_id: Option<i64>,
    state: Option<String>,
    before: Option<i64>,
    limit: Option<i64>,
}

/// `GET /v1/changes`.
pub async fn changes(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(q): Query<ChangeQuery>,
) -> Result<Json<Page<ChangeSummary>>, ApiError> {
    caller.need(ApiRole::Reader)?;
    let filter = ChangeFilter {
        repository_id: q.repository_id,
        state: q.state,
        before: q.before,
        limit: limit(q.limit)?,
    };
    let ids = readable(&app, &caller).await?;
    let page = app
        .storage
        .changes(visibility(&caller, ids.as_deref()), &filter)
        .await
        .map_err(|e| storage_error(&e))?;
    Ok(Json(page))
}

/// `GET /v1/changes/:id`.
pub async fn change(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<i64>,
) -> Result<Json<ChangeDetail>, ApiError> {
    caller.need(ApiRole::Reader)?;
    let ids = readable(&app, &caller).await?;
    let s = &app.storage;
    let change = s
        .change_summary(visibility(&caller, ids.as_deref()), id)
        .await
        .map_err(|e| storage_error(&e))?
        .ok_or_else(not_found)?;
    let runs = s.runs_of(id).await.map_err(|e| storage_error(&e))?;
    Ok(Json(ChangeDetail {
        change,
        runs: runs.into_iter().map(run_status).collect(),
        events: s.change_log(id).await.map_err(|e| storage_error(&e))?,
        publications: s.publications_of(id).await.map_err(|e| storage_error(&e))?,
    }))
}

#[derive(Deserialize)]
pub struct FindingQuery {
    repository_id: Option<i64>,
    change_id: Option<i64>,
    state: Option<String>,
    category: Option<String>,
    severity: Option<String>,
    rule_id: Option<String>,
    before: Option<i64>,
    limit: Option<i64>,
}

/// `GET /v1/findings`.
pub async fn findings(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(q): Query<FindingQuery>,
) -> Result<Json<Page<FindingSummary>>, ApiError> {
    caller.need(ApiRole::Reader)?;
    let filter = FindingFilter {
        repository_id: q.repository_id,
        change_id: q.change_id,
        state: q.state,
        category: q.category,
        severity: q.severity,
        rule_id: q.rule_id,
        before: q.before,
        limit: limit(q.limit)?,
    };
    let ids = readable(&app, &caller).await?;
    let page = app
        .storage
        .findings(visibility(&caller, ids.as_deref()), &filter)
        .await
        .map_err(|e| storage_error(&e))?;
    Ok(Json(page))
}

/// `GET /v1/findings/:id`: the stated details come from the latest run's immutable result.
pub async fn finding(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<i64>,
) -> Result<Json<FindingDetail>, ApiError> {
    caller.need(ApiRole::Reader)?;
    let ids = readable(&app, &caller).await?;
    let s = &app.storage;
    let summary = s
        .finding_summary(visibility(&caller, ids.as_deref()), id)
        .await
        .map_err(|e| storage_error(&e))?
        .ok_or_else(not_found)?;
    let (history, feedback) = s.finding_log(id).await.map_err(|e| storage_error(&e))?;
    let run = RunId(
        summary
            .last_run_id
            .parse()
            .map_err(|_| storage_error(&"a finding names a malformed run"))?,
    );
    let finding = match s.load_result(&caller.0.tenant_id, run).await {
        Ok(result) => result
            .findings
            .into_iter()
            .find(|f| f.id.to_string() == summary.finding_id),
        Err(StorageError::ArtifactUnavailable(_)) => None,
        Err(e) => return Err(storage_error(&e)),
    };
    Ok(Json(FindingDetail {
        summary,
        history,
        feedback,
        finding,
    }))
}

/// `GET /v1/runs/:id/attempts`.
pub async fn attempts(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<Json<Vec<Attempt>>, ApiError> {
    caller.need(ApiRole::Reader)?;
    let run = visible_run(&app, &caller, &id).await?;
    let attempts = app
        .storage
        .attempts(&caller.0.tenant_id, &run.run_id)
        .await
        .map_err(|e| storage_error(&e))?;
    Ok(Json(attempts))
}

#[derive(Deserialize)]
pub struct SummaryQuery {
    repository_id: Option<i64>,
    since: Option<i64>,
}

/// `GET /v1/summary`.
pub async fn summary(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(q): Query<SummaryQuery>,
) -> Result<Json<Summary>, ApiError> {
    caller.need(ApiRole::Reader)?;
    let at = now(&app).as_second();
    let ids = readable(&app, &caller).await?;
    let summary = app
        .storage
        .summary(
            visibility(&caller, ids.as_deref()),
            q.repository_id,
            q.since.unwrap_or(at - 30 * DAY),
            at,
        )
        .await
        .map_err(|e| storage_error(&e))?;
    Ok(Json(summary))
}

#[derive(Deserialize)]
pub struct UsageQuery {
    repository_id: Option<i64>,
    from: Option<String>,
    to: Option<String>,
}

/// `YYYY-MM-DD` of a real UTC day, or 400.
fn day(text: &str) -> Result<String, ApiError> {
    let valid = text.len() == 10 && format!("{text}T00:00:00Z").parse::<Timestamp>().is_ok();
    if valid {
        Ok(text.to_owned())
    } else {
        Err(invalid("from and to are YYYY-MM-DD"))
    }
}

/// `GET /v1/usage`: 30 days to today by default.
pub async fn usage(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(q): Query<UsageQuery>,
) -> Result<Json<Usage>, ApiError> {
    caller.need(ApiRole::Reader)?;
    let today = now(&app);
    let format = |at: i64| {
        Timestamp::from_second(at)
            .map_or_else(|_| String::new(), |t| t.strftime("%Y-%m-%d").to_string())
    };
    let from = q
        .from
        .as_deref()
        .map_or_else(|| Ok(format(today.as_second() - 29 * DAY)), day)?;
    let to =
        q.to.as_deref()
            .map_or_else(|| Ok(format(today.as_second())), day)?;
    if from > to {
        return Err(invalid("from is after to"));
    }

    let ids = readable(&app, &caller).await?;
    let s = &app.storage;
    let days = s
        .usage_days(
            visibility(&caller, ids.as_deref()),
            q.repository_id,
            &from,
            &to,
        )
        .await
        .map_err(|e| storage_error(&e))?;
    // Fees belong to the tenant, not a repository: only a caller that reads all of it sees them.
    let subscription_fees = if ids.is_none() && q.repository_id.is_none() {
        s.subscription_fees(&caller.0.tenant_id, &from[..7], &to[..7])
            .await
            .map_err(|e| storage_error(&e))?
    } else {
        Vec::new()
    };
    Ok(Json(Usage {
        from,
        to,
        days,
        subscription_fees,
    }))
}

#[derive(Deserialize)]
pub struct WaiverQuery {
    repository_id: Option<i64>,
    state: Option<String>,
}

/// `GET /v1/waivers`.
pub async fn waivers(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(q): Query<WaiverQuery>,
) -> Result<Json<Vec<Waiver>>, ApiError> {
    caller.need(ApiRole::Reader)?;
    if !matches!(
        q.state.as_deref(),
        None | Some("active" | "expired" | "revoked")
    ) {
        return Err(invalid("state is active, expired or revoked"));
    }

    let ids = readable(&app, &caller).await?;
    let waivers = app
        .storage
        .waivers(
            visibility(&caller, ids.as_deref()),
            q.repository_id,
            q.state.as_deref(),
            now(&app).as_second(),
        )
        .await
        .map_err(|e| storage_error(&e))?;
    Ok(Json(waivers))
}

/// `POST /v1/waivers/:id/revoke`: the next review no longer applies it.
pub async fn revoke(
    State(app): State<Arc<App>>,
    caller: Caller,
    Path(id): Path<i64>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    caller.need(ApiRole::Maintainer)?;
    let ids = readable(&app, &caller).await?;
    let v = visibility(&caller, ids.as_deref());
    app.storage
        .waiver(v, id)
        .await
        .map_err(|e| storage_error(&e))?
        .ok_or_else(not_found)?;
    idempotent(
        &app,
        &caller,
        "POST /v1/waivers/:id/revoke",
        &id.to_string(),
        &headers,
        &body,
        || async {
            let waiver = app
                .storage
                .revoke_waiver(v, id, now(&app).as_second())
                .await
                .map_err(|e| storage_error(&e))?
                .ok_or_else(not_found)?;
            Ok((StatusCode::OK, to_value(&waiver)?))
        },
    )
    .await
}
