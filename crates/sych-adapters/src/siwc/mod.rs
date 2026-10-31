//! GPT through a `ChatGPT` plan with Sign in with `ChatGPT`: text-only
//! Responses API on `api.openai.com/v1` with the plan-authorized OAuth token, no tools and no
//! agent loop. Required: `store: false`, `stream: true`, `instructions` instead of system
//! items; `max_output_tokens` and sampling fields are not accepted on this route.

use std::path::PathBuf;
use std::time::Duration;

use reqwest::Client;
use serde_json::{Value, json};
use sych_core::model::{
    Capabilities, ErrorKind, Finish, ModelProvider, ModelReply, ModelRequest, ObservedUsage,
    ProviderError, Speaker,
};
use sych_core::redact;
use tokio::time::sleep;

use crate::chat;

pub mod auth;

use auth::{AuthError, Credentials, RefreshLock};

const VENDOR: &str = "chatgpt";

/// Structured output is requested and checked; plan output caps and quota are not Sych's.
pub const CAPABILITIES: Capabilities = Capabilities {
    structured_output: true,
    context_tokens: None,
    output_cap_enforced: false,
    usage_reported: true,
    cost_reported: false,
    hidden_calls: false,
};

/// Refresh this long before the access token expires.
const REFRESH_MARGIN_SECONDS: i64 = 120;

/// Holds the credential file path, never the tokens between calls; no `Debug`.
pub struct Siwc {
    client: Client,
    url: String,
    auth_base: String,
    credentials: PathBuf,
}

fn fail(kind: ErrorKind, dispatched: bool, detail: &str) -> ProviderError {
    chat::provider_error(VENDOR, kind, dispatched, detail, None)
}

fn auth_failure(e: &AuthError) -> ProviderError {
    let kind = match e {
        AuthError::SignIn(_) => ErrorKind::Auth,
        AuthError::NotGranted(_) => ErrorKind::Entitlement,
        AuthError::Client(_) | AuthError::Store(_) => ErrorKind::Config,
        AuthError::Unavailable(_) => ErrorKind::Transient,
    };
    fail(kind, false, &e.to_string())
}

impl Siwc {
    /// `credentials` is the file `sych auth chatgpt` wrote; it must already exist.
    pub fn new(
        endpoint: &str,
        auth_base: &str,
        credentials: PathBuf,
    ) -> Result<Self, ProviderError> {
        match auth::load(&credentials) {
            Ok(Some(c)) if c.scopes.iter().any(|s| s == auth::PLAN_SCOPE) => {}
            Ok(Some(_)) => {
                return Err(auth_failure(&AuthError::NotGranted(
                    auth::PLAN_SCOPE.into(),
                )));
            }
            Ok(None) => {
                return Err(auth_failure(&AuthError::SignIn(format!(
                    "{} does not exist; run `sych auth chatgpt`",
                    credentials.display()
                ))));
            }
            Err(e) => return Err(auth_failure(&e)),
        }
        Ok(Self {
            client: chat::client()?,
            url: format!("{}/responses", endpoint.trim_end_matches('/')),
            auth_base: auth_base.trim_end_matches('/').to_owned(),
            credentials,
        })
    }

    /// A current access token, refreshed under the lock when near expiry.
    async fn token(&self) -> Result<String, ProviderError> {
        for _ in 0..50 {
            let current = auth::load(&self.credentials)
                .map_err(|e| auth_failure(&e))?
                .ok_or_else(|| {
                    auth_failure(&AuthError::SignIn("credential file is gone".into()))
                })?;
            if current.expires_at - REFRESH_MARGIN_SECONDS > auth::now() {
                return Ok(current.access_token);
            }

            let Some(_lock) =
                RefreshLock::try_take(&self.credentials).map_err(|e| auth_failure(&e))?
            else {
                // Another process refreshes this token set; wait for its result.
                sleep(Duration::from_millis(200)).await;
                continue;
            };
            // Re-read under the lock: the other process may have just finished.
            let current: Credentials = auth::load(&self.credentials)
                .map_err(|e| auth_failure(&e))?
                .ok_or_else(|| {
                    auth_failure(&AuthError::SignIn("credential file is gone".into()))
                })?;
            if current.expires_at - REFRESH_MARGIN_SECONDS > auth::now() {
                return Ok(current.access_token);
            }

            let new = auth::refresh(&self.client, &self.auth_base, &current)
                .await
                .map_err(|e| auth_failure(&e))?;
            auth::save(&self.credentials, &new).map_err(|e| auth_failure(&e))?;
            return Ok(new.access_token);
        }
        Err(fail(
            ErrorKind::Transient,
            false,
            "token refresh lock is held",
        ))
    }
}

