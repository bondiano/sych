//! Pi CLI adapter fixtures (roadmap M1.11). The event streams under `fixtures/pi` come from the
//! real Pi 1.4.0 run against a loopback Z.ai stub; the process boundary is checked with a
//! stand-in executable that records what it was given.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::json;
use sych_adapters::pi::{PI_VERSION, PiCli, args, exact_model, parse_events, transcript};
use sych_core::config::ReasoningEffort;
use sych_core::model::{ErrorKind, Finish, Message, ModelProvider, ModelRequest, Speaker};
use sych_core::{AttemptId, MicroUsd, TaskId};

fn fixture(name: &str) -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/pi")
            .join(name),
    )
    .expect("fixture")
}

fn request(messages: Vec<Message>) -> ModelRequest {
    ModelRequest {
        attempt_id: AttemptId::new_v7(),
        task_id: TaskId::new_v7(),
        model: "glm-5.3".into(),
        messages,
        schema_name: "probe".into(),
        schema: json!({ "type": "object" }),
        max_output_tokens: None,
        deadline: None,
    }
}

fn message(speaker: Speaker, content: &str) -> Message {
    Message {
        speaker,
        content: content.into(),
    }
}

#[test]
fn a_settled_answer_keeps_tokens_but_not_pi_prices_as_charge() {
    let reply = parse_events(&fixture("ok.jsonl")).expect("reply");
    assert_eq!(reply.text, r#"{"echo":"owl"}"#);
    assert_eq!(reply.finish, Finish::Complete);
    assert_eq!(reply.resolved_model.as_deref(), Some("glm-5.3"));
    assert_eq!(reply.upstream_provider.as_deref(), Some("zai"));
    assert_eq!(reply.generation_id.as_deref(), Some("chatcmpl-z1"));
    assert_eq!(
        (reply.usage.input_tokens, reply.usage.output_tokens),
        (Some(50), Some(7))
    );
    assert_eq!(
        reply.reported_cost, None,
        "Pi's catalog price is not the plan's bill"
    );
    assert_eq!(reply.list_cost, Some(MicroUsd(101)));
}

#[test]
fn auth_and_plan_quota_failures_are_distinct() {
    let auth = parse_events(&fixture("err401.jsonl")).expect_err("auth");
    assert_eq!((auth.kind, auth.dispatched), (ErrorKind::Auth, false));
    let quota = parse_events(&fixture("quota.jsonl")).expect_err("quota");
    assert_eq!(quota.kind, ErrorKind::QuotaExhausted);
    assert!(quota.message.contains("reset at"), "{}", quota.message);
}

#[test]
fn tool_use_hidden_calls_and_cut_streams_are_not_answers() {
    let ok = fixture("ok.jsonl");
    let tool = ok.replace(
        r#"{"type":"agent_settled","aborted":false}"#,
        r#"{"type":"tool_execution_start","toolCallId":"c1","toolName":"bash","args":{}}"#,
    );
    let e = parse_events(&tool).expect_err("tool");
    assert_eq!(e.kind, ErrorKind::Unsupported);
    let retry = ok.replace(
        r#"{"type":"agent_settled","aborted":false}"#,
        r#"{"type":"auto_retry_start"}"#,
    );
    assert_eq!(
        parse_events(&retry).expect_err("retry").kind,
        ErrorKind::Unsupported
    );
    let cut: String = ok
        .lines()
        .filter(|l| !l.contains("agent_settled"))
        .flat_map(|l| {
            [
                l, "
",
            ]
        })
        .collect();
    let e = parse_events(&cut).expect_err("cut");
    assert_eq!((e.kind, e.dispatched), (ErrorKind::Transient, true));
    let length = ok.replace(r#""stopReason":"stop""#, r#""stopReason":"length""#);
    assert_eq!(
        parse_events(&length).expect("reply").finish,
        Finish::Truncated
    );
}

#[test]
fn every_resource_and_tool_is_off_and_the_key_is_not_an_argument() {
    let args = args(
        "glm-5.3",
        Some(ReasoningEffort::Low),
        Path::new("/run/system.md"),
    );
    for flag in [
        "--no-session",
        "--no-tools",
        "--no-extensions",
        "--no-skills",
        "--no-prompt-templates",
        "--no-context-files",
        "--no-mcp",
        "--no-approve",
        "--offline",
    ] {
        assert!(args.iter().any(|a| a == flag), "{flag} in {args:?}");
    }
    assert!(!args.iter().any(|a| a.contains("api-key")));
    assert!(exact_model("glm-5.3") && !exact_model("zai/glm-5.3") && !exact_model("glm:high"));
}

#[test]
fn a_repair_is_one_prompt_with_the_previous_reply() {
    let request = request(vec![
        message(Speaker::System, "rules"),
        message(Speaker::User, "task"),
        message(Speaker::Assistant, "not json"),
        message(Speaker::User, "fix it"),
    ]);
    assert_eq!(
        transcript(&request),
        "task\n\nYour previous reply:\nnot json\n\nfix it"
    );
}

/// A stand-in `pi` that records its arguments, environment, stdin and run directory.
fn stand_in(dir: &Path, version: &str) -> PathBuf {
    let reply = dir.join("reply.jsonl");
    fs::write(&reply, fixture("ok.jsonl")).expect("reply");
    let script = dir.join("pi");
    let seen = dir.join("seen");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\n\
             if [ \"$1\" = --version ]; then echo {version}; exit 0; fi\n\
             printf '%s\\n' \"$@\" > {seen}.args\n\
             env > {seen}.env\n\
             cat > {seen}.stdin\n\
             pwd > {seen}.cwd; ls -A > {seen}.ls\n\
             cat \"$PI_CODING_AGENT_DIR/models.json\" > {seen}.models\n\
             cat {reply}\n",
            seen = seen.display(),
            reply = reply.display()
        ),
    )
    .expect("script");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("chmod");
    script
}

#[tokio::test(flavor = "current_thread")]
async fn runs_isolated_with_only_the_plan_key_and_the_packet_on_stdin() {
    let dir = tempfile::tempdir().expect("dir");
    let script = stand_in(dir.path(), PI_VERSION);
    let pi = PiCli::new(
        script,
        "https://api.z.ai/api/coding/paas/v4",
        "zai-plan-key".into(),
        "glm-5.3",
        None,
    )
    .expect("pinned pi");
    let request = request(vec![
        message(Speaker::System, "rules"),
        message(Speaker::User, "packet"),
    ]);
    let reply = pi.complete(&request).await.expect("reply");
    assert_eq!(reply.finish, Finish::Complete);

    let read = |ext: &str| fs::read_to_string(dir.path().join(format!("seen.{ext}"))).expect(ext);
    assert_eq!(read("stdin"), "packet");
    assert!(!read("args").contains("zai-plan-key"));
    assert!(
        read("ls").trim().is_empty(),
        "the working directory is empty"
    );
    assert!(
        read("models").contains("https://api.z.ai/api/coding/paas/v4"),
        "the plan endpoint is pinned"
    );
    let env = read("env");
    let names: Vec<&str> = env
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, _)| k)
        .collect();
    for name in &names {
        assert!(
            ["PATH", "HOME", "PWD", "OLDPWD", "SHLVL", "_", "ZAI_API_KEY"].contains(name)
                || name.starts_with("PI_"),
            "unexpected variable {name}"
        );
    }
    assert!(env.contains("ZAI_API_KEY=zai-plan-key"));
    assert!(env.contains("PI_OFFLINE=1") && env.contains("PI_TELEMETRY=0"));
    assert!(
        !dir.path().join("work").exists(),
        "the run directory is temporary"
    );
}

#[test]
fn another_pi_release_is_refused() {
    let dir = tempfile::tempdir().expect("dir");
    let script = stand_in(dir.path(), "1.5.0");
    let e = PiCli::new(
        script,
        "https://api.z.ai/api/coding/paas/v4",
        "k".into(),
        "glm-5.3",
        None,
    )
    .err()
    .expect("refused");
    assert_eq!(e.kind, ErrorKind::Config);
    assert!(e.message.contains(PI_VERSION), "{}", e.message);
}
