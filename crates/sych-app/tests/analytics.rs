//! Analytics over real HTTP and storage: a person's latest judgement counts once, findings of
//! finished pull requests settle by state, and another scope sees none of it.

use std::collections::BTreeSet;

use reqwest::StatusCode;
use serde_json::{Value, json};

mod fake_github;
mod http;
mod world;

use http::{call, listen};
use world::database::inspect;
use world::{commit_result, world};

async fn get(url: &str, who: &str, path: &str) -> (StatusCode, Value) {
    call(url, "GET", path, Some(who), None, &Value::Null).await
}

async fn judge(url: &str, who: &str, key: &str, finding: i64, judgement: &str) {
    let body = json!({"judgement": judgement, "scope": "finding", "rationale": "seen"});
    let path = format!("/v1/findings/{finding}/feedback");
    let (status, answer) = call(url, "POST", &path, Some(who), Some(key), &body).await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");
}

#[tokio::test(flavor = "current_thread")]
async fn latest_judgements_and_settled_findings_of_readable_repositories() {
    let w = world(None).await;
    commit_result(&w, 3).await;
    w.engine.lineage_once().await.expect("lineage");
    let url = listen(&w).await;
    let (_, page) = get(&url, "reader", "/v1/findings").await;
    let findings = page["items"].as_array().cloned().unwrap_or_default();
    let id = |i: usize| findings[i]["id"].as_i64().expect("id");

    judge(&url, "writer", "f1", id(0), "FALSE_POSITIVE").await;
    judge(&url, "writer", "f2", id(0), "WONT_FIX").await;
    judge(&url, "lead", "f3", id(1), "USEFUL").await;
    sqlx::query("UPDATE changes SET scm_state = 'MERGED'")
        .execute(&inspect(w.dir.config()).await)
        .await
        .expect("merge");

    let (status, quality) = get(&url, "reader", "/v1/analytics/quality?since=0").await;
    assert_eq!(status, StatusCode::OK, "{quality}");
    let judged: BTreeSet<String> = quality["judgements"]
        .as_array()
        .expect("judgements")
        .iter()
        .map(|j| format!("{} {} {}", j["repository"], j["judgement"], j["count"]))
        .collect();
    assert_eq!(
        judged,
        BTreeSet::from([
            r#""o/r" "USEFUL" 1"#.to_owned(),
            r#""o/r" "WONT_FIX" 1"#.to_owned(),
        ]),
        "a changed mind counts once: {quality}"
    );

    assert_eq!(
        quality["settled"],
        json!([{"key": "DISMISSED", "count": 1}, {"key": "OPEN", "count": 2}]),
        "{quality}"
    );
    let open: BTreeSet<String> = findings[1..]
        .iter()
        .map(|f| f["severity"].to_string())
        .collect();
    let merged = quality["merged_with_open"].as_array().expect("merged");
    assert_eq!(merged.len(), open.len(), "{quality}");
    assert!(
        merged
            .iter()
            .all(|m| m["changes"] == 1 && open.contains(&m["severity"].to_string()))
    );

    let (_, later) = get(&url, "reader", "/v1/analytics/quality?since=99999999999").await;
    assert_eq!(later["judgements"], json!([]));
    let (_, other) = get(
        &url,
        "reader",
        "/v1/analytics/quality?since=0&repository_id=999",
    )
    .await;
    assert_eq!(other["settled"], json!([]), "only the asked repository");
    let (_, recent) = get(&url, "reader", "/v1/analytics/quality").await;
    assert_eq!(recent["settled"], quality["settled"], "30 days by default");
    let (_, nothing) = get(&url, "outsider", "/v1/analytics/quality?since=0").await;
    assert_eq!(
        (
            &nothing["judgements"],
            &nothing["settled"],
            &nothing["merged_with_open"]
        ),
        (&json!([]), &json!([]), &json!([]))
    );
}