/// The request body; the trusted layers become `instructions`.
pub fn request_body(request: &ModelRequest) -> Value {
    let instructions: Vec<&str> = request
        .messages
        .iter()
        .filter(|m| m.speaker == Speaker::System)
        .map(|m| m.content.as_str())
        .collect();
    let input: Vec<Value> = request
        .messages
        .iter()
        .filter_map(|m| {
            let role = match m.speaker {
                Speaker::System => return None,
                Speaker::User => "user",
                Speaker::Assistant => "assistant",
            };
            Some(json!({ "role": role, "content": m.content }))
        })
        .collect();
    let mut schema = request.schema.clone();
    if let Some(object) = schema.as_object_mut() {
        object.remove("$schema");
    }

    json!({
        "model": request.model,
        "instructions": instructions.join("\n\n"),
        "input": input,
        "store": false,
        "stream": true,
        "text": { "format": {
            "type": "json_schema", "name": request.schema_name, "schema": schema, "strict": true,
        } },
    })
}

/// The documented plan-usage codes first, then the HTTP status (`errors-and-recovery`).
fn code_kind(status: u16, code: &str) -> ErrorKind {
    match code {
        "subscription_sharing_usage_limit_exceeded" => ErrorKind::QuotaExhausted,
        "subscription_sharing_user_not_eligible" => ErrorKind::Entitlement,
        "subscription_sharing_invalid_user" => ErrorKind::Auth,
        "subscription_sharing_unsupported_capability" => ErrorKind::Unsupported,
        "subscription_sharing_route_not_supported"
        | "chatpass_v2_scope_not_authorized"
        | "chatpass_v2_invalid_authorization_context" => ErrorKind::Config,
        "subscription_sharing_usage_unavailable" | "subscription_sharing_user_unavailable" => {
            ErrorKind::Transient
        }
        _ => match status {
            401 => ErrorKind::Auth,
            403 => ErrorKind::Entitlement,
            429 => ErrorKind::RateLimited,
            500.. => ErrorKind::Transient,
            _ => ErrorKind::Config,
        },
    }
}

fn error_detail(error: &Value) -> (String, String) {
    let code = error["code"].as_str().unwrap_or_default().to_owned();
    let param = error["param"]
        .as_str()
        .map(|p| format!(" ({p})"))
        .unwrap_or_default();
    let message = error["message"].as_str().unwrap_or_default();
    (code.clone(), format!("{code}{param}: {message}"))
}

/// An admission or request failure before the stream opened; nothing was generated.
pub fn parse_error(status: u16, text: &str) -> ProviderError {
    let body: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let (code, detail) = if body["error"].is_object() {
        error_detail(&body["error"])
    } else {
        // Admission errors may carry only `{"detail": "..."}`: diagnostic text.
        let detail = body["detail"].as_str().unwrap_or("no detail");
        (String::new(), format!("HTTP {status}: {detail}"))
    };
    // 503 is admission (routing or usage check unavailable); other 5xx may have run.
    fail(
        code_kind(status, &code),
        status >= 500 && status != 503,
        &detail,
    )
}

