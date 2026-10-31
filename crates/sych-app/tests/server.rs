//! Webhook admission over real HTTP: raw-body signature, body limit, content
//! and event types, duplicate deliveries and the durable receipt behind every 2xx.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::{Arc, OnceLock};

use reqwest::StatusCode;
use serde_json::{Value, json};
use sych_adapters::github::Secret;
use sych_adapters::github::webhook::sign;
use sych_app::server::{App, Counters, router};
use sych_core::Timestamp;
use sych_core::config::GithubConnection;
use sych_storage::Storage;
use tokio::net::TcpListener;

#[path = "../../sych-storage/tests/support/database.rs"]
mod database;

use database::TestDir;

const SECRET: &str = "webhook-secret";

fn connection(max_body_bytes: u32) -> GithubConnection {
    serde_yaml_ng::from_str(&format!(
        "id: gh\napp_id: 1\nprivate_key_file_env: KEY\nwebhook_secret_env: HOOK\n\
         repositories: [o/r]\nbot_login: sych[bot]\nmax_body_bytes: {max_body_bytes}"
    ))
    .expect("connection")
}

fn now() -> Timestamp {
    Timestamp::from_second(1_800_000_000).expect("timestamp")
}

struct Server {
    _dir: TestDir,
    url: String,
    app: Arc<App>,
}

async fn start(max_body_bytes: u32) -> Server {
    let dir = TestDir::new().await;
    let config = dir.config();
    Storage::migrate(config).await.expect("migrate");
    let storage = Storage::open(config).await.expect("open");
    let app = Arc::new(App {
        storage,
        github: BTreeMap::from([(
            "gh".into(),
            (connection(max_body_bytes), Secret::new(SECRET.into())),
        )]),
        clients: Vec::new(),
        metrics_allow: vec![IpAddr::from([127, 0, 0, 1])],
        counters: Counters::default(),
        engine: OnceLock::new(),
        artifacts: dir.path().join("artifacts"),
        now,
        public_base_url: None,
        oauth: BTreeMap::new(),
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    let routes = router(Arc::clone(&app));
    tokio::spawn(async move { axum::serve(listener, routes).await });
    Server {
        _dir: dir,
        url,
        app,
    }
}

fn pull_request(repo: &str) -> Vec<u8> {
    json!({"action": "synchronize", "sender": {"login": "dev"}, "installation": {"id": 9},
        "repository": {"id": 42, "full_name": repo},
        "pull_request": {"number": 7, "draft": false, "merged": false,
            "head": {"sha": "abc", "repo": {"full_name": repo}},
            "base": {"repo": {"full_name": repo}}}})
    .to_string()
    .into_bytes()
}

async fn deliver(
    s: &Server,
    path: &str,
    delivery: &str,
    body: &[u8],
    signature: &str,
    content_type: &str,
    event: &str,
) -> (StatusCode, Value) {
    let reply = reqwest::Client::new()
        .post(format!("{}{path}", s.url))
        .header("content-type", content_type)
        .header("x-github-event", event)
        .header("x-github-delivery", delivery)
        .header("x-hub-signature-256", signature)
        .body(body.to_vec())
        .send()
        .await
        .expect("send");
    let status = reply.status();
    (status, reply.json().await.unwrap_or(Value::Null))
}

async fn signed(s: &Server, delivery: &str, body: &[u8]) -> (StatusCode, Value) {
    let signature = sign(SECRET.as_bytes(), body);
    deliver(
        s,
        "/hooks/github/gh",
        delivery,
        body,
        &signature,
        "application/json",
        "pull_request",
    )
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn private_metrics_do_not_relax_the_public_peer_allowlist() {
    use axum::extract::{ConnectInfo, State};
    use axum::routing::get;
    use std::net::SocketAddr;
    use sych_app::server::metrics;

    let server = start(1 << 20).await;
    let denied = metrics::metrics(
        State(Arc::clone(&server.app)),
        ConnectInfo(SocketAddr::from(([192, 0, 2, 1], 12345))),
    )
    .await;
    assert_eq!(denied, Err(StatusCode::FORBIDDEN));

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("address"));
    let routes = axum::Router::new()
        .route("/metrics", get(metrics::scrape))
        .with_state(server.app);
    let task = tokio::spawn(async move { axum::serve(listener, routes).await });
    let client = reqwest::Client::new();
    let response = client
        .get(format!("{url}/metrics"))
        .send()
        .await
        .expect("scrape");
    assert_eq!(response.status(), StatusCode::OK);
    let text = response.text().await.expect("metrics");
    assert!(text.contains("sych_artifact_bytes 0"));
    assert!(!text.contains(SECRET));
    let response = client
        .get(format!("{url}/v1/runs/1"))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    task.abort();
}

#[tokio::test(flavor = "current_thread")]
async fn a_signed_delivery_is_durable_before_2xx_and_a_repeat_is_recognized() {
    let s = start(1 << 20).await;
    let body = pull_request("o/r");
    let (status, first) = signed(&s, "d-1", &body).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{first}");
    assert_eq!(first["duplicate"], false);
    assert_eq!(
        s.app.storage.open_intents().await.expect("intents").len(),
        1
    );

    let (status, again) = signed(&s, "d-1", &body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["duplicate"], true);
    assert_eq!(again["inbox_id"], first["inbox_id"]);
    let intents = s.app.storage.open_intents().await.expect("intents");
    assert_eq!(intents[0].events, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn unsigned_oversized_or_foreign_deliveries_store_nothing() {
    let s = start(512).await;
    let body = pull_request("o/r");
    let good = sign(SECRET.as_bytes(), &body);
    let cases = [
        (
            "/hooks/github/gh",
            sign(b"wrong", &body),
            "application/json",
            "pull_request",
            StatusCode::UNAUTHORIZED,
        ),
        (
            "/hooks/github/gh",
            good.clone(),
            "text/plain",
            "pull_request",
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            "/hooks/github/gh",
            good.clone(),
            "application/json",
            "push",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/hooks/github/other",
            good.clone(),
            "application/json",
            "pull_request",
            StatusCode::NOT_FOUND,
        ),
    ];
    for (i, (path, signature, content_type, event, expected)) in cases.into_iter().enumerate() {
        let (status, reply) = deliver(
            &s,
            path,
            &format!("d-{i}"),
            &body,
            &signature,
            content_type,
            event,
        )
        .await;
        assert_eq!(status, expected, "case {i}: {reply}");
        if status != StatusCode::NOT_FOUND {
            assert!(reply["error"]["correlation_id"].is_string(), "{reply}");
            assert!(!reply.to_string().contains(SECRET));
        }
    }

    let large = [b' '; 600];
    let (status, _) = signed(&s, "d-large", &large).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(
        s.app
            .storage
            .open_intents()
            .await
            .expect("intents")
            .is_empty()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_repository_outside_the_connection_is_a_receipt_without_work() {
    let s = start(1 << 20).await;
    let (status, reply) = signed(&s, "d-1", &pull_request("someone/else")).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(reply["intents"], json!([]));
    assert!(
        s.app
            .storage
            .open_intents()
            .await
            .expect("intents")
            .is_empty()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn health_says_nothing_about_config() {
    let s = start(1 << 20).await;
    for path in ["/health/live", "/health/ready"] {
        let reply = reqwest::get(format!("{}{path}", s.url)).await.expect("get");
        assert_eq!(reply.status(), StatusCode::OK);
        let text = reply.text().await.expect("text");
        assert!(!text.contains("gh") && !text.contains(SECRET), "{text}");
    }
}
