//! Fix handoff over `/v1`: a reader exports the run's own findings in scope (`handoff@2`) by
//! explicit IDs or open-finding filters; foreign findings and mismatched snapshots are refused.

use reqwest::StatusCode;
use serde_json::json;

mod fake_github;
mod http;
mod world;

use http::{call, listen};
use world::{commit_result, world};

#[tokio::test(flavor = "current_thread")]
async fn a_handoff_exports_own_findings_and_changes_nothing() {
    let w = world(None).await;
    commit_result(&w, 1).await;
    let run = w.run_ids().await[0].clone();
    let before = w.result(0).await;
    let id = before.findings[0].id.to_string();
    let url = listen(&w).await;
    let path = format!("/v1/runs/{run}/handoff");
    let export = |who: &'static str, body: serde_json::Value| {
        let (url, path) = (url.clone(), path.clone());
        async move { call(&url, "POST", &path, Some(who), None, &body).await }
    };

    let snapshot = before.source_snapshot_id.0.to_string();
    let (status, package) = export(
        "reader",
        json!({"finding_ids": [id], "source_snapshot_id": snapshot}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{package}");
    assert_eq!(package["schema_version"], json!(2));
    assert_eq!(package["format"], json!("handoff@2"));
    assert_eq!(package["finding_ids"], json!([id]));
    assert_eq!(package["source_snapshot_id"], json!(snapshot));
    assert!(package["head"].as_str().is_some(), "{package}");
    assert!(
        package["procedure"][0]
            .as_str()
            .is_some_and(|s| s.contains("Verify")),
        "{package}"
    );
    assert!(
        package["procedure"][2]
            .as_str()
            .is_some_and(|s| s.contains("consent")),
        "{package}"
    );
    let (_, all_open) = export("reader", json!({"all": true})).await;
    assert_eq!(
        all_open, package,
        "all open matches the single open finding"
    );

    let (status, _) = export("outsider", json!({"finding_ids": [id]})).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "out of scope");
    let (status, refused) = export("reader", json!({})).await;
    assert_eq!(
        (status, refused["error"]["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("INVALID_REQUEST"))
    );
    let (status, refused) = export(
        "reader",
        json!({"finding_ids": [id, "0192f000-0000-7000-8000-000000000000"]}),
    )
    .await;
    assert_eq!(
        (status, refused["error"]["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("FOREIGN_FINDING"))
    );
    let other = format!("sha256:{}", "0".repeat(64));
    let (status, refused) = export(
        "reader",
        json!({"finding_ids": [id], "source_snapshot_id": other}),
    )
    .await;
    assert_eq!(
        (status, refused["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("SNAPSHOT_MISMATCH"))
    );
    assert_eq!(w.result(0).await, before, "the export changes nothing");
}
