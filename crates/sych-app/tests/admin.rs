//! The administration view over real HTTP and storage: only a maintainer client of every
//! repository reads it; connections as configured with their deliveries and latest SCM read
//! failure, the delivery log without payloads, and the effective config with secret names only.

use reqwest::StatusCode;
use serde_json::{Value, json};

mod fake_github;
mod http;
mod world;

use http::{call, listen};
use world::database::inspect;
use world::world_with;

async fn get(url: &str, who: &str, path: &str) -> (StatusCode, Value) {
    call(url, "GET", path, Some(who), None, &Value::Null).await
}

#[tokio::test(flavor = "current_thread")]
async fn only_an_operator_of_every_repository_reads_the_administration_view() {
    // A model section, so the config names a key's env variable.
    let w = world_with(None, Some("http://127.0.0.1:9/v1")).await;
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    w.pr_event("d-1", "synchronize", &head).await;
    sqlx::query(
        "INSERT INTO change_events (change_id, run_id, kind, detail, at)
         SELECT id, NULL, 'SCM_READ_FAILED', 'token: 401 Bad credentials', 1800000100 FROM changes",
    )
    .execute(&inspect(w.dir.config()).await)
    .await
    .expect("event");
    let url = listen(&w).await;

    for path in [
        "/v1/admin/connections",
        "/v1/admin/deliveries",
        "/v1/admin/config",
    ] {
        for who in ["writer", "reader"] {
            assert_eq!(
                get(&url, who, path).await.0,
                StatusCode::FORBIDDEN,
                "{who} {path}"
            );
        }
    }
    assert_eq!(get(&url, "lead", "/v1/me").await.1["admin"], json!(true));
    assert_eq!(get(&url, "writer", "/v1/me").await.1["admin"], json!(false));

    let (status, connections) = get(&url, "lead", "/v1/admin/connections").await;
    assert_eq!(status, StatusCode::OK);
    let gh = &connections[0];
    assert_eq!(
        (gh["id"].as_str(), gh["bot_login"].as_str()),
        (Some("gh"), Some("sych[bot]"))
    );
    assert!(gh["build_commit"].as_str().is_some_and(|c| !c.is_empty()));
    assert_eq!(gh["deliveries"], json!([{"key": "SCHEDULED", "count": 1}]));
    assert_eq!(
        gh["last_scm_read_failure"]["detail"],
        "token: 401 Bad credentials"
    );

    let (status, page) = get(&url, "lead", "/v1/admin/deliveries?connection_id=gh").await;
    assert_eq!(status, StatusCode::OK);
    let delivery = &page["items"][0];
    assert_eq!(
        (
            delivery["delivery_id"].as_str(),
            delivery["decision"].as_str()
        ),
        (Some("d-1"), Some("SCHEDULED"))
    );
    assert!(
        delivery.get("normalized").is_none() && delivery.get("payload_sha256").is_none(),
        "no payload: {delivery}"
    );
    let (_, none) = get(&url, "lead", "/v1/admin/deliveries?decision=IGNORED").await;
    assert_eq!(none["items"], json!([]));
    assert_eq!(
        get(&url, "lead", "/v1/admin/deliveries?connection_id=other")
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    let (status, config) = get(&url, "lead", "/v1/admin/config").await;
    assert_eq!(status, StatusCode::OK);
    assert!(config["config_hash"].is_string(), "{config}");
    assert_eq!(
        config["server"]["models"]["providers"]["openrouter"]["api_key_env"], "CARGO_PKG_NAME",
        "a secret is named by its env variable, never its value"
    );
}
