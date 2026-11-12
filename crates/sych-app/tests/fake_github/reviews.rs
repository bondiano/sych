//! Inline review comments, `COMMENT` reviews and check runs of the GitHub fake; every write's
//! payload is logged.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use super::{Shared, TOKEN, authorized, fault, lock};

macro_rules! guarded {
    ($state:expr, $headers:expr, $path:expr) => {{
        let mut s = lock(&$state);
        s.log.push($path.clone());
        if let Some(r) = fault(&mut s, &$path) {
            return r;
        }
        if !authorized(&$headers, TOKEN) {
            return (StatusCode::UNAUTHORIZED, Json(json!({"message": "Bad credentials"}))).into_response();
        }
        s
    }};
}

fn review_json(id: u64, c: &Value) -> Value {
    json!({"id": id, "user": {"login": "sych[bot]", "type": "Bot"}, "body": c["body"],
        "path": c["path"], "line": c["line"], "side": c["side"], "commit_id": c["commit_id"]})
}

pub async fn create_review_comment(
    State(state): State<Shared>,
    Path((o, r, n)): Path<(String, String, u64)>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let path = format!("POST /repos/{o}/{r}/pulls/{n}/comments {body}");
    let mut s = guarded!(state, headers, path);
    if s.reject_inline_line == body["line"].as_u64() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"message": "line must be part of the diff"})),
        )
            .into_response();
    }

    s.next_id += 1;
    let id = s.next_id;
    s.review_comments.insert(id, body.clone());
    (StatusCode::CREATED, Json(review_json(id, &body))).into_response()
}

/// A `COMMENT` review with its comments, refused whole when one line is refused.
pub async fn create_review(
    State(state): State<Shared>,
    Path((o, r, n)): Path<(String, String, u64)>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let path = format!("POST /repos/{o}/{r}/pulls/{n}/reviews {body}");
    let mut s = guarded!(state, headers, path);
    let comments = body["comments"].as_array().cloned().unwrap_or_default();
    if comments
        .iter()
        .any(|c| s.reject_inline_line.is_some() && s.reject_inline_line == c["line"].as_u64())
    {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"message": "line must be part of the diff"})),
        )
            .into_response();
    }

    s.next_id += 1;
    let review = s.next_id;
    for mut c in comments {
        s.next_id += 1;
        let id = s.next_id;
        c["commit_id"] = body["commit_id"].clone();
        c["review"] = json!(review);
        s.review_comments.insert(id, c);
    }
    (
        StatusCode::OK,
        Json(json!({"id": review, "state": "COMMENTED"})),
    )
        .into_response()
}

pub async fn list_review_comments_of(
    State(state): State<Shared>,
    Path((o, r, n, review)): Path<(String, String, u64, u64)>,
    headers: HeaderMap,
) -> Response {
    let path = format!("GET /repos/{o}/{r}/pulls/{n}/reviews/{review}/comments");
    let s = guarded!(state, headers, path);
    let of: Vec<Value> = s
        .review_comments
        .iter()
        .filter(|(_, c)| c["review"] == review)
        .map(|(id, c)| review_json(*id, c))
        .collect();
    Json(of).into_response()
}

pub async fn list_review_comments(
    State(state): State<Shared>,
    Path((o, r, n)): Path<(String, String, u64)>,
    headers: HeaderMap,
) -> Response {
    let path = format!("GET /repos/{o}/{r}/pulls/{n}/comments");
    let s = guarded!(state, headers, path);
    let all: Vec<Value> = s
        .review_comments
        .iter()
        .map(|(id, c)| review_json(*id, c))
        .collect();
    Json(all).into_response()
}

pub async fn review_comment(
    State(state): State<Shared>,
    Path((o, r, id)): Path<(String, String, u64)>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Response {
    let path = format!("/repos/{o}/{r}/pulls/comments/{id}");
    let mut s = guarded!(state, headers, path);
    let Some(comment) = s.review_comments.get_mut(&id) else {
        return (StatusCode::NOT_FOUND, Json(json!({"message": "Not Found"}))).into_response();
    };
    if let Some(Json(update)) = body {
        comment["body"] = update["body"].clone();
    }

    Json(review_json(id, comment)).into_response()
}

pub async fn create_check_run(
    State(state): State<Shared>,
    Path((o, r)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let path = format!("POST /repos/{o}/{r}/check-runs {body}");
    let mut s = guarded!(state, headers, path);
    s.next_id += 1;
    let id = s.next_id;
    let run = json!({"id": id, "app": {"id": 1}, "external_id": body["external_id"],
        "head_sha": body["head_sha"], "conclusion": body["conclusion"], "output": body["output"]});
    s.check_runs.push(run.clone());
    (StatusCode::CREATED, Json(run)).into_response()
}

pub async fn update_check_run(
    State(state): State<Shared>,
    Path((o, r, id)): Path<(String, String, u64)>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let path = format!("PATCH /repos/{o}/{r}/check-runs/{id} {body}");
    let mut s = guarded!(state, headers, path);
    let Some(run) = s.check_runs.iter_mut().find(|c| c["id"] == id) else {
        return (StatusCode::NOT_FOUND, Json(json!({"message": "Not Found"}))).into_response();
    };
    run["conclusion"] = body["conclusion"].clone();
    run["output"] = body["output"].clone();
    Json(run.clone()).into_response()
}

pub async fn list_check_runs(
    State(state): State<Shared>,
    Path((o, r, sha)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    let path = format!("GET /repos/{o}/{r}/commits/{sha}/check-runs");
    let s = guarded!(state, headers, path);
    let runs: Vec<Value> = s
        .check_runs
        .iter()
        .filter(|c| c["head_sha"] == sha.as_str())
        .cloned()
        .collect();
    Json(json!({"total_count": runs.len(), "check_runs": runs})).into_response()
}
