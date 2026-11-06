//! The `/sych` Slack command over real HTTP: only a fresh request signed with the app's secret
//! is answered, ephemerally, and only about the repositories `slack.repositories` lists.

use reqwest::StatusCode;
use serde_json::Value;
use sych_adapters::slack::sign;
use url::form_urlencoded::byte_serialize;

mod fake_github;
mod http;
mod world;

use http::listen;
use world::{SLACK_SECRET, commit_result, now, world_server};

fn config(repositories: &str) -> String {
    format!(
        "http:\n  public_base_url: https://sych.test\nslack:\n  signing_secret_env: SLACK_SIGNING_SECRET\n  repositories: {repositories}\n"
    )
}

/// The command `text` signed with `secret` at `at`; returns the status and the reply.
async fn ask(url: &str, text: &str, secret: &str, at: i64) -> (StatusCode, Value) {
    let body = format!(
        "command=%2Fsych&text={}&user_id=U1&team_id=T1",
        byte_serialize(text.as_bytes()).collect::<String>()
    );
    let timestamp = at.to_string();
    let reply = reqwest::Client::new()
        .post(format!("{url}/slack/commands"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("x-slack-request-timestamp", &timestamp)
        .header(
            "x-slack-signature",
            sign(secret.as_bytes(), &timestamp, body.as_bytes()),
        )
        .body(body)
        .send()
        .await
        .expect("send");
    let status = reply.status();
    (status, reply.json().await.unwrap_or(Value::Null))
}

#[tokio::test(flavor = "current_thread")]
async fn a_signed_command_answers_about_a_listed_repository() {
    let w = world_server(None, None, &config("[gh/o/r]")).await;
    commit_result(&w, 3).await;
    w.engine.lineage_once().await.expect("lineage");
    let url = listen(&w).await;
    let at = now().as_second();

    let (status, help) = ask(&url, "help", SLACK_SECRET, at).await;
    assert_eq!(status, StatusCode::OK, "{help}");
    assert_eq!(help["response_type"], "ephemeral");
    assert!(
        help["text"]
            .as_str()
            .is_some_and(|t| t.contains("`/sych digest`")),
        "{help}"
    );

    let (_, change) = ask(&url, "o/r#7", SLACK_SECRET, at).await;
    let text = change["text"].as_str().unwrap_or_default();
    assert!(text.starts_with("*o/r#7*"), "{text}");
    assert!(text.contains("Latest review: COMPLETED"), "{text}");
    assert!(text.contains("Open findings: 3"), "{text}");
    assert!(text.contains("<https://sych.test/ui/changes/"), "{text}");

    let (_, digest) = ask(&url, "digest", SLACK_SECRET, at).await;
    let text = digest["text"].as_str().unwrap_or_default();
    assert!(text.starts_with("*Sych weekly report*"), "{text}");
    assert!(
        !text.contains("Tenant budget"),
        "the budget is the tenant's, not a reader's: {text}"
    );

    let (_, unknown) = ask(&url, "rerun o/r#7", SLACK_SECRET, at).await;
    assert!(
        unknown["text"]
            .as_str()
            .is_some_and(|t| t.starts_with("Not a command")),
        "{unknown}"
    );
    let (_, absent) = ask(&url, "o/r#8", SLACK_SECRET, at).await;
    assert_eq!(absent["text"], "No pull request o/r#8 here.");
}

#[tokio::test(flavor = "current_thread")]
async fn a_forged_stale_or_unlisted_request_learns_nothing() {
    let w = world_server(None, None, &config("[gh/o/other]")).await;
    commit_result(&w, 1).await;
    let url = listen(&w).await;
    let at = now().as_second();

    assert_eq!(
        ask(&url, "o/r#7", "not-the-secret", at).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        ask(&url, "o/r#7", SLACK_SECRET, at - 301).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, hidden) = ask(&url, "o/r#7", SLACK_SECRET, at).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        hidden["text"], "No pull request o/r#7 here.",
        "not in slack.repositories"
    );
}
