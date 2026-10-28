//! `/v1` request and response documents (spec §25): the DTOs the server parses and writes, and
//! the `OpenAPI` description generated from them (spec §25.4).

use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::config::CheckRequest;
use crate::{ReviewResult, Timestamp};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangeRef {
    pub connection_id: String,
    /// `owner/name`.
    pub repository: String,
    pub external_number: String,
}

/// `POST /v1/reviews`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewRequest {
    pub schema_version: u32,
    pub change: ChangeRef,
    /// `economical` or `deep`.
    #[serde(default = "economical")]
    pub profile: String,
    /// Off by default; on only where the connection publishes.
    #[serde(default)]
    pub publish: bool,
    /// Execution profiles to run with their parameters instead of every allowed one; the
    /// server's profiles still decide whether and how each runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<CheckRequest>,
}

fn economical() -> String {
    "economical".into()
}

/// 202 of `POST /v1/reviews` and reruns: durable admission, not a finished review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Accepted {
    pub request_id: String,
    pub run_id: Option<String>,
    pub status_url: String,
    /// The answer replays an earlier request with the same key.
    pub reused: bool,
}

/// `POST /v1/changes/:id/reruns`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RerunRequest {
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub publish: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunUsage {
    pub model_attempts: i64,
    pub cost_known_micro_usd: i64,
    pub unknown_cost_attempts: i64,
}

/// `GET /v1/runs/:id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunStatus {
    pub run_id: String,
    pub change_id: i64,
    pub generation: i64,
    pub state: String,
    pub stage: String,
    pub coverage: Option<String>,
    pub usage: RunUsage,
    pub failure: Option<String>,
    pub created_at: i64,
    pub finished_at: Option<i64>,
    pub result_url: String,
}

/// `POST /v1/runs/:id/cancel`: the state after the call; a committed run keeps its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CancelOutcome {
    pub run_id: String,
    pub state: Option<String>,
}

/// `POST /v1/findings/:id/feedback`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FeedbackRequest {
    /// `USEFUL`, `NOT_USEFUL`, `FALSE_POSITIVE` or `WONT_FIX`.
    pub judgement: String,
    pub scope: String,
    pub rationale: String,
}

/// `POST /v1/waivers`: a scoped risk decision; it never rewrites a status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaiverRequest {
    pub change_id: i64,
    pub rule_id: String,
    pub paths: Vec<String>,
    pub reason: String,
    pub owner: String,
    pub expires_at: Option<Timestamp>,
    pub until: Option<String>,
}

/// `GET /v1/publications/:id`: delivery and relevance kept apart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PublicationStatus {
    pub publication_id: i64,
    pub delivery: String,
    pub relevance: String,
    pub remote_id: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub correlation_id: String,
    pub details: Value,
}

/// Every refused request (spec §25.3): no tokens, prompts, source or stack traces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

fn component<T: JsonSchema>(components: &mut Map<String, Value>, name: &str) -> Value {
    let mut schema = serde_json::to_value(schema_for!(T)).unwrap_or(Value::Null);
    if let Some(defs) = schema
        .as_object_mut()
        .and_then(|o| o.remove("$defs"))
        .and_then(|d| d.as_object().cloned())
    {
        for (k, v) in defs {
            components.insert(k, v);
        }
    }

    if let Some(o) = schema.as_object_mut() {
        o.remove("$schema");
    }

    components.insert(name.into(), schema);
    json!({ "$ref": format!("#/components/schemas/{name}") })
}

/// The `OpenAPI` 3.1 description of `/v1`, from the same DTOs the server uses.
pub fn openapi() -> Value {
    let mut c = Map::new();
    let review = component::<ReviewRequest>(&mut c, "ReviewRequest");
    let accepted = component::<Accepted>(&mut c, "Accepted");
    let rerun = component::<RerunRequest>(&mut c, "RerunRequest");
    let status = component::<RunStatus>(&mut c, "RunStatus");
    let result = component::<ReviewResult>(&mut c, "ReviewResult");
    let cancel = component::<CancelOutcome>(&mut c, "CancelOutcome");
    let feedback = component::<FeedbackRequest>(&mut c, "FeedbackRequest");
    let waiver = component::<WaiverRequest>(&mut c, "WaiverRequest");
    let publication = component::<PublicationStatus>(&mut c, "PublicationStatus");
    let error = component::<ErrorEnvelope>(&mut c, "ErrorEnvelope");
    let errors = |codes: &[u16]| -> Map<String, Value> {
        codes
            .iter()
            .map(|code| {
                (
                    code.to_string(),
                    json!({"description": "error envelope",
                    "content": {"application/json": {"schema": error}}}),
                )
            })
            .collect()
    };
    let op = |summary: &str,
              body: Option<&Value>,
              ok: u16,
              response: &Value,
              mutating: bool,
              fail: &[u16]| {
        let mut responses = errors(fail);
        responses.insert(
            ok.to_string(),
            json!({"description": summary,
            "content": {"application/json": {"schema": response}}}),
        );
        let mut o =
            json!({"summary": summary, "security": [{"bearer": []}], "responses": responses});
        if let Some(body) = body {
            o["requestBody"] =
                json!({"required": true, "content": {"application/json": {"schema": body}}});
        }

        if mutating {
            o["parameters"] = json!([{"name": "Idempotency-Key", "in": "header", "required": true,
                "schema": {"type": "string", "maxLength": 200}}]);
        }

        o
    };
    let created = json!({"type": "object"});
    json!({
        "openapi": "3.1.0",
        "info": {"title": "Sych", "version": env!("CARGO_PKG_VERSION")},
        "components": {"schemas": c, "securitySchemes": {"bearer": {"type": "http", "scheme": "bearer"}}},
        "paths": {
            "/v1/reviews": {"post": op("Request a review; 202 is durable admission", Some(&review), 202, &accepted, true, &[400, 401, 403, 404, 409, 413, 429, 503])},
            "/v1/runs/{id}": {"get": op("Run state, stage, usage and links", None, 200, &status, false, &[401, 403, 404, 503])},
            "/v1/runs/{id}/result": {"get": op("The immutable result", None, 200, &result, false, &[401, 403, 404, 409, 503])},
            "/v1/runs/{id}/cancel": {"post": op("Cancel an active run; committed results keep their state", None, 200, &cancel, true, &[400, 401, 403, 404, 409, 503])},
            "/v1/changes/{id}/reruns": {"post": op("An explicit new generation", Some(&rerun), 202, &accepted, true, &[400, 401, 403, 404, 409, 429, 503])},
            "/v1/findings/{id}/feedback": {"post": op("Structured feedback", Some(&feedback), 201, &created, true, &[400, 401, 403, 404, 409, 503])},
            "/v1/waivers": {"post": op("A scoped waiver", Some(&waiver), 201, &created, true, &[400, 401, 403, 404, 409, 503])},
            "/v1/publications/{id}": {"get": op("Delivery, relevance and reconciliation state", None, 200, &publication, false, &[401, 403, 404, 503])},
        }
    })
}
