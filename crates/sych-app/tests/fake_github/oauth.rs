//! The App's user authorization on the fake: a code `code-<login>` becomes the user token
//! `gho_<login>`, and that person reaches `o/r` (id 42) with their level in `permissions`.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use super::{Shared, lock};

/// The client secret the fake accepts.
pub const CLIENT_SECRET: &str = "oauth-secret-0123";

fn user(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer gho_"))
        .map(str::to_owned)
}

/// GitHub answers a refused code with 200 and `error`, not an HTTP failure.
pub async fn access_token(State(state): State<Shared>, Json(body): Json<Value>) -> Response {
    lock(&state)
        .log
        .push("POST /login/oauth/access_token".into());
    let login = body["code"].as_str().and_then(|c| c.strip_prefix("code-"));
    match (login, body["client_secret"].as_str()) {
        (Some(login), Some(CLIENT_SECRET)) => {
            Json(json!({"access_token": format!("gho_{login}"), "token_type": "bearer"}))
                .into_response()
        }
        (_, Some(CLIENT_SECRET)) => Json(json!({"error": "bad_verification_code"})).into_response(),
        _ => Json(json!({"error": "incorrect_client_credentials"})).into_response(),
    }
}

pub async fn current_user(headers: HeaderMap) -> Response {
    match user(&headers) {
        Some(login) => Json(json!({"login": login, "id": 7})).into_response(),
        None => (
            StatusCode::UNAUTHORIZED,
            Json(json!({"message": "Bad credentials"})),
        )
            .into_response(),
    }
}

pub async fn installations(headers: HeaderMap) -> Response {
    if user(&headers).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    Json(json!({"total_count": 1, "installations": [{"id": 9}]})).into_response()
}

pub async fn repositories(
    State(state): State<Shared>,
    Path(installation): Path<u64>,
    headers: HeaderMap,
) -> Response {
    let Some(login) = user(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let level = lock(&state)
        .permissions
        .get(&login)
        .cloned()
        .unwrap_or_default();
    let rank = ["read", "triage", "write", "maintain", "admin"]
        .iter()
        .position(|l| *l == level);
    let at_least = |l: usize| rank.is_some_and(|r| r >= l);
    let repos = if installation == 9 && rank.is_some() {
        vec![
            json!({"id": 42, "full_name": "o/r", "permissions": {"pull": at_least(0),
            "triage": at_least(1), "push": at_least(2), "maintain": at_least(3), "admin": at_least(4)}}),
        ]
    } else {
        Vec::new()
    };
    Json(json!({"total_count": repos.len(), "repositories": repos})).into_response()
}
