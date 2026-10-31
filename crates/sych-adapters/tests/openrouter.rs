//! `OpenRouter` adapter fixtures: pinned routing, structured output request,
//! usage/cost, errors after 200, partial answers and unknown charges. Loopback only.

use serde_json::{Value, json};
use sych_adapters::openrouter::{OpenRouter, parse_reply, parse_stream, request_body};
use sych_core::config::ReasoningEffort;
use sych_core::model::{
    ErrorKind, Finish, Message, ModelProvider, ModelReply, ModelRequest, ProviderError, Speaker,
};
use sych_core::{AttemptId, MicroUsd, TaskId};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

mod common;

fn request() -> ModelRequest {
    ModelRequest {
        attempt_id: AttemptId::new_v7(),
        task_id: TaskId::new_v7(),
        model: "deepseek/deepseek-v4.1-flash".into(),
        messages: vec![
            Message {
                speaker: Speaker::System,
                content: "role".into(),
            },
            Message {
                speaker: Speaker::User,
                content: "task".into(),
            },
        ],
        schema_name: "probe".into(),
        schema: json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": { "ok": { "type": "boolean" } },
            "required": ["ok"],
            "additionalProperties": false
        }),
        max_output_tokens: Some(2_000),
        deadline: None,
    }
}

const SUCCESS: &str = r#"{
  "id": "gen-123", "model": "deepseek/deepseek-v4.1-flash-20261001", "provider": "DeepSeek",
  "choices": [{ "finish_reason": "stop", "message": { "role": "assistant", "content": "{\"ok\":true}" } }],
  "usage": { "prompt_tokens": 120, "completion_tokens": 30, "cost": 0.00014,
             "completion_tokens_details": { "reasoning_tokens": 12 } }
}"#;

