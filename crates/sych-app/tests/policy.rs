//! A repository's policy over `/v1`: a reader of the repository gets the policy of its newest
//! run, read again from the mirror at the pinned source, without the server layer; a repository
//! without such a run has none, and a mirror without the commit is a conflict.

use std::fs;

use reqwest::StatusCode;
use serde_json::Value;
use sych_app::server::mirrors;

mod fake_github;
mod http;
mod world;

use http::{call, listen};
use world::world;

const POLICY: &str = "schema_version: 1\n";

async fn get(url: &str, who: Option<&str>, path: &str) -> (StatusCode, Value) {
    call(url, "GET", path, who, None, &Value::Null).await
}

#[tokio::test(flavor = "current_thread")]
async fn a_reader_sees_the_policy_of_the_newest_run_at_its_source() {
    let w = world(Some(POLICY)).await;
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    w.pr_event("d-1", "opened", &head).await;
    w.schedule(6).await;
    let url = listen(&w).await;
    let (_, repositories) = get(&url, Some("reader"), "/v1/repositories").await;
    let id = repositories[0]["id"].as_i64().expect("repository id");
    let path = format!("/v1/repositories/{id}/policy");

    let (status, body) = get(&url, Some("reader"), &path).await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (StatusCode::NOT_FOUND, Some("NO_RUN")),
        "a queued run pinned no policy yet: {body}"
    );

    assert!(w.engine.work_once().await.expect("work"));
    let (status, view) = get(&url, Some("reader"), &path).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["repository"]["full_name"], "o/r");
    assert_eq!(view["run_id"], w.run_ids().await[0]);
    let source = &view["snapshot"]["source"];
    assert_eq!(source["kind"], "trusted_ref");
    assert_eq!(
        source["commit"],
        format!("sha1:{}", w.base),
        "the target tip"
    );
    assert_eq!(view["matches_snapshot"], true);
    let effective = &view["effective"];
    assert_eq!(effective["rules_hash"], view["snapshot"]["rules_hash"]);
    assert!(effective.get("server").is_none(), "no server layer");
    assert!(
        effective["repository"].get("contract_texts").is_none(),
        "contracts only by hash"
    );
    let layers = effective["provenance"].as_array().expect("provenance");
    assert!(layers.iter().all(|l| l["layer"] != "server"), "{layers:?}");
    assert!(
        layers.iter().any(|l| l["layer"] == "repository"
            && l["origin"]
                .as_str()
                .is_some_and(|o| o.starts_with(".sych/config.yaml@"))),
        "{layers:?}"
    );

    assert_eq!(
        get(&url, Some("outsider"), &path).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(get(&url, None, &path).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        get(&url, Some("reader"), "/v1/repositories/999/policy")
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    let mirror = mirrors::path(&w.app.artifacts, "gh", "o/r");
    fs::remove_dir_all(&mirror).expect("prune the mirror");
    let (status, body) = get(&url, Some("reader"), &path).await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("POLICY_UNAVAILABLE")),
        "{body}"
    );
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(!message.contains(&*mirror.to_string_lossy()), "{message}");
}
