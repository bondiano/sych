//! `/v1` over real HTTP for the API tests: a listening router, one call, a stored tour.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::Arc;

use reqwest::StatusCode;
use serde_json::Value;
use sych_app::server::router;
use tokio::net::TcpListener;

use super::world::World;

pub async fn listen(w: &World) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    let routes = router(Arc::clone(&w.app)).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, routes).await });
    url
}

pub async fn call(
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

/// Stores what `sych tour` would have, with a fixed draft; returns `(run, tour, first step)`.
pub async fn store_tour(w: &World) -> (String, String, String) {
    use sych_core::analysis::tour_prompt::{DraftStep, TourDraft, assemble};
    use sych_core::tour::{Question, QuestionKind, Stop, plan};
    use sych_storage::{ArtifactKind, tour_role};

    let run = w.run_ids().await[0].clone();
    let result = w.result(0).await;
    let mut plan = plan(&result, 5);
    // A clean result has no route; one stop without code still takes questions.
    if plan.stops.is_empty() {
        plan.stops.push(Stop {
            id: "stop-1".into(),
            title: "The change".into(),
            finding_ids: Vec::new(),
            obligation_ids: Vec::new(),
            code_refs: Vec::new(),
        });
    }

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
