//! `POST /hooks/github/:connection`: every check runs on the raw body before it is
//! parsed; a 2xx means the receipt and its intents are durable. No model or SCM call here.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use serde_json::json;
use sych_adapters::github::webhook::{self, Rejection};
use sych_core::sha256;
use sych_storage::{Admitted, Connection};

use super::App;
use super::error::ApiError;

fn refused(r: &Rejection) -> ApiError {
    let (status, code) = match r {
        Rejection::TooLarge(_) => (StatusCode::PAYLOAD_TOO_LARGE, "BODY_TOO_LARGE"),
        Rejection::ContentType => (StatusCode::UNSUPPORTED_MEDIA_TYPE, "CONTENT_TYPE"),
        Rejection::BadSignature => (StatusCode::UNAUTHORIZED, "BAD_SIGNATURE"),
        Rejection::MissingHeader(_) | Rejection::Event(_) | Rejection::Malformed(_) => {
            (StatusCode::BAD_REQUEST, "INVALID_DELIVERY")
        }
    };
    ApiError::new(status, code, r.to_string())
}

pub async fn github(
    State(app): State<Arc<App>>,
    Path(connection): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let Some((conn, secret)) = app.github.get(&connection) else {
        app.counters.reject("NOT_FOUND");
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            "no such connection",
        ));
    };
    let text = |name| headers.get(name).and_then(|v| v.to_str().ok());
    let check = || -> Result<_, Rejection> {
        if body.len() > conn.max_body_bytes as usize {
            return Err(Rejection::TooLarge(conn.max_body_bytes));
        }

        let json = text(header::CONTENT_TYPE.as_str())
            .is_some_and(|t| t.split(';').next() == Some("application/json"));
        if !json {
            return Err(Rejection::ContentType);
        }

        webhook::verify(
            secret.expose().as_bytes(),
            &body,
            text("x-hub-signature-256"),
        )?;
        let event = text("x-github-event").ok_or(Rejection::MissingHeader("X-GitHub-Event"))?;
        let delivery = text("x-github-delivery")
            .filter(|d| !d.is_empty() && d.len() <= 128)
            .ok_or(Rejection::MissingHeader("X-GitHub-Delivery"))?;
        Ok((delivery, webhook::parse(event, &body, &conn.bot_login)?))
    };
    let (delivery, event) = check().map_err(|r| {
        let e = refused(&r);
        app.counters.reject(e.code);
        e
    })?;
    let allowed = event
        .repository
        .as_ref()
        .is_some_and(|r| conn.allows(&r.full_name));
    let binding = Connection {
        tenant_id: conn.tenant_id.clone(),
        connection_id: conn.id.clone(),
        provider_instance: conn.provider_instance.clone(),
    };
    let admitted = app
        .storage
        .admit(
            &binding,
            delivery,
            &sha256(&body),
            &event,
            allowed,
            (app.now)(),
        )
        .await
        .map_err(|e| {
            app.counters.reject("STORAGE_UNAVAILABLE");
            // Not acknowledged: GitHub records a failed delivery and reconciliation catches up.
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "STORAGE_UNAVAILABLE",
                e.to_string(),
            )
        })?;
    let counter = match &admitted {
        Admitted::Stored { .. } => &app.counters.stored,
        Admitted::Duplicate { .. } => &app.counters.duplicate,
    };
    counter.fetch_add(1, Ordering::Relaxed);
    let (status, body) = match admitted {
        Admitted::Stored { inbox_id, intents } => (
            StatusCode::ACCEPTED,
            json!({"inbox_id": inbox_id, "duplicate": false, "intents": intents}),
        ),
        Admitted::Duplicate { inbox_id } => (
            StatusCode::OK,
            json!({"inbox_id": inbox_id, "duplicate": true}),
        ),
    };
    tracing::info!(connection, delivery, event = %event.event, status = status.as_u16(), "webhook admitted");
    Ok((status, Json(body)))
}
