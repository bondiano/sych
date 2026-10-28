//! `/v1` contracts over real HTTP (roadmap M2.8): identity and repository authorization,
//! idempotency with replay and conflict, 202 as durable admission, the error envelope,
//! cancellation that never rewrites a committed result, and metrics for allowed peers only.

use std::net::SocketAddr;
use std::sync::Arc;

use reqwest::StatusCode;
use serde_json::{Value, json};
use sych_app::server::router;
use tokio::net::TcpListener;

mod fake_github;
mod world;

use world::{World, commit_result, world};

async fn listen(w: &World) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    let routes = router(Arc::clone(&w.app)).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, routes).await });
    url
}

async fn call(
    url: &str,
    method: &str,
    path: &str,
    who: Option<&str>,
    key: Option<&str>,
    body: &Value,
) -> (StatusCode, Value) {
    let client = reqwest::Client::new();
    let mut req = client.request(method.parse().expect("method"), format!("{url}{path}"));
    if let Some(who) = who {
        req = req.bearer_auth(format!("{who}-token-0123456789"));
    }

    if let Some(key) = key {
        req = req.header("idempotency-key", key);
    }

    if !body.is_null() {
        req = req.json(body);
    }

    let reply = req.send().await.expect("send");
    let status = reply.status();
    (status, reply.json().await.unwrap_or(Value::Null))
}

fn review(number: &str) -> Value {
    json!({"schema_version": 1, "change": {"connection_id": "gh", "repository": "o/r",
        "external_number": number}, "profile": "economical", "publish": false})
}

