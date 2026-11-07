//! The playground: a maintainer tries a review of a pull request without publishing it. The run
//! shares the change's generation, supersedes nothing and stays out of lineage and analytics;
//! its spend counts against `budgets.playground_daily` as well as the tenant pools.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{Accepted, RunStatus, component};

/// `POST /v1/changes/:id/playground`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlaygroundRequest {
    /// `deep` for the deep profile; the default profile otherwise.
    #[serde(default)]
    pub profile: Option<String>,
}

/// The playground paths of `/v1`, registering their components in `c`; `op` as for the reads.
pub(super) fn paths(
    c: &mut Map<String, Value>,
    op: &dyn Fn(&str, bool, &Value) -> Value,
) -> Map<String, Value> {
    let request = component::<PlaygroundRequest>(c, "PlaygroundRequest");
    let accepted = component::<Accepted>(c, "Accepted");
    let runs = component::<Vec<RunStatus>>(c, "RunStatuses");
    let mut start = op(
        "Try a review of the pull request without publishing it; 202 is durable admission",
        true,
        &accepted,
    );
    start["requestBody"] =
        json!({"required": false, "content": {"application/json": {"schema": request}}});
    if let Some(responses) = start["responses"].as_object_mut()
        && let Some(ok) = responses.remove("200")
    {
        responses.insert("202".into(), ok);
    }

    let mut list = op(
        "Playground runs of readable repositories, the 50 newest",
        false,
        &runs,
    );
    list["parameters"] = json!([{"name": "repository_id", "in": "query", "required": false,
        "description": "only this repository", "schema": {"type": "integer"}}]);
    let paths = json!({
        "/v1/changes/{id}/playground": {"post": start},
        "/v1/playground": {"get": list},
    });
    match paths {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}
