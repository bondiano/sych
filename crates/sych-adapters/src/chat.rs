//! OpenAI-compatible chat completions over SSE, shared by the `OpenRouter` (M1.9) and
//! `OpenCode` Go (M1.12) adapters. Each vendor builds its own body and headers; the stream is
//! folded, mapped to typed errors and its usage normalized here.

use std::time::Duration;

use reqwest::redirect::Policy;
use reqwest::{Client, RequestBuilder};
use serde::Deserialize;
use serde_json::{Number, Value, json};
use sych_core::model::{
    ErrorKind, Finish, ModelReply, ModelRequest, ObservedUsage, ProviderError, Speaker,
};
use sych_core::{MicroUsd, redact};

/// Model request timeout (spec §17.1).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);

/// No redirects or system proxy: credentials go only to the allowlisted endpoint.
pub fn client() -> Result<Client, ProviderError> {
    Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .redirect(Policy::none())
        .no_proxy()
        .build()
        .map_err(|e| config_error(&format!("http client: {e}")))
}

pub fn config_error(message: &str) -> ProviderError {
    ProviderError {
        kind: ErrorKind::Config,
        dispatched: false,
        message: message.into(),
        retry_after_seconds: None,
    }
}

pub fn messages(request: &ModelRequest) -> Vec<Value> {
    request
        .messages
        .iter()
        .map(|m| {
            let role = match m.speaker {
                Speaker::System => "system",
                Speaker::User => "user",
                Speaker::Assistant => "assistant",
            };
            json!({ "role": role, "content": m.content })
        })
        .collect()
}

/// Exact decimal USD (as JSON prints it) to micro-USD, rounded up; never through a float.
pub fn usd_to_micro(number: &str) -> Option<MicroUsd> {
    let (mantissa, exponent) = match number.split_once(['e', 'E']) {
        Some((m, e)) => (m, e.parse::<i32>().ok()?),
        None => (number, 0),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{whole}{fraction}");
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }

    let value: u128 = digits.parse().ok()?;
    let shift = exponent + 6 - i32::try_from(fraction.len()).ok()?;
    let micro = if shift >= 0 {
        10u128
            .checked_pow(shift.unsigned_abs())
            .and_then(|p| value.checked_mul(p))
            .unwrap_or(u128::MAX)
    } else {
        10u128
            .checked_pow(shift.unsigned_abs())
            .map_or(u128::from(value > 0), |p| value.div_ceil(p))
    };
    Some(MicroUsd(u64::try_from(micro).unwrap_or(u64::MAX)))
}

fn micro(number: Option<&Number>) -> Option<MicroUsd> {
    number.and_then(|n| usd_to_micro(&n.to_string()))
}

#[derive(Deserialize)]
struct Body {
    id: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    choices: Option<Vec<Choice>>,
    usage: Option<Usage>,
    error: Option<ApiError>,
}

#[derive(Deserialize)]
struct Choice {
    finish_reason: Option<String>,
    message: Option<Message>,
    error: Option<ApiError>,
}

#[derive(Deserialize)]
struct Message {
    content: Option<String>,
    refusal: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    cost: Option<Number>,
    completion_tokens_details: Option<Details>,
    /// `OpenCode` billing fields; other vendors do not send them.
    opencode: Option<Opencode>,
}

