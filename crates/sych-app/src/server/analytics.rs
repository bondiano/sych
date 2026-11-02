//! `GET /v1/analytics/*`: dashboard analytics over the repositories the caller may read.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use sych_core::api::Quality;
use sych_core::config::ApiRole;

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
