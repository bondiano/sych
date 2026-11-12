//! Any OpenAI-compatible chat completions endpoint as a [`ModelProvider`]: a VPC gateway, vLLM,
//! LiteLLM, Ollama or a vendor API. The owner names the endpoint, the key and the price card.
//! Every request carries `max_tokens`, so the price card bounds a call before it is sent. No
//! native schema is claimed: the gateway validates locally and repairs at most once.

use reqwest::Client;
use serde_json::{Value, json};
use sych_core::model::{Capabilities, ModelProvider, ModelReply, ModelRequest, ProviderError};

use crate::chat::{self, config_error};
use crate::opencode_go::USER_AGENT;

const VENDOR: &str = "openai-compatible";

/// Usage comes back on the final chunk when the endpoint honours `stream_options`; the cost is
/// the price card's, never the endpoint's.
pub const CAPABILITIES: Capabilities = Capabilities {
    structured_output: false,
    context_tokens: None,
    output_cap_enforced: true,
    usage_reported: true,
    cost_reported: false,
    hidden_calls: false,
    reasoning_off: false,
};

/// Credentials are held only here and never formatted; there is no `Debug`.
pub struct OpenaiCompatible {
    client: Client,
    url: String,
    api_key: String,
}

impl OpenaiCompatible {
    pub fn new(endpoint: &str, api_key: String, model: &str) -> Result<Self, ProviderError> {
        if model.trim().is_empty() {
            return Err(config_error("openai-compatible model id is empty"));
        }

        Ok(Self {
            client: chat::client()?,
            url: format!("{}/chat/completions", endpoint.trim_end_matches('/')),
            api_key,
        })
    }
}

pub fn request_body(request: &ModelRequest) -> Value {
    let mut body = json!({
        "model": request.model,
        "messages": chat::messages(request),
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    if let Some(max) = request.max_output_tokens {
        body["max_tokens"] = json!(max);
    }

    body
}

impl ModelProvider for OpenaiCompatible {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelReply, ProviderError> {
        let http = self
            .client
            .post(&self.url)
            .bearer_auth(&self.api_key)
            .header("user-agent", USER_AGENT)
            .json(&request_body(request));
        let mut reply = chat::send(VENDOR, http).await?;
        // A cost field of an arbitrary endpoint is not the owner's price; the card prices usage.
        reply.reported_cost = None;
        reply.list_cost = None;
        Ok(reply)
    }
}
