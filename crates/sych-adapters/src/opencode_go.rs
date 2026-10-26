//! `OpenCode` Go / Go Plus subscription as a [`ModelProvider`] (roadmap M1.12): the documented
//! OpenAI-compatible chat completions of the Go inference path, with Sych's own User-Agent and
//! one `x-opencode-session` per task. No native schema is claimed: the gateway validates
//! locally and repairs at most once.

use reqwest::Client;
use serde_json::{Value, json};
use sych_core::model::{Capabilities, ModelProvider, ModelReply, ModelRequest, ProviderError};

use crate::chat::{self, config_error};

const VENDOR: &str = "opencode-go";

pub const USER_AGENT: &str = concat!("sych/", env!("CARGO_PKG_VERSION"));

/// Usage and cost come back on the final chunk; output caps and quota stay soft.
pub const CAPABILITIES: Capabilities = Capabilities {
    structured_output: false,
    context_tokens: None,
    output_cap_enforced: false,
    usage_reported: true,
    cost_reported: true,
    hidden_calls: false,
};

/// Billing sources a subscription-only call may report.
pub const SUBSCRIPTION_SOURCES: [&str; 2] = ["go", "go-plus"];

/// Credentials are held only here and never formatted; there is no `Debug`.
pub struct OpencodeGo {
    client: Client,
    url: String,
    api_key: String,
}

/// A Go model id as the catalog lists it: no vendor prefix, no moving alias.
pub fn exact_model(id: &str) -> bool {
    !id.is_empty() && !id.contains('/') && !id.ends_with("latest")
}

impl OpencodeGo {
    pub fn new(endpoint: &str, api_key: String, model: &str) -> Result<Self, ProviderError> {
        if !exact_model(model) {
            return Err(config_error(&format!(
                "opencode go model `{model}` is not an exact Go model id"
            )));
        }

        Ok(Self {
            client: chat::client()?,
            url: format!("{}/chat/completions", endpoint.trim_end_matches('/')),
            api_key,
        })
    }
}

pub fn request_body(request: &ModelRequest) -> Value {
    json!({
        "model": request.model,
        "messages": chat::messages(request),
        "stream": true,
        "stream_options": { "include_usage": true },
    })
}

impl ModelProvider for OpencodeGo {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelReply, ProviderError> {
        let http = self
            .client
            .post(&self.url)
            .bearer_auth(&self.api_key)
            .header("user-agent", USER_AGENT)
            .header("x-opencode-session", request.task_id.to_string())
            .json(&request_body(request));
        let reply = chat::send(VENDOR, http).await?;
        // The ledger flags any charge above the zero reservation; this names the cause.
        if let Some(source) = reply
            .billing_source
            .as_deref()
            .filter(|s| !SUBSCRIPTION_SOURCES.contains(s))
        {
            tracing::warn!(
                source,
                cost = ?reply.reported_cost,
                "opencode go request was not billed to the subscription"
            );
        }

        Ok(reply)
    }
}
