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

/// Stores what `sych tour` would have, with a fixed draft; returns `(run, tour, first step)`.
async fn store_tour(w: &World) -> (String, String, String) {
    use sych_core::analysis::tour_prompt::{DraftStep, TourDraft, assemble};
    use sych_core::tour::{Question, QuestionKind, plan};
    use sych_storage::{ArtifactKind, tour_role};

    let run = w.run_ids().await[0].clone();
    let result = w.result(0).await;
    let plan = plan(&result, 5);
    let draft = TourDraft {
        intent: "publish an order".into(),
        steps: plan
            .stops
            .iter()
            .map(|s| DraftStep {
                stop_id: s.id.clone(),
                title: format!("Stop {}", s.id),
                purpose: "where the defect is".into(),
                explanation: "the new line".into(),
                question: Some(Question {
                    kind: QuestionKind::Comprehension,
                    text: "What breaks?".into(),
                    reveal: "the caller".into(),
                }),
                example_snippets: Vec::new(),
            })
            .collect(),
    };
    let (tour, problems) = assemble(&plan, &result, draft, "fixed".into());
    assert!(problems.is_empty(), "{problems:?}");
    let s = &w.app.storage;
    let handle = s.run_handle("local", result.run_id).await.expect("run");
    s.put_artifact(
        &handle,
        &tour_role(tour.tour_id),
        ArtifactKind::Tour,
        &tour,
        sych_core::Timestamp::now(),
    )
    .await
    .expect("store");
    (run, tour.tour_id.to_string(), plan.stops[0].id.clone())
}

fn reveals(view: &Value) -> Vec<&str> {
    view["tour"]["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .map(|s| s["question"]["reveal"].as_str().expect("reveal"))
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn a_tour_session_is_personal_hides_answers_and_finishes_once() {
    let w = world(None).await;
    let head = commit_result(&w, 1).await;
    let (run, tour, step) = store_tour(&w).await;
    let url = listen(&w).await;
    let get = |who: &'static str, path: String| {
        let url = url.clone();
        async move { call(&url, "GET", &path, Some(who), None, &Value::Null).await }
    };
    let (status, view) = get("reader", format!("/v1/runs/{run}/tour")).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["tour"]["tour_id"].as_str(), Some(tour.as_str()));
    assert_eq!(view["session"]["state"].as_str(), Some("IN_PROGRESS"));
    assert_eq!(view["stale"], json!(false));
    assert!(reveals(&view).iter().all(|r| r.is_empty()), "hidden");

    let base = format!("/v1/runs/{run}/tours/{tour}");
    let (status, view) = call(
        &url,
        "POST",
        &format!("{base}/reveal"),
        Some("reader"),
        Some("r1"),
        &json!({"step_id": step}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(reveals(&view)[0], "the caller");
    assert_eq!(view["session"]["revealed"], json!([step]));
    let (status, _) = call(
        &url,
        "POST",
        &format!("{base}/reveal"),
        Some("reader"),
        Some("r2"),
        &json!({"step_id": "nope"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "unknown step");

    let (_, other) = get("writer", format!("/v1/runs/{run}/tour?tour={tour}")).await;
    assert!(reveals(&other).iter().all(|r| r.is_empty()), "own session");
    let (status, _) = get("outsider", format!("/v1/runs/{run}/tour")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "out of scope");
    let (status, _) = get(
        "reader",
        format!("/v1/runs/{run}/tour?tour=0192f000-0000-7000-8000-000000000000"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "another tour");

    let finish = |path: &'static str, key: &'static str| {
        let (url, base) = (url.clone(), base.clone());
        async move {
            call(
                &url,
                "POST",
                &format!("{base}/{path}"),
                Some("reader"),
                Some(key),
                &Value::Null,
            )
            .await
        }
    };
    let (status, view) = finish("complete", "c1").await;
    assert_eq!(
        (status, view["session"]["state"].as_str()),
        (StatusCode::OK, Some("COMPLETED"))
    );
    let (status, view) = finish("dismiss", "d1").await;
    assert_eq!(
        (status, view["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("SESSION_FINISHED"))
    );
    let (_, again) = get("reader", format!("/v1/runs/{run}/tour")).await;
    assert_eq!(again["session"]["state"].as_str(), Some("COMPLETED"));
    assert_eq!(reveals(&again)[0], "the caller", "kept after finishing");

    // A newer generation leaves the tour readable and `STALE`.
    fake_github::lock(&w.fake)
        .permissions
        .insert("dev".into(), "write".into());
    w.comment("d-9", "dev", "/sych rerun").await;
    w.schedule(1).await;
    world::commit_claimed(&w, &head, 1).await;
    let (status, view) = get("reader", format!("/v1/runs/{run}/tour")).await;
    assert_eq!(
        (status, view["stale"].clone()),
        (StatusCode::OK, json!(true))
    );
}
