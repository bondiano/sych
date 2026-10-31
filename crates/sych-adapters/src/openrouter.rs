//! `OpenRouter` chat completions as a [`ModelProvider`]. The model and upstream
//! providers are pinned per request: no auto router, no hidden provider fallback. Replies are
//! streamed: keep-alive comments hold long reasoning open, and a cut stream still names its
//! generation.

use reqwest::Client;
use serde_json::{Value, json};
use sych_core::config::ReasoningEffort;
use sych_core::model::{Capabilities, ModelProvider, ModelReply, ModelRequest, ProviderError};

use crate::chat::{self, config_error};

pub use crate::chat::REQUEST_TIMEOUT;

const VENDOR: &str = "openrouter";

pub const CAPABILITIES: Capabilities = Capabilities {
    structured_output: true,
    context_tokens: None,
    output_cap_enforced: true,
    usage_reported: true,
    cost_reported: true,
    hidden_calls: false,
};

/// Credentials are held only here and never formatted; there is no `Debug`.
pub struct OpenRouter {
    client: Client,
    url: String,
    api_key: String,
    upstreams: Vec<String>,
    reasoning: Option<ReasoningEffort>,
}

/// Rejects auto routers and moving aliases: the model must be an exact slug.
pub fn pinned_model(slug: &str) -> bool {
    !slug.is_empty()
        && !slug.starts_with("openrouter/")
        && !slug.starts_with('~')
        && !slug.ends_with("latest")
        && slug.contains('/')
}

impl OpenRouter {
    pub fn new(
        endpoint: &str,
        api_key: String,
        model: &str,
        upstreams: Vec<String>,
        reasoning: Option<ReasoningEffort>,
    ) -> Result<Self, ProviderError> {
        if !pinned_model(model) {
            return Err(config_error(&format!(
                "openrouter model `{model}` is not an exact pinned slug"
            )));
        }

        if upstreams.is_empty() {
            return Err(config_error(
                "openrouter profile lists no upstream providers",
            ));
        }

        let client = chat::client()?;
        Ok(Self {
            client,
            url: format!("{}/chat/completions", endpoint.trim_end_matches('/')),
            api_key,
            upstreams,
            reasoning,
        })
    }
}

/// The request body; every routing choice is explicit.
pub fn request_body(
    request: &ModelRequest,
    upstreams: &[String],
    reasoning: Option<ReasoningEffort>,
) -> Value {
    let messages = chat::messages(request);
    let mut schema = request.schema.clone();
    if let Some(object) = schema.as_object_mut() {
        object.remove("$schema");
    }

    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": true,
        "response_format": {
            "type": "json_schema",
            "json_schema": { "name": request.schema_name, "strict": true, "schema": schema },
        },
        "provider": {
            "order": upstreams,
            "allow_fallbacks": false,
            "require_parameters": true,
            "data_collection": "deny",
        },
    });
    if let Some(max) = request.max_output_tokens {
        body["max_tokens"] = json!(max);
    }

    // With `require_parameters` an upstream that cannot bound reasoning is not used.
    if let Some(effort) = reasoning {
        body["reasoning"] = json!({ "effort": effort.as_str() });
    }

    body
}

/// Maps one HTTP response to a reply or a typed error; a 200 can still carry an error.
pub fn parse_reply(
    status: u16,
    retry_after: Option<u32>,
    text: &str,
) -> Result<ModelReply, ProviderError> {
    chat::parse_reply(VENDOR, status, retry_after, text)
}

/// Folds a streamed reply; see [`chat::parse_stream`].
pub fn parse_stream(
    status: u16,
    retry_after: Option<u32>,
    text: &str,
) -> Result<ModelReply, ProviderError> {
    chat::parse_stream(VENDOR, status, retry_after, text)
}

impl ModelProvider for OpenRouter {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelReply, ProviderError> {
        let http = self
            .client
            .post(&self.url)
            .bearer_auth(&self.api_key)
            .header("X-Title", "sych")
            .json(&request_body(request, &self.upstreams, self.reasoning));
        chat::send(VENDOR, http).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_slugs_are_pinned() {
        assert!(pinned_model("deepseek/deepseek-v4.1-flash"));
        assert!(!pinned_model("openrouter/auto"));
        assert!(!pinned_model("~anthropic/claude-latest"));
        assert!(!pinned_model("deepseek-chat"));
    }
}
