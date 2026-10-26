//! An in-memory GitHub REST stand-in on loopback: the endpoints the adapter uses, a request log
//! and fault switches. It checks the bearer token, not the App JWT signature (the adapter's
//! unit test verifies that); a deterministic transport fake, not the real GitHub smoke.
#![allow(dead_code, clippy::many_single_char_names)]

mod reviews;

use std::collections::BTreeMap;
use std::mem;
use std::sync::{Arc, Mutex, MutexGuard};

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use serde_json::{Value, json};
use tokio::net::TcpListener;

pub const TOKEN: &str = "ghs_installation";

#[derive(Default)]
pub struct Fake {
    pub pulls: BTreeMap<u64, Value>,
    pub files: BTreeMap<u64, Vec<Value>>,
    pub permissions: BTreeMap<String, String>,
    pub installation_permissions: Value,
    /// Issue comments: id → (author login, body).
    pub comments: BTreeMap<u64, (String, String)>,
    pub next_id: u64,
    pub tokens_issued: u32,
    pub log: Vec<String>,
    /// Reply with this status to the next request whose path contains the key, once.
    pub fail_next: BTreeMap<String, (u16, Value)>,
    /// Apply the next write, then answer 502 as if the reply was lost.
    pub lose_next_write_reply: bool,
    /// Actions: workflow runs, jobs by run id, artifacts by run id, zip bytes by artifact id.
    pub runs: Vec<Value>,
    pub jobs: BTreeMap<u64, Vec<Value>>,
    pub artifacts: BTreeMap<u64, Vec<Value>>,
    pub blobs: BTreeMap<u64, Vec<u8>>,
    /// The fake's own base URL, for download redirects.
    pub base: String,
    /// Inline review comments by id, as created (their payload).
    pub review_comments: BTreeMap<u64, Value>,
    pub check_runs: Vec<Value>,
    /// Refuse an inline comment on this line with 422, as GitHub does outside the diff.
    pub reject_inline_line: Option<u64>,
}

pub type Shared = Arc<Mutex<Fake>>;

pub fn lock(s: &Shared) -> MutexGuard<'_, Fake> {
    s.lock().expect("fake lock")
}

pub(crate) fn authorized(headers: &HeaderMap, token: &str) -> bool {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == format!("Bearer {token}"))
}

pub(crate) fn fault(s: &mut Fake, path: &str) -> Option<Response> {
    let key = s
        .fail_next
        .keys()
        .find(|k| path.contains(k.as_str()))?
        .clone();
    let (status, body) = s.fail_next.remove(&key)?;
    let status = StatusCode::from_u16(status).expect("status");
    let mut response = (status, Json(body)).into_response();
    if status == StatusCode::TOO_MANY_REQUESTS {
        response
            .headers_mut()
            .insert("retry-after", "7".parse().expect("header"));
    }

    Some(response)
}

macro_rules! guard {
    ($state:expr, $headers:expr, $path:expr, $token:expr) => {{
        let mut s = lock(&$state);
        s.log.push($path.clone());
        if let Some(r) = fault(&mut s, &$path) {
            return r;
        }
        if !authorized(&$headers, $token) {
            return (StatusCode::UNAUTHORIZED, Json(json!({"message": "Bad credentials"})))
                .into_response();
        }
        s
    }};
}

async fn installation(
    State(state): State<Shared>,
    Path((o, r)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let path = format!("GET /repos/{o}/{r}/installation");
    let mut s = lock(&state);
    s.log.push(path.clone());
    if let Some(r) = fault(&mut s, &path) {
        return r;
    }

    let jwt = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| t.split('.').count() == 3);
    if !jwt {
        return (StatusCode::UNAUTHORIZED, Json(json!({"message": "JWT"}))).into_response();
    }

    Json(json!({"id": 9, "permissions": s.installation_permissions})).into_response()
}

async fn access_token(
    State(state): State<Shared>,
    Path(id): Path<u64>,
    Json(body): Json<Value>,
) -> Response {
    let mut s = lock(&state);
    s.log
        .push(format!("POST /app/installations/{id}/access_tokens {body}"));
    s.tokens_issued += 1;
    Json(json!({"token": TOKEN, "expires_at": "2027-01-15T10:00:00Z"})).into_response()
}

