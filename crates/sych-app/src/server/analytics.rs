//! `GET /v1/analytics/*`: dashboard analytics over the repositories the caller may read, and
//! `GET /v1/reports/weekly`, the same numbers of seven days in one document.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use sych_core::api::{
    BudgetUse, CostBreakdown, Honesty, Latency, Quality, WeeklyReport, month_forecast,
};
use sych_core::config::{ApiRole, Budget};
use sych_storage::{Spend, Visibility};

use super::App;
use super::api::{Caller, now, storage_error};
use super::error::ApiError;
use super::views::{DAY, invalid, readable};

#[derive(Deserialize)]
pub struct PeriodQuery {
    repository_id: Option<i64>,
    since: Option<i64>,
}

/// The repositories a request reads, one of them or all, and its period's start.
pub(super) struct Scope {
    tenant_id: String,
    ids: Option<Vec<i64>>,
    repository_id: Option<i64>,
    since: i64,
}

impl Scope {
    async fn of(
        app: &App,
        caller: &Caller,
        repository_id: Option<i64>,
        since: i64,
    ) -> Result<Self, ApiError> {
        caller.need(ApiRole::Reader)?;
        Ok(Self {
            tenant_id: caller.tenant_id.clone(),
            ids: readable(app, caller).await?,
            repository_id,
            since,
        })
    }

    async fn period(app: &App, caller: &Caller, q: &PeriodQuery) -> Result<Self, ApiError> {
        let since = q.since.unwrap_or(now(app).as_second() - 30 * DAY);
        Self::of(app, caller, q.repository_id, since).await
    }

    /// Every repository of the tenant, as the scheduled digest reads them.
    pub(super) fn tenant(tenant_id: &str, since: i64) -> Self {
        Self {
            tenant_id: tenant_id.to_owned(),
            ids: None,
            repository_id: None,
            since,
        }
    }

    /// Only `ids` of the tenant, as a Slack workspace reads them; no tenant budget.
    pub(super) fn repositories(tenant_id: &str, ids: Vec<i64>, since: i64) -> Self {
        Self {
            ids: Some(ids),
            ..Self::tenant(tenant_id, since)
        }
    }

    fn visibility(&self) -> Visibility<'_> {
        Visibility {
            tenant_id: &self.tenant_id,
            repositories: self.ids.as_deref(),
        }
    }
}

async fn quality_of(app: &App, s: &Scope) -> Result<Quality, ApiError> {
    app.storage
        .quality(s.visibility(), s.repository_id, s.since)
        .await
        .map_err(|e| storage_error(&e))
}

async fn honesty_of(app: &App, s: &Scope) -> Result<Honesty, ApiError> {
    app.storage
        .honesty(s.visibility(), s.repository_id, s.since)
        .await
        .map_err(|e| storage_error(&e))
}

async fn cost_of(app: &App, s: &Scope) -> Result<CostBreakdown, ApiError> {
    let today = now(app);
    let store = &app.storage;
    let Spend {
        by_repository,
        by_model,
        by_purpose,
        top_changes,
        superseded_micro_usd,
        useful_findings,
    } = store
        .spend(s.visibility(), s.repository_id, s.since)
        .await
        .map_err(|e| storage_error(&e))?;
    // Budgets belong to the tenant, not a repository: only a caller that reads all of it.
    let budget = if s.ids.is_none() && s.repository_id.is_none() {
        let (day, month) = store
            .spent(
                &s.tenant_id,
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
    Ok(CostBreakdown {
        since: s.since,
        by_repository,
        by_model,
        by_purpose,
        top_changes,
        superseded_micro_usd,
        useful_findings,
        budget,
    })
}

async fn latency_of(app: &App, s: &Scope) -> Result<Latency, ApiError> {
    let [to_delivery, run_duration, queue_wait] = app
        .storage
        .latency(s.visibility(), s.repository_id, s.since)
        .await
        .map_err(|e| storage_error(&e))?;
    Ok(Latency {
        since: s.since,
        to_delivery,
        run_duration,
        queue_wait,
    })
}

/// `GET /v1/analytics/quality`: 30 days by default.
pub async fn quality(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(q): Query<PeriodQuery>,
) -> Result<Json<Quality>, ApiError> {
    let s = Scope::period(&app, &caller, &q).await?;
    Ok(Json(quality_of(&app, &s).await?))
}

/// `GET /v1/analytics/honesty`: 30 days by default.
pub async fn honesty(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(q): Query<PeriodQuery>,
) -> Result<Json<Honesty>, ApiError> {
    let s = Scope::period(&app, &caller, &q).await?;
    Ok(Json(honesty_of(&app, &s).await?))
}

/// `GET /v1/analytics/cost`: 30 days by default.
pub async fn cost(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(q): Query<PeriodQuery>,
) -> Result<Json<CostBreakdown>, ApiError> {
    let s = Scope::period(&app, &caller, &q).await?;
    Ok(Json(cost_of(&app, &s).await?))
}

/// `GET /v1/analytics/latency`: 30 days by default.
pub async fn latency(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(q): Query<PeriodQuery>,
) -> Result<Json<Latency>, ApiError> {
    let s = Scope::period(&app, &caller, &q).await?;
    Ok(Json(latency_of(&app, &s).await?))
}

/// The weekly report of `s` up to `until`.
pub(super) async fn weekly_report(
    app: &App,
    s: &Scope,
    until: i64,
) -> Result<WeeklyReport, ApiError> {
    Ok(WeeklyReport {
        since: s.since,
        until,
        quality: quality_of(app, s).await?,
        honesty: honesty_of(app, s).await?,
        cost: cost_of(app, s).await?,
        latency: latency_of(app, s).await?,
    })
}

#[derive(Deserialize)]
pub struct ReportQuery {
    repository_id: Option<i64>,
    format: Option<String>,
}

/// `GET /v1/reports/weekly`: the seven days to now, JSON or `?format=markdown`.
pub async fn weekly(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(q): Query<ReportQuery>,
) -> Result<Response, ApiError> {
    let markdown = match q.format.as_deref() {
        None | Some("json") => false,
        Some("markdown") => true,
        Some(_) => return Err(invalid("format is json or markdown")),
    };
    let until = now(&app).as_second();
    let s = Scope::of(&app, &caller, q.repository_id, until - 7 * DAY).await?;
    let report = weekly_report(&app, &s, until).await?;
    Ok(if markdown {
        let kind = [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")];
        (kind, report.markdown()).into_response()
    } else {
        Json(report).into_response()
    })
}
