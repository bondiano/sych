//! Stateless `/v1/schema` endpoints: rule catalog and schema snapshot evaluation.
//!
//! Never accepts database DSNs, never executes external adapters and never mutates storage.

use axum::Json;
use axum::body::Bytes;
use axum::http::StatusCode;
use sych_core::api::{SchemaCheckRequest, SchemaRulesResponse};
use sych_core::config::ApiRole;
use sych_core::db_schema::rules::ALL_RULES;
use sych_core::db_schema::{DbSchemaReport, RuleSpec};

use crate::commands::schema::evaluate_stateless;

use super::api::{parse, status};
use super::caller::Caller;
use super::error::ApiError;

/// `GET /v1/schema/rules`: catalogue of built-in database and application schema rules.
pub async fn rules(caller: Caller) -> Result<Json<SchemaRulesResponse>, ApiError> {
    caller.need(ApiRole::Reader)?;
    let rules = ALL_RULES.iter().map(RuleSpec::info).collect();
    Ok(Json(SchemaRulesResponse { rules }))
}

/// `POST /v1/schema/check`: evaluate a database schema snapshot against structural rules
/// and an optional application model manifest.
pub async fn check(caller: Caller, body: Bytes) -> Result<Json<DbSchemaReport>, ApiError> {
    caller.need(ApiRole::Reader)?;
    let request: SchemaCheckRequest = parse(&body)?;
    let report = evaluate_stateless(
        &request.snapshot,
        request.manifest.as_ref(),
        request.config.as_ref(),
    )
    .map_err(|msg| status(StatusCode::BAD_REQUEST, "INVALID_REQUEST", msg))?;
    Ok(Json(report))
}
