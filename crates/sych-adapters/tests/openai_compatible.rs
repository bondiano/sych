//! OpenAI-compatible adapter fixtures: the owner's endpoint and key, `max_tokens` on every
//! bounded request, usage asked for, no claimed native schema. Loopback only.

use serde_json::{Value, json};
use sych_adapters::openai_compatible::{CAPABILITIES, OpenaiCompatible, request_body};
use sych_core::model::{Message, ModelProvider, ModelRequest, Speaker};
use sych_core::{AttemptId, TaskId};

mod common;

fn request(max_output_tokens: Option<u32>) -> ModelRequest {
    ModelRequest {
        attempt_id: AttemptId::new_v7(),
        task_id: TaskId::new_v7(),
        model: "qwen3-coder".into(),
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
        schema: json!({ "type": "object" }),
        max_output_tokens,
        deadline: None,
        reasoning_off: false,
    }
}

const REPLY: &str = concat!(
    "data: {\"id\":\"c-1\",\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"{\\\"ok\\\":true}\"},\"finish_reason\":\"stop\"}]}\n\n",
    "data: {\"id\":\"c-1\",\"choices\":[],\"usage\":{\"prompt_tokens\":90,\"completion_tokens\":12,\"cost\":0}}\n\n",
    "data: [DONE]\n\n",
);

#[test]
fn a_bounded_request_carries_max_tokens_and_no_native_schema() {
    // The price card bounds a call by `max_tokens`; the endpoint's cost and schema are not used.
    const {
        assert!(CAPABILITIES.output_cap_enforced);
        assert!(!CAPABILITIES.cost_reported && !CAPABILITIES.structured_output);
    }
    let body = request_body(&request(Some(4000)));
    assert_eq!(body["model"], "qwen3-coder");
    assert_eq!(body["max_tokens"], 4000);
    assert_eq!(body["stream_options"]["include_usage"], true);
    assert!(body.get("response_format").is_none(), "{body}");
    assert!(body.get("provider").is_none(), "{body}");
    assert!(request_body(&request(None)).get("max_tokens").is_none());
}

#[test]
fn an_empty_model_id_is_refused() {
    assert!(OpenaiCompatible::new("http://127.0.0.1:1/v1", "k".into(), " ").is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn sends_to_the_owners_endpoint_with_the_key() {
    let (endpoint, server) = common::serve_once("/llm/v1", "200 OK", REPLY).await;
    let adapter =
        OpenaiCompatible::new(&endpoint, "vpc-key".into(), "qwen3-coder").expect("adapter");
    let reply = adapter.complete(&request(Some(300))).await.expect("reply");
    assert_eq!(reply.usage.input_tokens, Some(90));
    assert_eq!(
        reply.reported_cost, None,
        "the price card, not the endpoint, prices a call"
    );
    let raw = server.await.expect("server");
    assert!(raw.starts_with("POST /llm/v1/chat/completions "));
    assert!(
        raw.to_ascii_lowercase()
            .contains("authorization: bearer vpc-key")
    );
    let body: Value =
        serde_json::from_str(raw.split_once("\r\n\r\n").expect("body").1).expect("json");
    assert_eq!(body["max_tokens"], 300);
}