async fn repository(
    State(state): State<Shared>,
    Path((o, r)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let path = format!("GET /repos/{o}/{r}");
    let _s = guard!(state, headers, path, TOKEN);
    Json(json!({"id": 42, "full_name": format!("{o}/{r}")})).into_response()
}

async fn pull(
    State(state): State<Shared>,
    Path((o, r, n)): Path<(String, String, u64)>,
    headers: HeaderMap,
) -> Response {
    let path = format!("GET /repos/{o}/{r}/pulls/{n}");
    let s = guard!(state, headers, path, TOKEN);
    match s.pulls.get(&n) {
        Some(p) => Json(p.clone()).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({"message": "Not Found"}))).into_response(),
    }
}

fn page(items: &[Value], q: &BTreeMap<String, String>) -> Vec<Value> {
    let per: usize = q.get("per_page").and_then(|v| v.parse().ok()).unwrap_or(30);
    let page: usize = q.get("page").and_then(|v| v.parse().ok()).unwrap_or(1);
    items
        .iter()
        .skip((page - 1) * per)
        .take(per)
        .cloned()
        .collect()
}

async fn files(
    State(state): State<Shared>,
    Path((o, r, n)): Path<(String, String, u64)>,
    Query(q): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let path = format!("GET /repos/{o}/{r}/pulls/{n}/files");
    let s = guard!(state, headers, path, TOKEN);
    // GitHub stops listing at 3000 files.
    let all: Vec<Value> = s.files.get(&n).cloned().unwrap_or_default();
    let listed = &all[..all.len().min(3000)];
    Json(page(listed, &q)).into_response()
}

async fn pulls(
    State(state): State<Shared>,
    Path((o, r)): Path<(String, String)>,
    Query(q): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let path = format!("GET /repos/{o}/{r}/pulls");
    let s = guard!(state, headers, path, TOKEN);
    let open: Vec<Value> = s
        .pulls
        .values()
        .filter(|p| p["state"] == "open")
        .cloned()
        .collect();
    Json(page(&open, &q)).into_response()
}

async fn permission(
    State(state): State<Shared>,
    Path((o, r, login)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    let path = format!("GET /repos/{o}/{r}/collaborators/{login}/permission");
    let s = guard!(state, headers, path, TOKEN);
    let level = s
        .permissions
        .get(&login)
        .cloned()
        .unwrap_or_else(|| "none".into());
    Json(json!({"permission": level})).into_response()
}

async fn list_comments(
    State(state): State<Shared>,
    Path((o, r, n)): Path<(String, String, u64)>,
    Query(q): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let path = format!("GET /repos/{o}/{r}/issues/{n}/comments");
    let s = guard!(state, headers, path, TOKEN);
    let all: Vec<Value> = s
        .comments
        .iter()
        .map(|(id, (login, body))| comment_json(*id, login, body))
        .collect();
    Json(page(&all, &q)).into_response()
}

fn comment_json(id: u64, login: &str, body: &str) -> Value {
    let kind = if login.ends_with("[bot]") {
        "Bot"
    } else {
        "User"
    };
    json!({"id": id, "user": {"login": login, "type": kind}, "body": body})
}

async fn get_comment(
    State(state): State<Shared>,
    Path((o, r, id)): Path<(String, String, u64)>,
    headers: HeaderMap,
) -> Response {
    let path = format!("GET /repos/{o}/{r}/issues/comments/{id}");
    let s = guard!(state, headers, path, TOKEN);
    match s.comments.get(&id) {
        Some((login, body)) => Json(comment_json(id, login, body)).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({"message": "Not Found"}))).into_response(),
    }
}

