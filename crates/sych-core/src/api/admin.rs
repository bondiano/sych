//! The tenant's administration view for an operator who reads every repository: the GitHub
//! connections as configured, their webhook deliveries and the effective server config with
//! secret names only. Times are Unix seconds.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::component;
use super::views::{Count, Page};

/// `GET /v1/admin/connections`: one GitHub connection as the server runs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConnectionView {
    pub id: String,
    pub app_id: u64,
    pub bot_login: String,
    /// The configured `owner/name` patterns.
    pub repositories: Vec<String>,
    pub publish: bool,
    pub checks: bool,
    /// `sandbox` or `production`.
    pub environment: String,
    pub self_review: bool,
    /// The reviewer build a self-review connection is pinned to.
    pub reviewer_commit: Option<String>,
    /// The commit this server was built from.
    pub build_commit: String,
    /// The latest failure to read GitHub for a change, a sign of broken credentials.
    pub last_scm_read_failure: Option<ScmReadFailure>,
    /// Webhook deliveries of the last 30 days by admission decision.
    pub deliveries: Vec<Count>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScmReadFailure {
    pub at: i64,
    pub change_id: i64,
    pub detail: String,
}

/// `GET /v1/admin/deliveries`: one received webhook and what admission decided; no payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Delivery {
    pub id: i64,
    pub connection_id: String,
    pub delivery_id: String,
    pub event: String,
    pub action: Option<String>,
    /// `SCHEDULED`, `NO_CHANGE`, `IGNORED`, `BOT_COMMENT`, `NO_REPOSITORY` or
    /// `REPOSITORY_NOT_ALLOWED`.
    pub decision: String,
    pub received_at: i64,
}

/// The administration paths of `/v1`, registering their components in `c`; `op` as for the
/// reads.
pub(super) fn paths(
    c: &mut Map<String, Value>,
    op: &dyn Fn(&str, bool, &Value) -> Value,
) -> Map<String, Value> {
    let connections = component::<Vec<ConnectionView>>(c, "ConnectionViews");
    let deliveries = component::<Page<Delivery>>(c, "DeliveryPage");
    let object = json!({"type": "object"});
    let query = |name: &'static str, kind: &'static str, about: &'static str| {
        json!({"name": name, "in": "query", "required": false, "description": about,
            "schema": {"type": kind}})
    };
    let mut list_deliveries = op(
        "Webhook deliveries newest first, kept 30 days",
        false,
        &deliveries,
    );
    list_deliveries["parameters"] = json!([
        query("connection_id", "string", "only this connection"),
        query("decision", "string", "only this admission decision"),
        query(
            "before",
            "integer",
            "the `next_before` of the previous page"
        ),
        query("limit", "integer", "1 to 200, 50 by default"),
    ]);
    let paths = json!({
        "/v1/admin/connections": {"get": op("The tenant's GitHub connections as the server runs them", false, &connections)},
        "/v1/admin/deliveries": {"get": list_deliveries},
        "/v1/admin/config": {"get": op("The effective server config as `sych config validate` prints it", false, &object)},
    });
    match paths {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}