#[derive(Deserialize)]
struct Details {
    reasoning_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct Opencode {
    billing_source: Option<String>,
    allowance: Option<Number>,
    list_cost: Option<Number>,
}

#[derive(Deserialize)]
struct ApiError {
    code: Option<Value>,
    message: Option<String>,
}

/// Kind and whether the request may have been processed, by HTTP status.
fn status_kind(status: u16) -> (ErrorKind, bool) {
    match status {
        401 => (ErrorKind::Auth, false),
        402 => (ErrorKind::QuotaExhausted, false),
        403 => (ErrorKind::Entitlement, false),
        408 => (ErrorKind::Timeout, true),
        429 => (ErrorKind::RateLimited, false),
        // No upstream meets the pinned order/parameters/data policy.
        503 => (ErrorKind::Unsupported, false),
        500.. => (ErrorKind::Transient, true),
        _ => (ErrorKind::Config, false),
    }
}

pub fn provider_error(
    vendor: &str,
    kind: ErrorKind,
    dispatched: bool,
    detail: &str,
    retry_after: Option<u32>,
) -> ProviderError {
    let detail: String = detail.chars().take(300).collect();
    ProviderError {
        kind,
        dispatched,
        message: format!("{vendor}: {}", redact(&detail)),
        retry_after_seconds: retry_after,
    }
}

fn api_error(error: &ApiError, default_status: u16) -> (u16, String) {
    let status = error
        .code
        .as_ref()
        .and_then(Value::as_u64)
        .and_then(|c| u16::try_from(c).ok())
        .unwrap_or(default_status);
    (status, error.message.clone().unwrap_or_default())
}

/// Maps one HTTP response to a reply or a typed error; a 200 can still carry an error.
pub fn parse_reply(
    vendor: &str,
    status: u16,
    retry_after: Option<u32>,
    text: &str,
) -> Result<ModelReply, ProviderError> {
    let fail = |kind, dispatched, detail: &str, retry| {
        provider_error(vendor, kind, dispatched, detail, retry)
    };
    let body: Option<Body> = serde_json::from_str(text).ok();
    if status != 200 {
        let detail = body
            .as_ref()
            .and_then(|b| b.error.as_ref())
            .and_then(|e| e.message.clone())
            .unwrap_or_else(|| format!("HTTP {status}"));
        let (kind, dispatched) = status_kind(status);
        return Err(fail(kind, dispatched, &detail, retry_after));
    }

    let Some(body) = body else {
        return Err(fail(ErrorKind::Transient, true, "malformed response", None));
    };
    reply_of(vendor, retry_after, body)
}

/// Maps an accepted completion body, streamed or not, to a reply or a typed error.
fn reply_of(
    vendor: &str,
    retry_after: Option<u32>,
    body: Body,
) -> Result<ModelReply, ProviderError> {
    let fail = |kind, dispatched, detail: &str, retry| {
        provider_error(vendor, kind, dispatched, detail, retry)
    };
    if let Some(error) = &body.error {
        // Accepted, then failed upstream: it may have been billed.
        let (code, detail) = api_error(error, 502);
        return Err(fail(status_kind(code).0, true, &detail, retry_after));
    }

    let Some(choice) = body.choices.and_then(|c| c.into_iter().next()) else {
        return Err(fail(ErrorKind::Transient, true, "no choices", None));
    };
    let message = choice.message.unwrap_or(Message {
        content: None,
        refusal: None,
    });
    let finish = match (choice.finish_reason.as_deref(), &message.refusal) {
        (_, Some(_)) | (Some("content_filter"), _) => Finish::Refused,
        (Some("stop"), None) => Finish::Complete,
        (Some("error"), None) => {
            let detail = choice
                .error
                .map_or_else(String::new, |e| api_error(&e, 502).1);
            return Err(fail(ErrorKind::Transient, true, &detail, None));
        }
        (Some("tool_calls"), None) => {
            return Err(fail(
                ErrorKind::Unsupported,
                true,
                "unexpected tool call",
                None,
            ));
        }
        // `length`, a missing or an unknown reason: not a complete answer.
        _ => Finish::Truncated,
    };
    let usage = body.usage;
    let opencode = usage.as_ref().and_then(|u| u.opencode.as_ref());
    Ok(ModelReply {
        text: message.content.unwrap_or_default(),
        finish,
        resolved_model: body.model,
        upstream_provider: body.provider,
        generation_id: body.id,
        usage: usage
            .as_ref()
            .map_or_else(ObservedUsage::default, |u| ObservedUsage {
                input_tokens: u.prompt_tokens,
                output_tokens: u.completion_tokens,
                reasoning_tokens: u
                    .completion_tokens_details
                    .as_ref()
                    .and_then(|d| d.reasoning_tokens),
            }),
        reported_cost: micro(usage.as_ref().and_then(|u| u.cost.as_ref())),
        billing_source: opencode.and_then(|o| o.billing_source.clone()),
        allowance: micro(opencode.and_then(|o| o.allowance.as_ref())),
        list_cost: micro(opencode.and_then(|o| o.list_cost.as_ref())),
    })
}

#[derive(Deserialize)]
struct Chunk {
    id: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    #[serde(default)]
    choices: Vec<StreamChoice>,
    usage: Option<Usage>,
    error: Option<ApiError>,
}

#[derive(Deserialize)]
struct StreamChoice {
    delta: Option<Delta>,
    finish_reason: Option<String>,
    native_finish_reason: Option<String>,
    error: Option<ApiError>,
}

#[derive(Deserialize)]
struct Delta {
    content: Option<String>,
    refusal: Option<String>,
}

/// The generation id of a (possibly cut) stream, for later reconciliation.
fn generation_of(text: &str) -> Option<String> {
    text.lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .filter_map(|d| serde_json::from_str::<Chunk>(d.trim()).ok())
        .find_map(|c| c.id)
}

/// Folds a server-sent event stream into the body a non-streamed reply has, then maps it like
/// [`parse_reply`] does. A stream that never finished is a dispatched failure, not a partial
/// answer; a non-200 response is a plain JSON error.
pub fn parse_stream(
    vendor: &str,
    status: u16,
    retry_after: Option<u32>,
    text: &str,
) -> Result<ModelReply, ProviderError> {
    if status != 200 {
        return parse_reply(vendor, status, retry_after, text);
    }

    let (mut id, mut model, mut provider, mut usage, mut error) = (None, None, None, None, None);
    let (mut content, mut refusal, mut finish, mut choice_error) =
        (String::new(), None::<String>, None, None);
    let mut native = None;
    let mut done = false;
    // Keep-alive comments (`: OPENROUTER PROCESSING`) and blank lines carry no data.
    for data in text.lines().filter_map(|l| l.strip_prefix("data:")) {
        let data = data.trim();
        if data == "[DONE]" {
            done = true;
            continue;
        }

        let Ok(chunk) = serde_json::from_str::<Chunk>(data) else {
            return Err(provider_error(
                vendor,
                ErrorKind::Transient,
                true,
                "malformed stream chunk",
                None,
            ));
        };
        id = id.or(chunk.id);
        model = model.or(chunk.model);
        provider = provider.or(chunk.provider);
        usage = chunk.usage.or(usage);
        error = chunk.error.or(error);
        for c in chunk.choices {
            if let Some(delta) = c.delta {
                content.push_str(delta.content.as_deref().unwrap_or_default());
                if let Some(r) = delta.refusal {
                    refusal.get_or_insert_with(String::new).push_str(&r);
                }
            }

            finish = c.finish_reason.or(finish);
            native = c.native_finish_reason.or(native);
            choice_error = c.error.or(choice_error);
        }
    }
    if !done && finish.is_none() && error.is_none() {
        let generation = id.as_deref().unwrap_or("unknown");
        return Err(provider_error(
            vendor,
            ErrorKind::Transient,
            true,
            &format!("stream ended early (generation {generation})"),
            None,
        ));
    }

    if refusal.is_some() || !matches!(finish.as_deref(), Some("stop")) {
        // Refusal text is model output that may quote source or secrets, so only its presence
        // is logged; the generation id finds the rest.
        let refused = refusal.is_some();
        tracing::warn!(vendor, ?finish, ?native, refused, generation = ?id, "reply did not stop normally");
    }

    let body = Body {
        id,
        model,
        provider,
        usage,
        error,
        choices: Some(vec![Choice {
            finish_reason: finish,
            message: Some(Message {
                content: Some(content),
                refusal,
            }),
            error: choice_error,
        }]),
    };
    reply_of(vendor, retry_after, body)
}

/// Sends a prepared streaming request and folds its reply; transport failures are typed by
/// whether the request could have reached the vendor.
pub async fn send(vendor: &str, request: RequestBuilder) -> Result<ModelReply, ProviderError> {
    let mut response = match request.send().await {
        Ok(response) => response,
        Err(e) => {
            let kind = if e.is_timeout() {
                ErrorKind::Timeout
            } else {
                ErrorKind::Transient
            };
            // A failed connect never reached the provider; anything later may have.
            return Err(provider_error(
                vendor,
                kind,
                !e.is_connect(),
                "transport failure",
                None,
            ));
        }
    };
    let status = response.status();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok());
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => bytes.extend_from_slice(&chunk),
            Ok(None) => break,
            Err(e) => {
                // The generation id seen so far lets the charge be reconciled later.
                let seen = String::from_utf8_lossy(&bytes);
                let generation = generation_of(&seen).unwrap_or_else(|| "unknown".into());
                let (kind, what) = if e.is_timeout() {
                    (ErrorKind::Timeout, "response timed out")
                } else {
                    (ErrorKind::Transient, "response body lost")
                };
                return Err(provider_error(
                    vendor,
                    kind,
                    true,
                    &format!("{what} (generation {generation})"),
                    None,
                ));
            }
        }
    }
    parse_stream(
        vendor,
        status.as_u16(),
        retry_after,
        &String::from_utf8_lossy(&bytes),
    )
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::{Arc, Mutex};

    use tracing::subscriber::with_default;

    use super::*;

    #[test]
    fn usd_converts_exactly_and_rounds_up() {
        assert_eq!(usd_to_micro("0.00014"), Some(MicroUsd(140)));
        assert_eq!(usd_to_micro("1.4e-4"), Some(MicroUsd(140)));
        assert_eq!(usd_to_micro("0.0000001"), Some(MicroUsd(1)));
        assert_eq!(usd_to_micro("0"), Some(MicroUsd(0)));
        assert_eq!(usd_to_micro("2"), Some(MicroUsd(2_000_000)));
        assert_eq!(usd_to_micro("-1"), None);
        assert_eq!(usd_to_micro("1e400"), Some(MicroUsd(u64::MAX)));
    }

    #[test]
    fn refusal_text_stays_out_of_logs() {
        let canary = "ghp_SYNTHETICCANARY0000000000000000000000";
        let stream = format!(
            "data: {{\"id\":\"gen-1\",\"choices\":[{{\"delta\":{{\"refusal\":\"{canary}\"}},\
             \"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n"
        );
        let log = Arc::new(Mutex::new(Vec::new()));
        let sink = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || Sink(sink.clone()))
            .finish();
        let reply = with_default(subscriber, || parse_stream("test", 200, None, &stream));
        assert_eq!(reply.expect("reply").finish, Finish::Refused);
        let log = String::from_utf8_lossy(&log.lock().expect("log")).into_owned();
        assert!(
            log.contains("refused=true") && log.contains("gen-1"),
            "{log}"
        );
        assert!(!log.contains(canary), "{log}");
    }

    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().expect("log").extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
