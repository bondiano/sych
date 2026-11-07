//! The playground over real HTTP: a maintainer tries a review of a pull request when a
//! playground budget is set; the run completes, is audited, lists apart and stays out of the
//! change's runs and the totals.

use reqwest::StatusCode;
use serde_json::{Value, json};

mod fake_github;
mod http;
mod world;

use http::{call, listen};
use world::{world, world_server};

const BUDGET: &str = "budgets:\n  playground_daily:\n    max_cost_micro_usd: 1000000\n";

async fn get(url: &str, who: &str, path: &str) -> (StatusCode, Value) {
    call(url, "GET", path, Some(who), None, &Value::Null).await
}

fn review() -> Value {
    json!({"schema_version": 1, "change": {"connection_id": "gh", "repository": "o/r",
        "external_number": "7"}, "profile": "economical", "publish": false})
}

#[tokio::test(flavor = "current_thread")]
async fn a_maintainer_tries_a_review_that_stays_apart_from_the_change() {
    let w = world_server(None, None, BUDGET).await;
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    let url = listen(&w).await;
    let (status, _) = call(
        &url,
        "POST",
        "/v1/reviews",
        Some("writer"),
        Some("r"),
        &review(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert!(w.engine.work_once().await.expect("work"));

    let path = "/v1/changes/1/playground";
    let (status, _) = call(&url, "POST", path, Some("writer"), Some("p0"), &json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a maintainer's try");
    let (status, started) = call(&url, "POST", path, Some("lead"), Some("p1"), &json!({})).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    let run = started["run_id"].as_str().expect("run").to_owned();
    assert!(w.engine.work_once().await.expect("work"));

    let (_, tried) = get(&url, "reader", &format!("/v1/runs/{run}")).await;
    assert_eq!(
        (tried["state"].as_str(), tried["playground"].as_bool()),
        (Some("COMPLETED"), Some(true)),
        "{tried}"
    );
    let (_, listed) = get(&url, "reader", "/v1/playground").await;
    assert_eq!(listed[0]["run_id"], run.as_str(), "{listed}");
    let (_, change) = get(&url, "reader", "/v1/changes/1").await;
    let runs = change["runs"].as_array().cloned().unwrap_or_default();
    assert_eq!(runs.len(), 1, "only the review: {runs:?}");
    assert_eq!(runs[0]["playground"], false);
    let (_, summary) = get(&url, "reader", "/v1/summary?since=0").await;
    let counted: i64 = (summary["runs_by_state"]
        .as_array()
        .cloned()
        .unwrap_or_default())
    .iter()
    .filter_map(|c| c["count"].as_i64())
    .sum();
    assert_eq!(counted, 1, "{summary}");
    let (_, audit) = get(&url, "lead", "/v1/audit?action=PLAYGROUND_REQUESTED").await;
    assert_eq!(audit["items"][0]["target"], format!("run:{run}"), "{audit}");
}

#[tokio::test(flavor = "current_thread")]
async fn without_a_playground_budget_nothing_is_tried() {
    let w = world(None).await;
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    let url = listen(&w).await;
    call(
        &url,
        "POST",
        "/v1/reviews",
        Some("writer"),
        Some("r"),
        &review(),
    )
    .await;
    let (status, body) = call(
        &url,
        "POST",
        "/v1/changes/1/playground",
        Some("lead"),
        Some("p1"),
        &json!({}),
    )
    .await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("PLAYGROUND_DISABLED"))
    );
}
