//! `OpenCode` Go adapter fixtures (roadmap M1.12): the `/go` path, Sych's User-Agent, one
//! session per task, no claimed native schema, and Go billing fields. Loopback only.

use serde_json::{Value, json};
use sych_adapters::chat::parse_stream;
use sych_adapters::opencode_go::{OpencodeGo, USER_AGENT, exact_model, request_body};
use sych_core::model::{Message, ModelProvider, ModelRequest, Speaker};
use sych_core::{AttemptId, MicroUsd, TaskId};

mod common;

fn request(task_id: TaskId) -> ModelRequest {
    ModelRequest {
        attempt_id: AttemptId::new_v7(),
        task_id,
        model: "deepseek-v4.1-flash".into(),
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
        max_output_tokens: None,
        deadline: None,
    }
}

/// A Go reply inside the plan's limits: no money, some allowance.
const WITHIN_PLAN: &str = concat!(
    "data: {\"id\":\"chatcmpl-1\",\"model\":\"deepseek-v4.1-flash\",",
    "\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"{\\\"ok\\\":true}\"},\"finish_reason\":\"stop\"}]}\n\n",
    "data: {\"id\":\"chatcmpl-1\",\"choices\":[],\"usage\":{\"prompt_tokens\":90,\"completion_tokens\":12,",
    "\"cost\":0,\"opencode\":{\"billing_source\":\"go\",\"allowance\":0.00021,\"list_cost\":0.00004,",
    "\"tokens\":{\"input\":90,\"output\":12,\"reasoning\":0}}}}\n\n",
    "data: [DONE]\n\n",
);

/// The same request once the plan ran out and Extra Usage paid for it.
const FROM_BALANCE: &str = concat!(
    "data: {\"id\":\"chatcmpl-2\",\"choices\":[{\"delta\":{\"content\":\"{}\"},\"finish_reason\":\"stop\"}]}\n\n",
    "data: {\"id\":\"chatcmpl-2\",\"choices\":[],\"usage\":{\"prompt_tokens\":90,\"completion_tokens\":2,",
    "\"cost\":0.00004,\"opencode\":{\"billing_source\":\"balance\",\"list_cost\":0.00004}}}\n\n",
    "data: [DONE]\n\n",
);

#[test]
fn body_claims_no_native_schema_and_asks_for_usage() {
    let body = request_body(&request(TaskId::new_v7()));
    assert_eq!(body["model"], "deepseek-v4.1-flash");
    assert_eq!(body["stream_options"]["include_usage"], true);
    assert!(body.get("response_format").is_none(), "{body}");
    assert!(body.get("max_tokens").is_none(), "{body}");
    assert!(body.get("provider").is_none(), "{body}");
}

#[test]
fn zero_charge_with_allowance_is_a_reported_fact() {
    let reply = parse_stream("opencode-go", 200, None, WITHIN_PLAN).expect("reply");
    assert_eq!(reply.reported_cost, Some(MicroUsd(0)));
    assert_eq!(reply.billing_source.as_deref(), Some("go"));
    assert_eq!(reply.allowance, Some(MicroUsd(210)));
    assert_eq!(reply.list_cost, Some(MicroUsd(40)));
    assert_eq!(reply.usage.input_tokens, Some(90));
}

#[test]
fn a_balance_charge_is_reported_as_money() {
    let reply = parse_stream("opencode-go", 200, None, FROM_BALANCE).expect("reply");
    assert_eq!(reply.billing_source.as_deref(), Some("balance"));
    assert_eq!(reply.reported_cost, Some(MicroUsd(40)));
    assert_eq!(reply.allowance, None);
}

#[test]
fn models_are_exact_go_ids() {
    assert!(exact_model("deepseek-v4.1-flash"));
    assert!(!exact_model("opencode-go/deepseek-v4.1-flash"));
    assert!(!exact_model("deepseek-latest"));
    assert!(OpencodeGo::new("http://127.0.0.1:1/x", "k".into(), "a/b").is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn sends_go_path_own_agent_and_task_session() {
    let (endpoint, server) =
        common::serve_once("/inference/go/openai/v1", "200 OK", WITHIN_PLAN).await;
    let adapter =
        OpencodeGo::new(&endpoint, "go-key".into(), "deepseek-v4.1-flash").expect("adapter");
    let task = TaskId::new_v7();
    let reply = adapter.complete(&request(task)).await.expect("reply");
    assert_eq!(reply.allowance, Some(MicroUsd(210)));
    let raw = server.await.expect("server");
    let head = raw.to_ascii_lowercase();
    assert!(raw.starts_with("POST /inference/go/openai/v1/chat/completions "));
    assert!(head.contains("authorization: bearer go-key"));
    assert!(head.contains(&format!("user-agent: {USER_AGENT}")));
    assert!(head.contains(&format!("x-opencode-session: {task}")));
    let body: Value =
        serde_json::from_str(raw.split_once("\r\n\r\n").expect("body").1).expect("json");
    assert_eq!(body["stream"], true);
}
