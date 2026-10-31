//! `/v1` request and response documents: the DTOs the server parses and writes, and
//! the `OpenAPI` description generated from them.

use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::analysis::followup::Exchange;
use crate::config::CheckRequest;
use crate::handoff::Handoff;
use crate::tour::{QuestionKind, TourArtifact, TourSession};
use crate::{FindingId, ReviewResult, SourceSnapshotId, Timestamp};

mod views;

pub use views::*;

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
    /// server's profiles still decide whether and how each runs. Independent of `profile`,
    /// which only sets the review depth.
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

/// `GET /v1/runs/:id/tour` and the session calls: the tour as the caller's session sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TourView {
    /// Answers not revealed in this session are blanked.
    pub tour: TourArtifact,
    /// The caller's own progress; never published and never an approval.
    pub session: TourSession,
    /// A newer revision or generation of the change exists; the tour still shows what was reviewed.
    pub stale: bool,
    /// The caller's own questions on this tour, oldest first; never another caller's.
    pub exchanges: Vec<Exchange>,
}

/// `POST /v1/runs/:id/tours/:tour/reveal`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RevealRequest {
    pub step_id: String,
}

/// `POST /v1/runs/:id/tours/:tour/questions`: a bounded follow-up on one step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QuestionRequest {
    pub step_id: String,
    pub kind: QuestionKind,
    /// The question; for `FACT_REQUEST`, the caller's own statement of the missing fact.
    pub question: String,
    /// Only with `RISK_DECISION`, which needs the maintainer role.
    #[serde(default)]
    pub waiver: Option<WaiverTerms>,
}

/// `POST /v1/runs/:id/handoff`: the run's own findings to export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HandoffRequest {
    pub finding_ids: Vec<FindingId>,
    /// Refused unless the run reviewed this snapshot.
    #[serde(default)]
    pub source_snapshot_id: Option<SourceSnapshotId>,
}

/// What a waiver accepts, for which paths and until when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaiverTerms {
    pub rule_id: String,
    pub paths: Vec<String>,
    pub reason: String,
    pub owner: String,
    pub expires_at: Option<Timestamp>,
    pub until: Option<String>,
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
    /// The run this one continues, such as after late CI evidence.
    pub parent_run_id: Option<String>,
    /// Obligations the run could not assess; `None` before it finished.
    pub unknown_obligations: Option<i64>,
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

/// Every refused request: no tokens, prompts, source or stack traces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

/// `schemars` refers to `#/$defs/X`; in the document those definitions are components.
fn relink(v: &mut Value) {
    match v {
        Value::Object(o) => {
            if let Some(Value::String(r)) = o.get_mut("$ref")
                && let Some(name) = r.strip_prefix("#/$defs/")
            {
                *r = format!("#/components/schemas/{name}");
            }

            o.values_mut().for_each(relink);
        }
        Value::Array(a) => a.iter_mut().for_each(relink),
        _ => {}
    }
}

fn component<T: JsonSchema>(components: &mut Map<String, Value>, name: &str) -> Value {
    let mut schema = serde_json::to_value(schema_for!(T)).unwrap_or(Value::Null);
    relink(&mut schema);
    if let Some(defs) = schema
        .as_object_mut()
        .and_then(|o| o.remove("$defs"))
        .and_then(|d| d.as_object().cloned())
    {
        for (k, v) in defs {
            // Two Rust types with one name would silently replace each other's schema.
            if let Some(known) = components.get(&k) {
                assert_eq!(known, &v, "two different schemas are named {k}");
            }

            components.insert(k, v);
        }
    }

    // A root schema carries a `title` its copies in `$defs` do not; both describe one type.
    if let Some(o) = schema.as_object_mut() {
        o.remove("$schema");
        o.remove("title");
    }

    if let Some(known) = components.get(name) {
        assert_eq!(known, &schema, "two different schemas are named {name}");
    }

    components.insert(name.into(), schema);
    json!({ "$ref": format!("#/components/schemas/{name}") })
}

/// Adds `paths` to the document; a path present in both keeps the methods of each, as
/// `/v1/waivers` holds the POST written here and the GET of the read models.
fn merge(doc: &mut Value, paths: Map<String, Value>) {
    for (path, item) in paths {
        match (doc["paths"].get_mut(&path), item) {
            (Some(Value::Object(existing)), Value::Object(added)) => existing.extend(added),
            (_, item) => doc["paths"][path] = item,
        }
    }
}

