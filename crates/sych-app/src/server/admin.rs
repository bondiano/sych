//! `/v1/admin`: the tenant's connections, webhook deliveries and effective config, only for an
//! operator who maintains every repository. A person signed in through GitHub never reads
//! them: they name repositories and settings beyond that person's access.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::Value;
use sych_core::api::{ConnectionView, Delivery, Page};
use sych_core::config::{ApiRole, Environment};
use sych_storage::DeliveryFilter;

use super::App;
use super::api::{Caller, now, status, storage_error};
use super::error::ApiError;
use super::views::{limit, not_found};

const MONTH: i64 = 30 * 86_400;

/// Whether the caller may see the administration view.
pub(super) fn operator(caller: &Caller) -> bool {
    caller.reads_everything() && caller.need(ApiRole::Maintainer).is_ok()
}

fn require(caller: &Caller) -> Result<(), ApiError> {
    if operator(caller) {
        Ok(())
    } else {
        Err(status(
            StatusCode::FORBIDDEN,
            "FORBIDDEN",
            "only a maintainer client of every repository reads the administration view",
        ))
    }
}

/// The ids of the caller tenant's connections.
fn connections<'a>(app: &'a App, caller: &Caller) -> Vec<&'a str> {
    app.github
        .values()
        .filter(|(c, _)| c.tenant_id == caller.tenant_id)
        .map(|(c, _)| c.id.as_str())
        .collect()
}

/// `GET /v1/admin/connections`.
pub async fn connection_views(
    State(app): State<Arc<App>>,
    caller: Caller,
) -> Result<Json<Vec<ConnectionView>>, ApiError> {
    require(&caller)?;
    let since = now(&app).as_second() - MONTH;
    let mut out = Vec::new();
    for (c, _) in app
        .github
        .values()
        .filter(|(c, _)| c.tenant_id == caller.tenant_id)
    {
        let storage = &app.storage;
        let failure = storage.last_scm_failure(&caller.tenant_id, &c.id);
        let deliveries = storage.deliveries_by_decision(&c.id, since);
        out.push(ConnectionView {
            id: c.id.clone(),
            app_id: c.app_id,
            bot_login: c.bot_login.clone(),
            repositories: c.repositories.clone(),
            publish: c.publish,
            checks: c.checks,
            environment: match c.environment {
                Environment::Sandbox => "sandbox",
                Environment::Production => "production",
            }
            .into(),
            self_review: c.self_review,
            reviewer_commit: c.reviewer_commit.clone(),
            build_commit: crate::BUILD_COMMIT.into(),
            last_scm_read_failure: failure.await.map_err(|e| storage_error(&e))?,
            deliveries: deliveries.await.map_err(|e| storage_error(&e))?,
        });
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct DeliveryQuery {
    connection_id: Option<String>,
    decision: Option<String>,
    before: Option<i64>,
    limit: Option<i64>,
}

/// `GET /v1/admin/deliveries`.
pub async fn deliveries(
    State(app): State<Arc<App>>,
    caller: Caller,
    Query(q): Query<DeliveryQuery>,
) -> Result<Json<Page<Delivery>>, ApiError> {
    require(&caller)?;
    let limit = limit(q.limit)?;
    let own = connections(&app, &caller);
    let chosen = match q.connection_id.as_deref() {
        None => own,
        Some(id) if own.contains(&id) => vec![id],
        Some(_) => return Err(not_found()),
    };
    let filter = DeliveryFilter {
        decision: q.decision,
        before: q.before,
        limit,
    };
    let page = app
        .storage
        .deliveries(&chosen, &filter)
        .await
        .map_err(|e| storage_error(&e))?;
    Ok(Json(page))
}

/// `GET /v1/admin/config`: as `sych config validate` prints it, read once at start.
pub async fn config(State(app): State<Arc<App>>, caller: Caller) -> Result<Json<Value>, ApiError> {
    require(&caller)?;
    app.config.clone().map(Json).ok_or_else(not_found)
}