#[tokio::test(flavor = "current_thread")]
async fn a_review_request_is_durable_idempotent_and_scoped() {
    let w = world(None).await;
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    let url = listen(&w).await;

    let (status, body) = call(&url, "POST", "/v1/reviews", None, Some("k1"), &review("7")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "UNAUTHENTICATED");
    assert!(body["error"]["correlation_id"].is_string() && body["error"]["retryable"] == false);
    let (status, _) = call(
        &url,
        "POST",
        "/v1/reviews",
        Some("writer"),
        None,
        &review("7"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Idempotency-Key is required"
    );
    let (status, _) = call(
        &url,
        "POST",
        "/v1/reviews",
        Some("reader"),
        Some("k1"),
        &review("7"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &url,
        "POST",
        "/v1/reviews",
        Some("outsider"),
        Some("k1"),
        &review("7"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "another repository reads as absent"
    );

    let (status, first) = call(
        &url,
        "POST",
        "/v1/reviews",
        Some("writer"),
        Some("k1"),
        &review("7"),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{first}");
    assert_eq!(first["reused"], false);
    let run = first["run_id"].as_str().expect("run").to_owned();
    assert_eq!(first["status_url"], format!("/v1/runs/{run}"));
    let (status, again) = call(
        &url,
        "POST",
        "/v1/reviews",
        Some("writer"),
        Some("k1"),
        &review("7"),
    )
    .await;
    assert_eq!(
        (status, again["run_id"].as_str(), again["reused"].as_bool()),
        (StatusCode::ACCEPTED, Some(run.as_str()), Some(true))
    );
    let (status, conflict) = call(
        &url,
        "POST",
        "/v1/reviews",
        Some("writer"),
        Some("k1"),
        &review("8"),
    )
    .await;
    assert_eq!(
        (status, conflict["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("IDEMPOTENCY_CONFLICT"))
    );
    assert_eq!(
        w.app.storage.jobs_of(&run).await.expect("jobs").len(),
        1,
        "one generation"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_run_is_readable_in_scope_and_its_result_only_after_commit() {
    let w = world(None).await;
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    let url = listen(&w).await;
    let (_, first) = call(
        &url,
        "POST",
        "/v1/reviews",
        Some("writer"),
        Some("k1"),
        &review("7"),
    )
    .await;
    let run = first["run_id"].as_str().expect("run").to_owned();
    let (status, view) = call(
        &url,
        "GET",
        &format!("/v1/runs/{run}"),
        Some("reader"),
        None,
        &Value::Null,
    )
    .await;
    assert_eq!(
        (status, view["state"].as_str()),
        (StatusCode::OK, Some("QUEUED"))
    );
    let (status, _) = call(
        &url,
        "GET",
        &format!("/v1/runs/{run}/result"),
        Some("reader"),
        None,
        &Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "no result before commit");
    assert!(w.engine.work_once().await.expect("work"));
    let (status, result) = call(
        &url,
        "GET",
        &format!("/v1/runs/{run}/result"),
        Some("reader"),
        None,
        &Value::Null,
    )
    .await;
    assert_eq!(
        (status, result["run_id"].as_str()),
        (StatusCode::OK, Some(run.as_str()))
    );
    let (status, _) = call(
        &url,
        "GET",
        &format!("/v1/runs/{run}"),
        Some("outsider"),
        None,
        &Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_stops_active_runs_and_keeps_committed_ones() {
    let w = world(None).await;
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    let url = listen(&w).await;
    let (_, first) = call(
        &url,
        "POST",
        "/v1/reviews",
        Some("writer"),
        Some("a"),
        &review("7"),
    )
    .await;
    let done = first["run_id"].as_str().expect("run").to_owned();
    w.engine.work_once().await.expect("work");
    let (status, kept) = call(
        &url,
        "POST",
        &format!("/v1/runs/{done}/cancel"),
        Some("writer"),
        Some("c1"),
        &json!({}),
    )
    .await;
    assert_eq!(
        (status, kept["state"].as_str()),
        (StatusCode::OK, Some("COMPLETED"))
    );

    let (_, second) = call(
        &url,
        "POST",
        "/v1/changes/1/reruns",
        Some("writer"),
        Some("r1"),
        &json!({}),
    )
    .await;
    let active = second["run_id"].as_str().expect("run").to_owned();
    let (status, cancelled) = call(
        &url,
        "POST",
        &format!("/v1/runs/{active}/cancel"),
        Some("writer"),
        Some("c2"),
        &json!({}),
    )
    .await;
    assert_eq!(
        (status, cancelled["state"].as_str()),
        (StatusCode::OK, Some("CANCELLED"))
    );
    assert!(
        !w.engine.work_once().await.expect("work"),
        "nothing to claim"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn feedback_and_waivers_need_their_roles_and_metrics_stay_bounded() {
    let w = world(None).await;
    commit_result(&w, 1).await;
    w.engine.lineage_once().await.expect("lineage");
    let url = listen(&w).await;
    let fb =
        json!({"judgement": "FALSE_POSITIVE", "scope": "finding", "rationale": "guarded upstream"});
    let (status, _) = call(
        &url,
        "POST",
        "/v1/findings/1/feedback",
        Some("reader"),
        Some("f"),
        &fb,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &url,
        "POST",
        "/v1/findings/1/feedback",
        Some("writer"),
        Some("f"),
        &fb,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        w.app.storage.findings_of(1).await.expect("findings")[0].state,
        "DISMISSED"
    );

    let waiver = json!({"change_id": 1, "rule_id": "EFFECT-01", "paths": ["src/**"], "reason": "window",
        "owner": "backend", "expires_at": "2027-02-01T00:00:00Z", "until": null});
    let (status, _) = call(
        &url,
        "POST",
        "/v1/waivers",
        Some("writer"),
        Some("w"),
        &waiver,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, created) = call(
        &url,
        "POST",
        "/v1/waivers",
        Some("lead"),
        Some("w"),
        &waiver,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    let metrics = reqwest::get(format!("{url}/metrics"))
        .await
        .expect("get")
        .text()
        .await
        .expect("text");
    assert!(
        metrics.contains("sych_runs{state=\"COMPLETED\"} 1"),
        "{metrics}"
    );
    assert!(metrics.contains("sych_feedback{judgement=\"FALSE_POSITIVE\"} 1"));
    assert!(
        !metrics.contains("src/") && !metrics.contains("token-0123"),
        "no paths or secrets as labels"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn requested_checks_reach_the_run_and_a_malformed_one_is_refused() {
    let w = world(None).await;
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    let url = listen(&w).await;
    let mut body = review("7");
    body["checks"] = json!([{"profile": "go-test", "params": {"race": "-race"}}]);
    let (status, _) = call(
        &url,
        "POST",
        "/v1/reviews",
        Some("writer"),
        Some("c1"),
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let latest = w.app.storage.latest(1).await.expect("latest").expect("run");
    assert_eq!(
        serde_json::to_value(&latest.inputs.targeted).expect("json"),
        json!([{"profile": "go-test", "params": {"race": "-race"}}]),
        "the worker reads them from the run inputs"
    );

    body["checks"] = json!([{"profile": "go-test", "argv": ["sh"]}]);
    let (status, _) = call(
        &url,
        "POST",
        "/v1/reviews",
        Some("writer"),
        Some("c2"),
        &body,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a request cannot add arguments"
    );
}
