//! The public error envelope (spec §25.3): code, message, retryable and a correlation id;
//! never tokens, prompts, source fragments or stack traces.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub details: Value,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            details: json!({}),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let correlation_id = Uuid::now_v7().to_string();
        let retryable = matches!(
            self.status,
            StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE
        );
        tracing::warn!(status = self.status.as_u16(), code = self.code, %correlation_id, message = %self.message, "request refused");
        let body = json!({"error": {
            "code": self.code,
            "message": self.message,
            "retryable": retryable,
            "correlation_id": correlation_id,
            "details": self.details,
        }});
        (self.status, Json(body)).into_response()
    }
}
