//! Rule suggestions from feedback: proposed edits of a repository's `.sych/rules.yaml` that a
//! maintainer applies by a pull request of their own, or sets aside.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::component;

/// One proposed edit of a rule of the repository's current policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Suggestion {
    /// Stable while the rule's version and the proposed edit stay the same.
    pub fingerprint: String,
    pub rule_id: String,
    pub rule_version: u32,
    /// `EXCEPTION`: add an expiring exception for `paths`; `DEMOTE`: make the rule not required.
    pub kind: String,
    pub paths: Vec<String>,
    /// Judgements against the rule: false positive, and for an exception won't fix too.
    pub against: i64,
    pub judged: i64,
    pub finding_ids: Vec<i64>,
    /// The YAML to put into `.sych/rules.yaml`, built only from the policy, counts and dates.
    pub yaml: String,
    /// `PENDING`, `DISMISSED` or `DONE`.
    pub state: String,
    /// The pull request that applied it, for `DONE`.
    pub pull_request: Option<String>,
}

/// `POST /v1/repositories/:id/suggestions/:fingerprint/decision`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SuggestionDecision {
    /// `DISMISSED` or `DONE`.
    pub state: String,
    /// For `DONE`: the pull request to `.sych/` that applies it.
    #[serde(default)]
    pub pull_request: Option<String>,
}

/// The suggestion paths of `/v1`, registering their components in `c`; `op` as for the reads.
pub(super) fn paths(
    c: &mut Map<String, Value>,
    op: &dyn Fn(&str, bool, &Value) -> Value,
) -> Map<String, Value> {
    let list = component::<Vec<Suggestion>>(c, "Suggestions");
    let one = component::<Suggestion>(c, "Suggestion");
    let decision = component::<SuggestionDecision>(c, "SuggestionDecision");
    let mut decide = op(
        "Set a suggestion aside or mark it applied by a pull request",
        true,
        &one,
    );
    decide["requestBody"] =
        json!({"required": true, "content": {"application/json": {"schema": decision}}});
    let paths = json!({
        "/v1/repositories/{id}/suggestions": {"get": op(
            "Rule suggestions from the feedback of the last 90 days against the current policy",
            false,
            &list,
        )},
        "/v1/repositories/{id}/suggestions/{fingerprint}/decision": {"post": decide},
    });
    match paths {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}