async fn create_comment(
    State(state): State<Shared>,
    Path((o, r, n)): Path<(String, String, u64)>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let path = format!("POST /repos/{o}/{r}/issues/{n}/comments");
    let mut s = guard!(state, headers, path, TOKEN);
    s.next_id += 1;
    let id = s.next_id;
    let text = body["body"].as_str().unwrap_or_default().to_owned();
    s.comments.insert(id, ("sych[bot]".into(), text.clone()));
    if mem::take(&mut s.lose_next_write_reply) {
        return (StatusCode::BAD_GATEWAY, Json(json!({"message": "lost"}))).into_response();
    }

    (
        StatusCode::CREATED,
        Json(comment_json(id, "sych[bot]", &text)),
    )
        .into_response()
}

async fn update_comment(
    State(state): State<Shared>,
    Path((o, r, id)): Path<(String, String, u64)>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let path = format!("PATCH /repos/{o}/{r}/issues/comments/{id}");
    let mut s = guard!(state, headers, path, TOKEN);
    let text = body["body"].as_str().unwrap_or_default().to_owned();
    let Some(entry) = s.comments.get_mut(&id) else {
        return (StatusCode::NOT_FOUND, Json(json!({"message": "Not Found"}))).into_response();
    };
    entry.1.clone_from(&text);
    let login = entry.0.clone();
    if mem::take(&mut s.lose_next_write_reply) {
        return (StatusCode::BAD_GATEWAY, Json(json!({"message": "lost"}))).into_response();
    }

    Json(comment_json(id, &login, &text)).into_response()
}

async fn runs(
    State(state): State<Shared>,
    Path((o, r)): Path<(String, String)>,
    Query(q): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let path = format!("GET /repos/{o}/{r}/actions/runs");
    let s = guard!(state, headers, path, TOKEN);
    let head = q.get("head_sha").cloned().unwrap_or_default();
    let runs: Vec<Value> = s
        .runs
        .iter()
        .filter(|r| r["head_sha"] == head.as_str())
        .cloned()
        .collect();
    Json(json!({"total_count": runs.len(), "workflow_runs": runs})).into_response()
}

async fn jobs(
    State(state): State<Shared>,
    Path((o, r, run, attempt)): Path<(String, String, u64, u64)>,
    headers: HeaderMap,
) -> Response {
    let path = format!("GET /repos/{o}/{r}/actions/runs/{run}/attempts/{attempt}/jobs");
    let s = guard!(state, headers, path, TOKEN);
    Json(json!({"jobs": s.jobs.get(&run).cloned().unwrap_or_default()})).into_response()
}

async fn artifacts(
    State(state): State<Shared>,
    Path((o, r, run)): Path<(String, String, u64)>,
    headers: HeaderMap,
) -> Response {
    let path = format!("GET /repos/{o}/{r}/actions/runs/{run}/artifacts");
    let s = guard!(state, headers, path, TOKEN);
    Json(json!({"artifacts": s.artifacts.get(&run).cloned().unwrap_or_default()})).into_response()
}

async fn artifact_zip(
    State(state): State<Shared>,
    Path((o, r, id)): Path<(String, String, u64)>,
    headers: HeaderMap,
) -> Response {
    let path = format!("GET /repos/{o}/{r}/actions/artifacts/{id}/zip");
    let s = guard!(state, headers, path, TOKEN);
    let location = format!("{}/blob/{id}", s.base);
    (StatusCode::FOUND, [("location", location)]).into_response()
}

