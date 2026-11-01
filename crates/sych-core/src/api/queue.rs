//! The operations view of the dashboard: the jobs that carry runs, with the run each one serves,
//! and publications that wait for a reconciler or an operator. Times are Unix seconds.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::views::{Page, Repository};
use super::{PublicationStatus, component};

/// `GET /v1/jobs`: one unit of a run's work and the run it serves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobEntry {
    pub id: i64,
    pub kind: String,
    /// `QUEUED`, `CLAIMED`, `DONE`, `FAILED` or `CANCELLED`.
    pub state: String,
    pub attempts: i64,
    pub max_attempts: i64,
    pub next_attempt_at: i64,
    /// Until when a worker holds a `CLAIMED` job.
    pub lease_until: Option<i64>,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub finished_at: Option<i64>,
    pub run_id: String,
    pub run_state: String,
    /// Why the run failed, once it has.
    pub run_failure: Option<String>,
    pub change_id: i64,
    pub number: Option<i64>,
    pub repository: Repository,
}

/// `GET /v1/publications`: a publication operation no one has settled yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnresolvedPublication {
    /// The operation's id, as `GET /v1/publications/:id` takes it.
    pub id: i64,
    pub change_id: i64,
    pub number: Option<i64>,
    pub repository: Repository,
    pub slot: String,
    /// `PUBLISHING`, or `UNCERTAIN` when it may have reached GitHub.
    pub state: String,
    pub attempts: i64,
    pub created_at: i64,
    /// When the reconciler stops looking for the object and an operator decides.
    pub reconcile_deadline: Option<i64>,
    pub last_error: Option<String>,
}

/// `POST /v1/publications/:id/reconcile`: create an `UNCERTAIN` publication again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReconcileRequest {
    /// Recorded in audit with the duplicate risk the decision accepts.
    pub reason: String,
}

/// The operations paths of `/v1`, registering their components in `c`; `op` as for the reads.
pub(super) fn paths(
    c: &mut Map<String, Value>,
    op: &dyn Fn(&str, bool, &Value) -> Value,
) -> Map<String, Value> {
    let jobs = component::<Page<JobEntry>>(c, "JobPage");
    let unresolved = component::<Vec<UnresolvedPublication>>(c, "UnresolvedPublications");
    let request = component::<ReconcileRequest>(c, "ReconcileRequest");
    let status = component::<PublicationStatus>(c, "PublicationStatus");
    let mut reconcile = op(
        "Create an UNCERTAIN publication again; a duplicate comment may appear",
        true,
        &status,
    );
    reconcile["requestBody"] =
        json!({"required": true, "content": {"application/json": {"schema": request}}});
    let query = |name: &'static str, kind: &'static str, about: &'static str| {
        json!({"name": name, "in": "query", "required": false, "description": about,
            "schema": {"type": kind}})
    };
    let mut list_jobs = op("Jobs newest first, with the run each serves", false, &jobs);
    list_jobs["parameters"] = json!([
        query("repository_id", "integer", "only this repository"),
        query(
            "state",
            "string",
            "`QUEUED`, `CLAIMED`, `DONE`, `FAILED` or `CANCELLED`"
        ),
        query(
            "before",
            "integer",
            "the `next_before` of the previous page"
        ),
        query("limit", "integer", "1 to 200, 50 by default"),
    ]);
    let mut list_unresolved = op(
        "PUBLISHING and UNCERTAIN publications, oldest first",
        false,
        &unresolved,
    );
    list_unresolved["parameters"] =
        json!([query("repository_id", "integer", "only this repository")]);
    let paths = json!({
        "/v1/jobs": {"get": list_jobs},
        "/v1/publications": {"get": list_unresolved},
        "/v1/publications/{id}/reconcile": {"post": reconcile},
    });
    match paths {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}
