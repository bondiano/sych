//! GLM Coding Plan through the pinned Pi coding agent (roadmap M1.11): Z.ai allows the plan in
//! supported tools, so Sych runs the real Pi CLI rather than its own HTTP client. Each request
//! is one isolated Pi process: empty HOME, agent directory and working directory, every tool,
//! extension, skill, MCP server and context file off, retries, compaction and cache warming
//! off, the packet on stdin and one JSONL event stream back.

use std::path::{Path, PathBuf};
use std::process::{self, Stdio};
use std::{env, fs, io};

use serde::Deserialize;
use serde_json::{Value, json};
use sych_core::config::ReasoningEffort;
use sych_core::model::{
    Capabilities, ErrorKind, Finish, ModelProvider, ModelReply, ModelRequest, ObservedUsage,
    ProviderError, Speaker,
};
use sych_core::redact;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

use crate::chat::{REQUEST_TIMEOUT, config_error, usd_to_micro};

/// The Pi release whose flags, settings and event stream this adapter was checked against.
pub const PI_VERSION: &str = "1.4.0";

/// Pi's provider id of the global GLM Coding Plan.
pub const PI_PROVIDER: &str = "zai";

/// Upstream output and quota are not bounded by Sych; Pi reports tokens, not money.
pub const CAPABILITIES: Capabilities = Capabilities {
    structured_output: false,
    context_tokens: None,
    output_cap_enforced: false,
    usage_reported: true,
    cost_reported: false,
    hidden_calls: false,
};

/// Agent-directory settings: no hidden retries, compaction, cache refreshes or project trust.
fn settings() -> Value {
    json!({
        "retry": { "enabled": false, "provider": { "maxRetries": 0 } },
        "compaction": { "enabled": false },
        "cacheWarming": "off",
        "defaultProjectTrust": "never",
    })
}

/// Credentials are held only here and never formatted; there is no `Debug`.
pub struct PiCli {
    executable: PathBuf,
    endpoint: String,
    api_key: String,
    model: String,
    thinking: Option<ReasoningEffort>,
}

/// A Z.ai model code as Pi lists it: no provider prefix, no moving alias.
pub fn exact_model(id: &str) -> bool {
    !id.is_empty() && !id.contains(['/', ':']) && !id.ends_with("latest")
}

fn fail(kind: ErrorKind, dispatched: bool, detail: &str) -> ProviderError {
    let detail: String = detail.chars().take(300).collect();
    ProviderError {
        kind,
        dispatched,
        message: format!("pi: {}", redact(&detail)),
        retry_after_seconds: None,
    }
}

/// Only the search path is inherited: Pi is a Node script and needs `node`.
fn command(executable: &Path) -> Command {
    let mut cmd = Command::new(executable);
    cmd.env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(path) = env::var_os("PATH") {
        cmd.env("PATH", path);
    }

    cmd
}

impl PiCli {
    /// Checks the model id and that the executable is the pinned Pi release.
    pub fn new(
        executable: PathBuf,
        endpoint: &str,
        api_key: String,
        model: &str,
        thinking: Option<ReasoningEffort>,
    ) -> Result<Self, ProviderError> {
        if !exact_model(model) {
            return Err(config_error(&format!(
                "pi model `{model}` is not an exact Z.ai model code"
            )));
        }

        let mut version = process::Command::new(&executable);
        version.env_clear().arg("--version").stdin(Stdio::null());
        if let Some(path) = env::var_os("PATH") {
            version.env("PATH", path);
        }

        let output = version
            .output()
            .map_err(|e| config_error(&format!("cannot run {}: {e}", executable.display())))?;
        let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if version != PI_VERSION {
            return Err(config_error(&format!(
                "{} is Pi `{version}`, expected the pinned {PI_VERSION}",
                executable.display()
            )));
        }

        Ok(Self {
            executable,
            endpoint: endpoint.trim_end_matches('/').to_owned(),
            api_key,
            model: model.to_owned(),
            thinking,
        })
    }
}

/// Arguments of one run; the prompt itself goes to stdin.
pub fn args(model: &str, thinking: Option<ReasoningEffort>, system: &Path) -> Vec<String> {
    let mut args: Vec<String> = [
        "--mode",
        "json",
        "--no-session",
        "--no-tools",
        "--no-extensions",
        "--no-skills",
        "--no-prompt-templates",
        "--no-themes",
        "--no-context-files",
        "--no-mcp",
        "--no-approve",
        "--offline",
        "--provider",
        PI_PROVIDER,
        "--model",
        model,
    ]
    .map(str::to_owned)
    .into();
    if let Some(effort) = thinking {
        args.extend(["--thinking".into(), effort.as_str().into()]);
    }

    args.extend(["--system-prompt".into(), system.display().to_string()]);
    args
}