/// Folds the Responses event stream; only `response.completed` is a finished answer.
pub fn parse_stream(text: &str) -> Result<ModelReply, ProviderError> {
    let (mut content, mut refused, mut id, mut model) = (String::new(), false, None, None);
    for data in text.lines().filter_map(|l| l.strip_prefix("data:")) {
        let Ok(event) = serde_json::from_str::<Value>(data.trim()) else {
            return Err(fail(ErrorKind::Transient, true, "malformed stream event"));
        };
        let response = &event["response"];
        id = id.or_else(|| response["id"].as_str().map(str::to_owned));
        model = model.or_else(|| response["model"].as_str().map(str::to_owned));
        match event["type"].as_str().unwrap_or_default() {
            "response.output_text.delta" => {
                content.push_str(event["delta"].as_str().unwrap_or_default());
            }
            "response.refusal.delta" | "response.refusal.done" => refused = true,
            // No tools are offered; any other item (a function or hosted tool call) is refused.
            "response.output_item.added" => {
                let kind = event["item"]["type"].as_str().unwrap_or_default();
                if !matches!(kind, "message" | "reasoning") {
                    return Err(fail(
                        ErrorKind::Unsupported,
                        true,
                        &format!("unexpected output item `{kind}`"),
                    ));
                }
            }
            "response.completed" => {
                let usage = &response["usage"];
                return Ok(ModelReply {
                    text: content,
                    finish: if refused {
                        Finish::Refused
                    } else {
                        Finish::Complete
                    },
                    resolved_model: response["model"].as_str().map(str::to_owned).or(model),
                    upstream_provider: Some("openai".into()),
                    generation_id: id,
                    usage: ObservedUsage {
                        input_tokens: usage["input_tokens"].as_u64(),
                        output_tokens: usage["output_tokens"].as_u64(),
                        reasoning_tokens: usage["output_tokens_details"]["reasoning_tokens"]
                            .as_u64(),
                    },
                    // Plan usage reports no money; standard API prices are not its cost.
                    reported_cost: None,
                    billing_source: Some("chatgpt_plan".into()),
                    allowance: None,
                    list_cost: None,
                });
            }
            "response.incomplete" => {
                let reason = response["incomplete_details"]["reason"]
                    .as_str()
                    .unwrap_or_default();
                let finish = if reason == "content_filter" {
                    Finish::Refused
                } else {
                    Finish::Truncated
                };
                tracing::warn!(reason, generation = ?id, "chatgpt response incomplete");
                return Ok(ModelReply {
                    text: content,
                    finish,
                    resolved_model: model,
                    upstream_provider: Some("openai".into()),
                    generation_id: id,
                    usage: ObservedUsage::default(),
                    reported_cost: None,
                    billing_source: Some("chatgpt_plan".into()),
                    allowance: None,
                    list_cost: None,
                });
            }
            "response.failed" | "error" => {
                let error = if event["type"] == "error" {
                    &event
                } else {
                    &response["error"]
                };
                let (code, detail) = error_detail(error);
                // Failed after the stream opened: it may have counted against the plan.
                return Err(fail(code_kind(0, &code), true, &detail));
            }
            _ => {}
        }
    }
    let generation = id.as_deref().unwrap_or("unknown");
    Err(fail(
        ErrorKind::Transient,
        true,
        &format!("stream ended before response.completed (response {generation})"),
    ))
}

impl ModelProvider for Siwc {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelReply, ProviderError> {
        let token = self.token().await?;
        let response = self
            .client
            .post(&self.url)
            .bearer_auth(&token)
            .header("accept", "text/event-stream")
            .json(&request_body(request))
            .send()
            .await;
        let mut response = match response {
            Ok(r) => r,
            Err(e) => {
                let kind = if e.is_timeout() {
                    ErrorKind::Timeout
                } else {
                    ErrorKind::Transient
                };
                return Err(fail(kind, !e.is_connect(), "transport failure"));
            }
        };
        let status = response.status().as_u16();
        let mut bytes = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => bytes.extend_from_slice(&chunk),
                Ok(None) => break,
                Err(e) => {
                    let kind = if e.is_timeout() {
                        ErrorKind::Timeout
                    } else {
                        ErrorKind::Transient
                    };
                    return Err(fail(kind, true, "response body lost"));
                }
            }
        }
        let text = String::from_utf8_lossy(&bytes);
        if status != 200 {
            return Err(parse_error(status, &redact(&text)));
        }

        parse_stream(&text)
    }
}
