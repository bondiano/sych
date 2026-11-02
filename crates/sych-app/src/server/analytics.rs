//! `GET /v1/analytics/*`: dashboard analytics over the repositories the caller may read.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use sych_core::api::{BudgetUse, CostBreakdown, Honesty, Latency, Quality, month_forecast};
use sych_core::config::{ApiRole, Budget};
use sych_storage::Spend;

use super::App;
use super::api::{Caller, now, storage_error};
use super::error::ApiError;
use super::views::{DAY, readable, visibility};

#[derive(Deserialize)]
pub struct PeriodQuery {
    repository_id: Option<i64>,
    since: Option<i64>,
}

/// `GET /v1/analytics/quality`: 30 days by default.
pub async fn quality(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(q): Query<PeriodQuery>,
) -> Result<Json<Quality>, ApiError> {
    caller.need(ApiRole::Reader)?;
    let since = q.since.unwrap_or(now(&app).as_second() - 30 * DAY);
    let ids = readable(&app, &caller).await?;
    let quality = app
        .storage
        .quality(visibility(&caller, ids.as_deref()), q.repository_id, since)
        .await
        .map_err(|e| storage_error(&e))?;
    Ok(Json(quality))
}

/// `GET /v1/analytics/honesty`: 30 days by default.
pub async fn honesty(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(q): Query<PeriodQuery>,
) -> Result<Json<Honesty>, ApiError> {
    caller.need(ApiRole::Reader)?;
    let since = q.since.unwrap_or(now(&app).as_second() - 30 * DAY);
    let ids = readable(&app, &caller).await?;
    let honesty = app
        .storage
        .honesty(visibility(&caller, ids.as_deref()), q.repository_id, since)
        .await
        .map_err(|e| storage_error(&e))?;
    Ok(Json(honesty))
}

/// `GET /v1/analytics/cost`: 30 days by default.
pub async fn cost(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(q): Query<PeriodQuery>,
) -> Result<Json<CostBreakdown>, ApiError> {
    caller.need(ApiRole::Reader)?;
    let today = now(&app);
    let since = q.since.unwrap_or(today.as_second() - 30 * DAY);
    let ids = readable(&app, &caller).await?;
    let s = &app.storage;
    let Spend {
        by_repository,
        by_model,
        by_purpose,
        top_changes,
        superseded_micro_usd,
        useful_findings,
    } = s
        .spend(visibility(&caller, ids.as_deref()), q.repository_id, since)
        .await
        .map_err(|e| storage_error(&e))?;
    // Budgets belong to the tenant, not a repository: only a caller that reads all of it.
    let budget = if ids.is_none() && q.repository_id.is_none() {
        let (day, month) = s
            .spent(
                &caller.tenant_id,
                &today.strftime("%Y-%m-%d").to_string(),
                &today.strftime("%Y-%m").to_string(),
            )
            .await
            .map_err(|e| storage_error(&e))?;
        let limit = |b: &Budget| {
            b.max_cost_micro_usd
                .map(|m| i64::try_from(m.0).unwrap_or(i64::MAX))
        };
        Some(BudgetUse {
            day_spent_micro_usd: day,
            day_limit_micro_usd: limit(&app.budgets.tenant_daily),
            month_spent_micro_usd: month,
            month_limit_micro_usd: limit(&app.budgets.tenant_monthly),
            month_forecast_micro_usd: month_forecast(month, today),
        })
    } else {
        None
    };
    Ok(Json(CostBreakdown {
        since,
        by_repository,
        by_model,
        by_purpose,
        top_changes,
        superseded_micro_usd,
        useful_findings,
        budget,
    }))
}

/// `GET /v1/analytics/latency`: 30 days by default.
pub async fn latency(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(q): Query<PeriodQuery>,
) -> Result<Json<Latency>, ApiError> {
    caller.need(ApiRole::Reader)?;
    let since = q.since.unwrap_or(now(&app).as_second() - 30 * DAY);
    let ids = readable(&app, &caller).await?;
    let [to_delivery, run_duration, queue_wait] = app
        .storage
        .latency(visibility(&caller, ids.as_deref()), q.repository_id, since)
        .await
        .map_err(|e| storage_error(&e))?;
    Ok(Json(Latency {
        since,
        to_delivery,
        run_duration,
        queue_wait,
    }))
}