/// The conversation after the system message as one prompt; Pi takes a single user turn.
pub fn transcript(request: &ModelRequest) -> String {
    request
        .messages
        .iter()
        .filter_map(|m| match m.speaker {
            Speaker::System => None,
            Speaker::User => Some(m.content.clone()),
            Speaker::Assistant => Some(format!("Your previous reply:\n{}", m.content)),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[derive(Deserialize)]
struct Event {
    #[serde(rename = "type")]
    kind: String,
    message: Option<Message>,
    #[serde(rename = "toolName")]
    tool_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_field_names)] // Pi's own field names
struct Message {
    role: String,
    /// Blocks of an assistant message; a system message has a plain string.
    #[serde(default)]
    content: Value,
    provider: Option<String>,
    model: Option<String>,
    response_model: Option<String>,
    response_id: Option<String>,
    usage: Option<Usage>,
    stop_reason: Option<String>,
    error_message: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Usage {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    reasoning: Option<u64>,
    total_tokens: u64,
    cost: Option<Cost>,
}

#[derive(Deserialize)]
struct Cost {
    total: Value,
}

/// Z.ai business codes of the plan (`docs.z.ai/api-reference/api-code`), then the HTTP status.
fn zai_kind(status: u16, code: Option<&str>) -> ErrorKind {
    match code {
        Some("1000" | "1001" | "1003" | "1005") => ErrorKind::Auth,
        Some("1308" | "1310" | "1316" | "1317" | "1318" | "1319" | "1320" | "1321") => {
            ErrorKind::QuotaExhausted
        }
        // No package, expired plan, model outside the plan, fair-use block, enterprise-only key.
        Some("1113" | "1309" | "1311" | "1313" | "1314" | "1315" | "1220") => {
            ErrorKind::Entitlement
        }
        Some("1302") => ErrorKind::RateLimited,
        Some("1305" | "1234") => ErrorKind::Transient,
        _ => match status {
            401 => ErrorKind::Auth,
            403 => ErrorKind::Entitlement,
            429 => ErrorKind::RateLimited,
            500.. => ErrorKind::Transient,
            _ => ErrorKind::Config,
        },
    }
}

/// Pi's `errorMessage` of a failed call: `<status>: <body>`, or a transport message.
fn classify(error: &str) -> ProviderError {
    let parsed = error
        .split_once(": ")
        .and_then(|(status, body)| Some((status.trim().parse::<u16>().ok()?, body)));
    let Some((status, body)) = parsed else {
        // No HTTP status: the request may or may not have left the host.
        return fail(ErrorKind::Transient, true, error);
    };
    let body: Option<Value> = serde_json::from_str(body).ok();
    let code = body
        .as_ref()
        .and_then(|b| b.get("code").or_else(|| b.pointer("/error/code")))
        .and_then(|c| {
            c.as_str()
                .map(str::to_owned)
                .or_else(|| Some(c.to_string()))
        });
    let kind = zai_kind(status, code.as_deref());
    fail(kind, status >= 500, error)
}

/// Folds one Pi JSONL stream to the single assistant answer of the run.
pub fn parse_events(stdout: &str) -> Result<ModelReply, ProviderError> {
    let mut answer = None;
    let mut assistants = 0;
    let mut settled = false;
    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        let event: Event = serde_json::from_str(line)
            .map_err(|_| fail(ErrorKind::Transient, true, "malformed event line"))?;
        match event.kind.as_str() {
            "tool_execution_start" => {
                let tool = event.tool_name.unwrap_or_default();
                return Err(fail(
                    ErrorKind::Unsupported,
                    true,
                    &format!("tool `{tool}` started"),
                ));
            }
            // Turned off in settings; seeing one means Pi made calls Sych did not dispatch.
            "auto_retry_start" | "compaction_start" | "summarization_retry_scheduled" => {
                return Err(fail(
                    ErrorKind::Unsupported,
                    true,
                    &format!("hidden `{}` despite settings", event.kind),
                ));
            }
            "message_end" => {
                if let Some(m) = event.message.filter(|m| m.role == "assistant") {
                    assistants += 1;
                    answer = Some(m);
                }
            }
            "agent_settled" => settled = true,
            _ => {}
        }
    }
    let message = match answer {
        Some(m) if settled && assistants == 1 => m,
        Some(_) if assistants > 1 => {
            return Err(fail(
                ErrorKind::Unsupported,
                true,
                "more than one model turn",
            ));
        }
        _ => {
            return Err(fail(
                ErrorKind::Transient,
                true,
                "event stream ended before the answer settled",
            ));
        }
    };
    let finish = match message.stop_reason.as_deref() {
        Some("stop") => Finish::Complete,
        Some("error") => return Err(classify(message.error_message.as_deref().unwrap_or(""))),
        Some("aborted") => return Err(fail(ErrorKind::Cancelled, true, "aborted")),
        Some("toolUse") => return Err(fail(ErrorKind::Unsupported, true, "unexpected tool call")),
        // `length`, `deferred` or unknown: not a complete answer.
        _ => Finish::Truncated,
    };
    let text = message
        .content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["type"] == "text")
        .filter_map(|c| c["text"].as_str())
        .collect();
    // All-zero usage means the provider did not report it, not that nothing was used.
    let usage = message.usage.as_ref().filter(|u| u.total_tokens > 0);
    Ok(ModelReply {
        text,
        finish,
        resolved_model: message.response_model.or(message.model),
        upstream_provider: message.provider,
        generation_id: message.response_id,
        usage: usage.map_or_else(ObservedUsage::default, |u| ObservedUsage {
            input_tokens: Some(u.input + u.cache_read + u.cache_write),
            output_tokens: Some(u.output),
            reasoning_tokens: u.reasoning,
        }),
        // Pi prices tokens from its own catalog: a reference, never the plan's charge.
        reported_cost: None,
        billing_source: None,
        allowance: None,
        list_cost: usage
            .and_then(|u| u.cost.as_ref())
            .and_then(|c| usd_to_micro(&c.total.to_string())),
    })
}

impl ModelProvider for PiCli {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelReply, ProviderError> {
        let local = |e: io::Error| config_error(&format!("pi run directory: {e}"));
        let dir = tempfile::tempdir().map_err(local)?;
        let (home, agent, work) = (
            dir.path().join("home"),
            dir.path().join("agent"),
            dir.path().join("work"),
        );
        for d in [&home, &agent, &work] {
            fs::create_dir(d).map_err(local)?;
        }
        let models = json!({ "providers": { PI_PROVIDER: { "baseUrl": self.endpoint } } });
        fs::write(agent.join("settings.json"), settings().to_string()).map_err(local)?;
        fs::write(agent.join("models.json"), models.to_string()).map_err(local)?;
        let system = dir.path().join("system.md");
        let instructions = request
            .messages
            .iter()
            .filter(|m| m.speaker == Speaker::System)
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        fs::write(&system, instructions).map_err(local)?;

        let mut child = command(&self.executable)
            .args(args(&self.model, self.thinking, &system))
            .current_dir(&work)
            .env("HOME", &home)
            .env("PI_CODING_AGENT_DIR", &agent)
            .env("PI_OFFLINE", "1")
            .env("PI_SKIP_VERSION_CHECK", "1")
            .env("PI_TELEMETRY", "0")
            .env("ZAI_API_KEY", &self.api_key)
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| config_error(&format!("cannot start pi: {e}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            let prompt = transcript(request);
            stdin
                .write_all(prompt.as_bytes())
                .await
                .map_err(|e| fail(ErrorKind::Transient, false, &format!("pi stdin: {e}")))?;
        }

        let Ok(output) = timeout(REQUEST_TIMEOUT, child.wait_with_output()).await else {
            // Dropping the child kills it; upstream work may continue and stays unknown.
            return Err(fail(ErrorKind::Timeout, true, "pi did not finish in time"));
        };
        let output = output.map_err(|e| fail(ErrorKind::Transient, true, &e.to_string()))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() && stdout.trim().is_empty() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let first = stderr.lines().next().unwrap_or("no output");
            return Err(fail(
                ErrorKind::Transient,
                true,
                &format!("pi exited with {}: {first}", output.status),
            ));
        }

        parse_events(&stdout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_codes_are_told_apart() {
        let quota = classify(r#"429: {"code":"1308","message":"Usage limit reached for 5 hour"}"#);
        assert_eq!(
            (quota.kind, quota.dispatched),
            (ErrorKind::QuotaExhausted, false)
        );
        let rate = classify(r#"429: {"code":"1302","message":"Rate limit reached for requests"}"#);
        assert_eq!(rate.kind, ErrorKind::RateLimited);
        let expired = classify(r#"429: {"code":"1309","message":"package has expired"}"#);
        assert_eq!(expired.kind, ErrorKind::Entitlement);
        let auth = classify(r#"401: {"error":{"code":"1000","message":"Authentication Failed"}}"#);
        assert_eq!(auth.kind, ErrorKind::Auth);
        let lost = classify("Connection error.");
        assert_eq!((lost.kind, lost.dispatched), (ErrorKind::Transient, true));
    }
}
