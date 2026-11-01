//! Operations over real HTTP and storage: jobs only for a writer of their repository,
//! publications that wait for a decision, and an audited CREATE again of an `UNCERTAIN` one.

use reqwest::StatusCode;
use serde_json::{Value, json};

mod fake_github;
mod http;
mod world;

use fake_github::lock;
use http::{call, listen};
use world::{World, world};

async fn get(url: &str, who: &str, path: &str) -> (StatusCode, Value) {
    call(url, "GET", path, Some(who), None, &Value::Null).await
}

/// PR 7 reviewed, and its summary sent with the reply lost: the operation is `UNCERTAIN`.
async fn uncertain() -> (World, String) {
    let w = world(None).await;
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    w.pr_event("d-1", "synchronize", &head).await;
    w.schedule(6).await;
    assert!(w.engine.work_once().await.expect("work"));
    lock(&w.fake).lose_next_write_reply = true;
    assert!(w.engine.publish_once().await.expect("publish"));
    let url = listen(&w).await;
    (w, url)
}

#[tokio::test(flavor = "current_thread")]
async fn jobs_are_listed_for_a_writer_of_their_repository() {
    let (_w, url) = uncertain().await;

    let (status, page) = get(&url, "writer", "/v1/jobs").await;
    assert_eq!(status, StatusCode::OK);
    let jobs = page["items"].as_array().cloned().unwrap_or_default();
    assert!(!jobs.is_empty());
    let job = &jobs[0];
    assert_eq!(
        (
            job["repository"]["full_name"].as_str(),
            job["number"].as_i64(),
            job["state"].as_str(),
            job["run_state"].as_str()
        ),
        (Some("o/r"), Some(7), Some("DONE"), Some("COMPLETED"))
    );
    let (_, done) = get(&url, "writer", "/v1/jobs?state=DONE&limit=1").await;
    assert_eq!(done["items"].as_array().map(Vec::len), Some(1));

    assert_eq!(
        get(&url, "reader", "/v1/jobs").await.0,
        StatusCode::FORBIDDEN,
        "a reader does not see the queue"
    );
    assert_eq!(
        get(&url, "outsider", "/v1/jobs").await.1["items"],
        json!([]),
        "a writer of another repository lists nothing"
    );
    assert_eq!(
        get(&url, "writer", "/v1/jobs?state=LOST").await.0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_uncertain_publication_is_created_again_by_an_audited_decision() {
    let (_w, url) = uncertain().await;

    let (status, list) = get(&url, "reader", "/v1/publications").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (list[0]["state"].as_str(), list[0]["number"].as_i64()),
        (Some("UNCERTAIN"), Some(7))
    );
    assert_eq!(get(&url, "outsider", "/v1/publications").await.1, json!([]));
    let id = list[0]["id"].as_i64().expect("id");
    let path = format!("/v1/publications/{id}/reconcile");
    let decision = json!({"reason": "the reply was lost; no comment on the PR"});

    let refused = call(&url, "POST", &path, Some("reader"), Some("r1"), &decision).await;
    assert_eq!(refused.0, StatusCode::FORBIDDEN);
    let hidden = call(&url, "POST", &path, Some("outsider"), Some("o1"), &decision).await;
    assert_eq!(hidden.0, StatusCode::NOT_FOUND);
    let blank = json!({"reason": " "});
    let blank = call(&url, "POST", &path, Some("writer"), Some("w0"), &blank).await;
    assert_eq!(blank.0, StatusCode::BAD_REQUEST);

    let (status, done) = call(&url, "POST", &path, Some("writer"), Some("w1"), &decision).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["delivery"], "PENDING");
    let (status, replay) = call(&url, "POST", &path, Some("writer"), Some("w1"), &decision).await;
    assert_eq!((status, &replay["reused"]), (StatusCode::OK, &json!(true)));
    let (status, again) = call(&url, "POST", &path, Some("writer"), Some("w2"), &decision).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(again["error"]["code"], "NOT_RECREATABLE");
    assert_eq!(get(&url, "reader", "/v1/publications").await.1, json!([]));

    let (_, audit) = get(
        &url,
        "lead",
        "/v1/audit?action=PUBLICATION_RECREATE_AUTHORIZED",
    )
    .await;
    let event = &audit["items"][0];
    assert_eq!(
        (event["actor"].as_str(), event["target"].as_str()),
        (
            Some("writer"),
            Some(format!("publication_operation:{id}").as_str())
        )
    );
    assert_eq!(event["metadata"]["reason"], decision["reason"]);
}
