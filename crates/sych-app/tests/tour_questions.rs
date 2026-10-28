//! Follow-up questions over `/v1` (roadmap M5.3): a question is the caller's own, a risk
//! decision needs the maintainer role and records a waiver without changing the result, and a
//! comprehension question is answered from the engine's mirror under the caller's own budget.

use reqwest::StatusCode;
use serde_json::{Value, json};

mod fake_github;
mod fake_model;
mod http;
mod world;

use http::{call, listen, store_tour};
use world::{commit_result, world, world_with};

#[tokio::test(flavor = "current_thread")]
async fn questions_are_private_and_a_waiver_needs_a_maintainer() {
    let w = world(None).await;
    commit_result(&w, 1).await;
    let (run, tour, step) = store_tour(&w).await;
    let url = listen(&w).await;
    let path = format!("/v1/runs/{run}/tours/{tour}/questions");
    let ask = |who: &'static str, key: &'static str, body: Value| {
        let (url, path) = (url.clone(), path.clone());
        async move { call(&url, "POST", &path, Some(who), Some(key), &body).await }
    };
    let view = |who: &'static str| {
        let url = url.clone();
        let path = format!("/v1/runs/{run}/tour");
        async move {
            call(&url, "GET", &path, Some(who), None, &Value::Null)
                .await
                .1
        }
    };

    let fact = json!({"step_id": step, "kind": "FACT_REQUEST",
        "question": "consumers deduplicate by order id"});
    let (status, first) = ask("reader", "f1", fact.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    assert_eq!(first["reply"]["type"], "HUMAN_STATEMENT");
    assert_eq!(first["actor"], "reader");
    let (_, replay) = ask("reader", "f1", fact.clone()).await;
    assert_eq!(replay["id"], first["id"], "a replay records nothing new");
    assert_eq!(
        view("reader").await["exchanges"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(
        view("writer").await["exchanges"],
        json!([]),
        "another caller's questions stay private"
    );
    let (status, _) = ask("outsider", "f2", fact).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "out of scope");

    let terms = json!({"rule_id": "EFFECT-01", "paths": ["src/**"], "reason": "rollout window",
        "owner": "backend", "expires_at": "2027-02-01T00:00:00Z", "until": null});
    let risk = json!({"step_id": step, "kind": "RISK_DECISION",
        "question": "accept EFFECT-01 for the rollout", "waiver": terms});
    let before = w.result(0).await;
    let (status, refused) = ask("reader", "r1", risk.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    let at = world::now().as_second();
    assert!(
        w.app
            .storage
            .active_waivers(1, at)
            .await
            .expect("waivers")
            .is_empty()
    );
    let (status, decided) = ask("lead", "r2", risk.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{decided}");
    assert_eq!(decided["reply"]["type"], "RISK_DECISION");
    assert_eq!(decided["reply"]["rule_id"], "EFFECT-01");
    assert_eq!(
        w.app
            .storage
            .active_waivers(1, at)
            .await
            .expect("waivers")
            .len(),
        1
    );
    assert_eq!(w.result(0).await, before, "statuses are never rewritten");

    let mut bare = risk;
    bare["waiver"] = Value::Null;
    let (status, _) = ask("lead", "r3", bare).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a risk decision needs terms"
    );
    let mixed = json!({"step_id": step, "kind": "FACT_REQUEST", "question": "x", "waiver": terms});
    let (status, _) = ask("reader", "r4", mixed).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "terms only with a risk decision"
    );
    let (status, _) = ask(
        "reader",
        "r5",
        json!({"step_id": "nope", "kind": "FACT_REQUEST", "question": "x"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "unknown step");
}

#[tokio::test(flavor = "current_thread")]
async fn a_comprehension_question_is_answered_from_the_mirror_under_its_own_budget() {
    let model = fake_model::start();
    let w = world_with(None, Some(&model.endpoint)).await;
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    w.pr_event("d-1", "opened", &head).await;
    w.schedule(6).await;
    w.engine.work_once().await.expect("work");
    assert_eq!(w.states().await, ["COMPLETED"]);
    let (run, tour, step) = store_tour(&w).await;
    let url = listen(&w).await;
    let (status, answered) = call(
        &url,
        "POST",
        &format!("/v1/runs/{run}/tours/{tour}/questions"),
        Some("reader"),
        Some("q1"),
        &json!({"step_id": step, "kind": "COMPREHENSION", "question": "What breaks?"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{answered}");
    assert_eq!(answered["reply"]["type"], "EXPLANATION");
    assert_eq!(answered["reply"]["text"], "The caller gets the error.");
    assert_eq!(answered["reply"]["attempts"], 1);
    assert_eq!(model.calls().last().map(String::as_str), Some("followup"));
}
