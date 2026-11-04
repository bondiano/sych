//! The weekly digest over a local fake Slack webhook: due at its slot it is sent once, a second
//! tick sends nothing, a failed send is retried after the pause, and the deliveries are listed
//! for an operator.

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use serde_json::Value;
use tokio::net::TcpListener;

mod fake_github;
mod http;
mod world;

use http::{call, listen};
use world::{SECRETS, advance, commit_result, now, world_server};

#[derive(Clone, Default)]
struct Hook {
    bodies: Arc<Mutex<Vec<String>>>,
    status: Arc<AtomicU16>,
}

async fn receive(State(hook): State<Hook>, body: String) -> StatusCode {
    hook.bodies.lock().expect("lock").push(body);
    StatusCode::from_u16(hook.status.load(Ordering::Relaxed)).unwrap_or(StatusCode::OK)
}

/// A fake incoming webhook answering `status`; returns it and its URL.
async fn slack(status: u16) -> (Hook, String) {
    let hook = Hook::default();
    hook.status.store(status, Ordering::Relaxed);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}/hook", listener.local_addr().expect("addr"));
    let routes = Router::new()
        .route("/hook", post(receive))
        .with_state(hook.clone());
    tokio::spawn(async move { axum::serve(listener, routes).await });
    (hook, url)
}

/// A digest due now: today's weekday at the current UTC hour.
fn digest_config() -> String {
    let at = now();
    format!(
        "http:\n  public_base_url: https://sych.test\nnotifications:\n  weekly_digest:\n    weekday: {}\n    hour_utc: {}\n    slack: {{webhook_url_env: SYCH_TEST_SLACK_URL}}\n",
        at.strftime("%A").to_string().to_lowercase(),
        at.strftime("%H")
            .to_string()
            .trim_start_matches('0')
            .parse::<u8>()
            .unwrap_or(0),
    )
}

fn sent(hook: &Hook) -> Vec<String> {
    hook.bodies.lock().expect("lock").clone()
}

#[tokio::test(flavor = "current_thread")]
async fn the_digest_is_sent_once_per_slot() {
    let (hook, url) = slack(200).await;
    SECRETS.with(|s| s.borrow_mut().insert("SYCH_TEST_SLACK_URL".into(), url));
    let w = world_server(None, None, &digest_config()).await;
    commit_result(&w, 1).await;

    w.engine.digest_once().await.expect("digest");
    let bodies = sent(&hook);
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    let text: Value = serde_json::from_str(&bodies[0]).expect("json");
    let text = text["text"].as_str().unwrap_or_default();
    assert!(text.contains("Sych weekly report"), "{text}");
    assert!(text.contains("https://sych.test/ui/analytics"), "{text}");

    w.engine.digest_once().await.expect("digest");
    advance(3600);
    w.engine.digest_once().await.expect("digest");
    assert_eq!(sent(&hook).len(), 1, "one slot, one send");

    let api = listen(&w).await;
    let (status, deliveries) = call(
        &api,
        "GET",
        "/v1/admin/digests",
        Some("lead"),
        None,
        &Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{deliveries}");
    assert_eq!(
        (
            &deliveries[0]["channel"],
            &deliveries[0]["state"],
            &deliveries[0]["attempts"]
        ),
        (
            &Value::from("SLACK"),
            &Value::from("DELIVERED"),
            &Value::from(1)
        ),
        "{deliveries}"
    );
    let (status, _) = call(
        &api,
        "GET",
        "/v1/admin/digests",
        Some("reader"),
        None,
        &Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "an operator's view");
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_send_is_retried_after_a_pause_and_then_given_up() {
    let (hook, url) = slack(500).await;
    SECRETS.with(|s| s.borrow_mut().insert("SYCH_TEST_SLACK_URL".into(), url));
    let w = world_server(None, None, &digest_config()).await;

    w.engine.digest_once().await.expect("digest");
    w.engine.digest_once().await.expect("digest");
    assert_eq!(sent(&hook).len(), 1, "no retry before the pause");
    for expected in [2, 3] {
        advance(300);
        w.engine.digest_once().await.expect("digest");
        assert_eq!(sent(&hook).len(), expected);
    }
    advance(300);
    w.engine.digest_once().await.expect("digest");
    assert_eq!(sent(&hook).len(), 3, "three attempts at most");

    let api = listen(&w).await;
    let (_, deliveries) = call(
        &api,
        "GET",
        "/v1/admin/digests",
        Some("lead"),
        None,
        &Value::Null,
    )
    .await;
    assert_eq!(
        (&deliveries[0]["state"], &deliveries[0]["attempts"]),
        (&Value::from("FAILED"), &Value::from(3)),
        "{deliveries}"
    );
    assert!(
        deliveries[0]["error"]
            .as_str()
            .is_some_and(|e| e.contains("500") && !e.contains("/hook")),
        "the status without the webhook URL: {deliveries}"
    );
}