/// The same reply as `SUCCESS`, streamed with a keep-alive comment and usage in the last chunk.
const STREAM: &str = concat!(
    ": OPENROUTER PROCESSING\n\n",
    "data: {\"id\":\"gen-123\",\"model\":\"deepseek/deepseek-v4.1-flash-20261001\",\"provider\":\"DeepSeek\",",
    "\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"{\\\"ok\\\"\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"gen-123\",\"choices\":[{\"delta\":{\"content\":\":true}\"},\"finish_reason\":\"stop\"}],",
    "\"usage\":{\"prompt_tokens\":120,\"completion_tokens\":30,\"cost\":0.00014,",
    "\"completion_tokens_details\":{\"reasoning_tokens\":12}}}\n\n",
    "data: [DONE]\n\n",
);

fn err(result: Result<ModelReply, ProviderError>) -> ProviderError {
    result.expect_err("provider error")
}

#[test]
fn request_pins_model_upstreams_and_parameters() {
    let body = request_body(&request(), &["deepseek".into()], None);
    assert_eq!(body["model"], "deepseek/deepseek-v4.1-flash");
    assert_eq!(
        body["provider"],
        json!({
            "order": ["deepseek"],
            "allow_fallbacks": false,
            "require_parameters": true,
            "data_collection": "deny"
        })
    );
    assert!(body.get("models").is_none(), "no model fallback list");
    assert!(
        body.get("reasoning").is_none(),
        "no reasoning field unless the profile sets one"
    );
    let low = request_body(&request(), &["deepseek".into()], Some(ReasoningEffort::Low));
    assert_eq!(low["reasoning"], json!({"effort": "low"}));
    assert_eq!(body["max_tokens"], 2_000);
    assert_eq!(body["stream"], true);
    let format = &body["response_format"];
    assert_eq!(format["type"], "json_schema");
    assert_eq!(format["json_schema"]["strict"], true);
    assert!(format["json_schema"]["schema"].get("$schema").is_none());
    assert_eq!(
        body["messages"][0],
        json!({ "role": "system", "content": "role" })
    );
}

#[test]
fn success_keeps_resolved_model_upstream_usage_and_exact_cost() {
    let reply = parse_reply(200, None, SUCCESS).expect("reply");
    assert_eq!(reply.finish, Finish::Complete);
    assert_eq!(reply.text, r#"{"ok":true}"#);
    assert_eq!(
        reply.resolved_model.as_deref(),
        Some("deepseek/deepseek-v4.1-flash-20261001")
    );
    assert_eq!(reply.upstream_provider.as_deref(), Some("DeepSeek"));
    assert_eq!(reply.generation_id.as_deref(), Some("gen-123"));
    assert_eq!(reply.usage.input_tokens, Some(120));
    assert_eq!(reply.usage.output_tokens, Some(30));
    assert_eq!(
        reply.usage.reasoning_tokens,
        Some(12),
        "reasoning is inside output"
    );
    assert_eq!(reply.reported_cost, Some(MicroUsd(140)));
}

#[test]
fn a_streamed_reply_folds_into_the_same_reply() {
    assert_eq!(
        parse_stream(200, None, STREAM).expect("reply"),
        parse_reply(200, None, SUCCESS).expect("reply")
    );
}

#[test]
fn a_cut_or_failing_stream_is_a_dispatched_failure_with_its_generation() {
    let cut = &STREAM[..STREAM
        .find("data: {\"id\":\"gen-123\",\"choices\"")
        .expect("second chunk")];
    let e = err(parse_stream(200, None, cut));
    assert_eq!((e.kind, e.dispatched), (ErrorKind::Transient, true));
    assert!(e.message.contains("generation gen-123"), "{}", e.message);
    let failed = "data: {\"id\":\"gen-7\",\"error\":{\"code\":502,\"message\":\"upstream died\"},\"choices\":[{\"delta\":{\"content\":\"\"},\"finish_reason\":\"error\"}]}\n\n";
    assert!(err(parse_stream(200, None, failed)).dispatched);
    let limited = STREAM.replace("\"stop\"", "\"length\"");
    assert_eq!(
        parse_stream(200, None, &limited).expect("reply").finish,
        Finish::Truncated
    );
    let http = err(parse_stream(
        429,
        Some(3),
        r#"{"error":{"code":429,"message":"slow"}}"#,
    ));
    assert_eq!(
        (http.kind, http.retry_after_seconds),
        (ErrorKind::RateLimited, Some(3))
    );
}

#[test]
fn missing_usage_and_cost_stay_unknown() {
    let text = r#"{"choices":[{"finish_reason":"stop","message":{"content":"{}"}}]}"#;
    let reply = parse_reply(200, None, text).expect("reply");
    assert_eq!(reply.usage.input_tokens, None);
    assert_eq!(reply.reported_cost, None);
}

#[test]
fn partial_and_refused_answers_are_not_complete() {
    let length = SUCCESS.replace(r#""finish_reason": "stop""#, r#""finish_reason": "length""#);
    assert_eq!(
        parse_reply(200, None, &length).expect("reply").finish,
        Finish::Truncated
    );
    let unknown = SUCCESS.replace(r#""finish_reason": "stop""#, r#""finish_reason": null"#);
    assert_eq!(
        parse_reply(200, None, &unknown).expect("reply").finish,
        Finish::Truncated
    );
    let filtered = SUCCESS.replace("stop", "content_filter");
    assert_eq!(
        parse_reply(200, None, &filtered).expect("reply").finish,
        Finish::Refused
    );
    let refusal =
        r#"{"choices":[{"finish_reason":"stop","message":{"content":null,"refusal":"no"}}]}"#;
    assert_eq!(
        parse_reply(200, None, refusal).expect("reply").finish,
        Finish::Refused
    );
}

#[test]
fn error_after_200_is_dispatched_and_never_a_success() {
    let mid = r#"{"id":"gen-9","error":{"code":502,"message":"upstream died"}}"#;
    let e = err(parse_reply(200, None, mid));
    assert_eq!((e.kind, e.dispatched), (ErrorKind::Transient, true));
    let choice = r#"{"choices":[{"finish_reason":"error","error":{"code":500,"message":"x"}}]}"#;
    assert!(err(parse_reply(200, None, choice)).dispatched);
    let e = err(parse_reply(200, None, "not json"));
    assert_eq!((e.kind, e.dispatched), (ErrorKind::Transient, true));
    let tool = SUCCESS.replace("stop", "tool_calls");
    assert_eq!(
        err(parse_reply(200, None, &tool)).kind,
        ErrorKind::Unsupported
    );
}

#[test]
fn http_errors_are_typed_with_dispatch_and_retry_after() {
    let body = |code: u16| format!(r#"{{"error":{{"code":{code},"message":"m"}}}}"#);
    for (status, kind, dispatched) in [
        (400, ErrorKind::Config, false),
        (401, ErrorKind::Auth, false),
        (402, ErrorKind::QuotaExhausted, false),
        (403, ErrorKind::Entitlement, false),
        (408, ErrorKind::Timeout, true),
        (429, ErrorKind::RateLimited, false),
        (502, ErrorKind::Transient, true),
        (503, ErrorKind::Unsupported, false),
    ] {
        let e = err(parse_reply(status, Some(7), &body(status)));
        assert_eq!((e.kind, e.dispatched), (kind, dispatched), "HTTP {status}");
        assert_eq!(e.retry_after_seconds, Some(7));
    }
}

#[test]
fn provider_error_text_is_redacted() {
    let body =
        r#"{"error":{"code":401,"message":"bad key sk-or-v1-0123456789abcdef0123456789abcdef"}}"#;
    let e = err(parse_reply(401, None, body));
    assert!(!e.message.contains("0123456789abcdef"), "{}", e.message);
}

#[test]
fn moving_aliases_and_missing_upstreams_are_refused() {
    let endpoint = "https://openrouter.ai/api/v1";
    for (model, upstreams) in [
        ("openrouter/auto", vec!["x".to_owned()]),
        ("deepseek/deepseek-v4.1-flash", vec![]),
    ] {
        let e = OpenRouter::new(endpoint, "k".into(), model, upstreams, None).err();
        assert_eq!(e.map(|e| e.kind), Some(ErrorKind::Config));
    }
}

/// Headers and as many body bytes as `content-length` announces, counted in bytes.
async fn serve_once(status: &'static str, body: &'static str) -> (String, JoinHandle<String>) {
    common::serve_once("/api/v1", status, body).await
}

#[tokio::test(flavor = "current_thread")]
async fn sends_bearer_auth_and_pinned_body_over_http() {
    let (endpoint, server) = serve_once("200 OK", STREAM).await;
    let adapter = OpenRouter::new(
        &endpoint,
        "test-key".into(),
        "deepseek/deepseek-v4.1-flash",
        vec!["deepseek".into()],
        None,
    )
    .expect("adapter");
    let reply = adapter.complete(&request()).await.expect("reply");
    assert_eq!(reply.reported_cost, Some(MicroUsd(140)));
    let raw = server.await.expect("server");
    assert!(raw.starts_with("POST /api/v1/chat/completions "));
    assert!(
        raw.contains("authorization: Bearer test-key")
            || raw.contains("Authorization: Bearer test-key")
    );
    let body: Value =
        serde_json::from_str(raw.split_once("\r\n\r\n").expect("body").1).expect("json");
    assert_eq!(body["provider"]["allow_fallbacks"], false);
}

#[tokio::test(flavor = "current_thread")]
async fn refused_connection_was_not_dispatched() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let endpoint = format!("http://{}/api/v1", listener.local_addr().expect("addr"));
    drop(listener);
    let adapter = OpenRouter::new(
        &endpoint,
        "k".into(),
        "deepseek/deepseek-v4.1-flash",
        vec!["deepseek".into()],
        None,
    )
    .expect("adapter");
    let e = adapter.complete(&request()).await.expect_err("no server");
    assert_eq!((e.kind, e.dispatched), (ErrorKind::Transient, false));
}

#[tokio::test(flavor = "current_thread")]
async fn rate_limit_carries_retry_after_header() {
    let body = r#"{"error":{"code":429,"message":"slow down"}}"#;
    let (endpoint, server) = serve_once("429 Too Many Requests\r\nretry-after: 12", body).await;
    let adapter = OpenRouter::new(
        &endpoint,
        "k".into(),
        "deepseek/deepseek-v4.1-flash",
        vec!["deepseek".into()],
        None,
    )
    .expect("adapter");
    let e = adapter.complete(&request()).await.expect_err("429");
    server.await.expect("server");
    assert_eq!(
        (e.kind, e.retry_after_seconds),
        (ErrorKind::RateLimited, Some(12))
    );
}