/// Signed storage: answers only without a credential, as the real one ignores it.
async fn blob(State(state): State<Shared>, Path(id): Path<u64>, headers: HeaderMap) -> Response {
    let mut s = lock(&state);
    s.log.push(format!(
        "GET /blob/{id} auth={}",
        headers.contains_key("authorization")
    ));
    match s.blobs.get(&id) {
        Some(bytes) => bytes.clone().into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// A stored (uncompressed) zip of `(name, bytes)`, as a CI artifact.
pub fn stored_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let (mut out, mut central) = (Vec::new(), Vec::new());
    for (name, bytes) in entries {
        let mut crc = flate2::Crc::new();
        crc.update(bytes);
        let offset = u32::try_from(out.len()).expect("small");
        let size = u32::try_from(bytes.len()).expect("small").to_le_bytes();
        let n = u16::try_from(name.len()).expect("short").to_le_bytes();
        out.extend(0x0403_4b50_u32.to_le_bytes());
        out.extend([20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        out.extend(crc.sum().to_le_bytes());
        out.extend(size);
        out.extend(size);
        out.extend(n);
        out.extend([0, 0]);
        out.extend(name.as_bytes());
        out.extend(*bytes);
        central.extend(0x0201_4b50_u32.to_le_bytes());
        central.extend([20, 3, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        central.extend(crc.sum().to_le_bytes());
        central.extend(size);
        central.extend(size);
        central.extend(n);
        central.extend([0; 8]);
        central.extend((0o100_644_u32 << 16).to_le_bytes());
        central.extend(offset.to_le_bytes());
        central.extend(name.as_bytes());
    }
    let offset = u32::try_from(out.len()).expect("small");
    let count = u16::try_from(entries.len()).expect("few").to_le_bytes();
    let size = u32::try_from(central.len()).expect("small");
    out.extend(&central);
    out.extend(0x0605_4b50_u32.to_le_bytes());
    out.extend([0; 4]);
    out.extend(count);
    out.extend(count);
    out.extend(size.to_le_bytes());
    out.extend(offset.to_le_bytes());
    out.extend([0, 0]);
    out
}

/// Starts the fake on loopback; returns its base URL and shared state.
pub async fn start() -> (String, Shared) {
    let state: Shared = Arc::default();
    lock(&state).installation_permissions = json!({"contents": "read",
        "pull_requests": "write", "actions": "read", "metadata": "read"});
    let app = Router::new()
        .route("/repos/{o}/{r}", get(repository))
        .route("/repos/{o}/{r}/installation", get(installation))
        .route("/app/installations/{id}/access_tokens", post(access_token))
        .route("/repos/{o}/{r}/pulls", get(pulls))
        .route("/repos/{o}/{r}/pulls/{n}", get(pull))
        .route("/repos/{o}/{r}/pulls/{n}/files", get(files))
        .route(
            "/repos/{o}/{r}/collaborators/{login}/permission",
            get(permission),
        )
        .route(
            "/repos/{o}/{r}/issues/{n}/comments",
            get(list_comments).post(create_comment),
        )
        .route(
            "/repos/{o}/{r}/issues/comments/{id}",
            get(get_comment).patch(update_comment),
        )
        .route(
            "/repos/{o}/{r}/pulls/{n}/comments",
            get(reviews::list_review_comments).post(reviews::create_review_comment),
        )
        .route(
            "/repos/{o}/{r}/pulls/comments/{id}",
            get(reviews::review_comment).patch(reviews::review_comment),
        )
        .route("/repos/{o}/{r}/check-runs", post(reviews::create_check_run))
        .route(
            "/repos/{o}/{r}/check-runs/{id}",
            patch(reviews::update_check_run),
        )
        .route(
            "/repos/{o}/{r}/commits/{sha}/check-runs",
            get(reviews::list_check_runs),
        )
        .route("/repos/{o}/{r}/actions/runs", get(runs))
        .route(
            "/repos/{o}/{r}/actions/runs/{run}/attempts/{attempt}/jobs",
            get(jobs),
        )
        .route(
            "/repos/{o}/{r}/actions/runs/{run}/artifacts",
            get(artifacts),
        )
        .route(
            "/repos/{o}/{r}/actions/artifacts/{id}/zip",
            get(artifact_zip),
        )
        .route("/blob/{id}", get(blob))
        .with_state(Arc::clone(&state));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    lock(&state).base.clone_from(&url);
    tokio::spawn(async move { axum::serve(listener, app).await });
    (url, state)
}

pub fn pull_json(number: u64, head: &str, base_sha: &str) -> Value {
    json!({"number": number, "state": "open", "merged": false, "draft": false,
        "changed_files": 1,
        "head": {"sha": head, "repo": {"full_name": "o/r"}},
        "base": {"ref": "main", "sha": base_sha, "repo": {"full_name": "o/r"}}})
}