/// Declares every `{name}` of each path template as a required path parameter of its
/// operations: a run and a tour are named by uuid, everything else by a numeric id.
fn path_parameters(doc: &mut Value) {
    let Some(paths) = doc["paths"].as_object_mut() else {
        return;
    };
    for (path, item) in paths {
        let declared: Vec<Value> = path
            .split('/')
            .filter_map(|s| s.strip_prefix('{')?.strip_suffix('}'))
            .map(|name| {
                let text = name != "id" || path.starts_with("/v1/runs/");
                let schema = if text {
                    json!({"type": "string"})
                } else {
                    json!({"type": "integer", "format": "int64"})
                };
                json!({"name": name, "in": "path", "required": true, "schema": schema})
            })
            .collect();
        for operation in item
            .as_object_mut()
            .into_iter()
            .flat_map(|o| o.values_mut())
        {
            let mut parameters = declared.clone();
            if let Some(Value::Array(own)) = operation.get("parameters") {
                parameters.extend(own.iter().cloned());
            }

            if !parameters.is_empty() {
                operation["parameters"] = Value::Array(parameters);
            }
        }
    }
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
    let tour = component::<TourView>(&mut c, "TourView");
    let reveal = component::<RevealRequest>(&mut c, "RevealRequest");
    let question = component::<QuestionRequest>(&mut c, "QuestionRequest");
    let exchange = component::<Exchange>(&mut c, "Exchange");
    let handoff_request = component::<HandoffRequest>(&mut c, "HandoffRequest");
    let handoff = component::<Handoff>(&mut c, "Handoff");
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
        let mut o = json!({"summary": summary, "security": [{"bearer": []}, {"session": []}], "responses": responses});
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
    let mut get_tour = op(
        "The newest stored tour (or `?tour=`) as the caller's session sees it",
        None,
        200,
        &tour,
        false,
        &[400, 401, 403, 404, 409, 503],
    );
    get_tour["parameters"] = json!([{"name": "tour", "in": "query", "required": false,
        "description": "a tour id of this run; the newest when omitted",
        "schema": {"type": "string", "format": "uuid"}}]);
    // Reads answer 200 or the usual refusals; the one mutation also conflicts.
    let views = views::paths(&mut c, &|summary, mutating, response| {
        let fail: &[u16] = if mutating {
            &[400, 401, 403, 404, 409, 503]
        } else {
            &[400, 401, 403, 404, 503]
        };
        op(summary, None, 200, response, mutating, fail)
    });
    let mut doc = json!({
        "openapi": "3.1.0",
        "info": {"title": "Sych", "version": env!("CARGO_PKG_VERSION")},
        "components": {"schemas": c, "securitySchemes": {"bearer": {"type": "http", "scheme": "bearer"},
            "session": {"type": "apiKey", "in": "cookie", "name": "sych_session"}}},
        "paths": {
            "/v1/reviews": {"post": op("Request a review; 202 is durable admission", Some(&review), 202, &accepted, true, &[400, 401, 403, 404, 409, 413, 429, 503])},
            "/v1/runs/{id}": {"get": op("Run state, stage, usage and links", None, 200, &status, false, &[401, 403, 404, 503])},
            "/v1/runs/{id}/result": {"get": op("The immutable result", None, 200, &result, false, &[401, 403, 404, 409, 503])},
            "/v1/runs/{id}/cancel": {"post": op("Cancel an active run; committed results keep their state", None, 200, &cancel, true, &[400, 401, 403, 404, 409, 503])},
            "/v1/changes/{id}/reruns": {"post": op("An explicit new generation", Some(&rerun), 202, &accepted, true, &[400, 401, 403, 404, 409, 429, 503])},
            "/v1/findings/{id}/feedback": {"post": op("Structured feedback", Some(&feedback), 201, &created, true, &[400, 401, 403, 404, 409, 503])},
            "/v1/waivers": {"post": op("A scoped waiver", Some(&waiver), 201, &created, true, &[400, 401, 403, 404, 409, 503])},
            "/v1/publications/{id}": {"get": op("Delivery, relevance and reconciliation state", None, 200, &publication, false, &[401, 403, 404, 503])},
            "/v1/runs/{id}/tour": {"get": get_tour},
            "/v1/runs/{id}/tours/{tour}/reveal": {"post": op("Reveal one step's answer in the caller's session", Some(&reveal), 200, &tour, true, &[400, 401, 403, 404, 409, 503])},
            "/v1/runs/{id}/tours/{tour}/complete": {"post": op("Complete the caller's session; not an approval", None, 200, &tour, true, &[400, 401, 403, 404, 409, 503])},
            "/v1/runs/{id}/tours/{tour}/dismiss": {"post": op("Dismiss the caller's session", None, 200, &tour, true, &[400, 401, 403, 404, 409, 503])},
            "/v1/runs/{id}/handoff": {"post": op("Export selected findings for a separate author workflow; no model, no change", Some(&handoff_request), 200, &handoff, false, &[400, 401, 403, 404, 409, 503])},
            "/v1/runs/{id}/tours/{tour}/questions": {"post": op("A bounded follow-up on one step; private, never evidence", Some(&question), 201, &exchange, true, &[400, 401, 403, 404, 409, 503])},
        }
    });
    merge(&mut doc, views);
    path_parameters(&mut doc);
    doc
}
