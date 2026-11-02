//! Analytics over the repositories a caller may read: how people judged findings, how the
//! findings of finished pull requests settled, and why reviews stopped short. Counts only: a share is the reader's to compute,
//! shown beside its `n`. Times are Unix seconds.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::component;
use super::views::Count;

/// Findings of one rule in one repository that people judged the same way; a person's latest
/// judgement of a finding counts once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Judged {
    /// `owner/name`.
    pub repository: String,
    pub rule_id: Option<String>,
    pub category: String,
    /// `None` for findings recorded before severity was stored.
    pub severity: Option<String>,
    /// `USEFUL`, `NOT_USEFUL`, `FALSE_POSITIVE` or `WONT_FIX`.
    pub judgement: String,
    pub count: i64,
}

/// Merged pull requests with open findings of one severity, among findings updated in the
/// period; a pull request counts once per severity it left open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SeverityCount {
    pub severity: Option<String>,
    pub changes: i64,
}

/// `GET /v1/analytics/quality`: since `since`, by the time of the judgement or of the finding's
/// last update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Quality {
    pub since: i64,
    pub judgements: Vec<Judged>,
    /// Findings of merged or closed pull requests by state, without `OBSOLETE`: the addressed
    /// rate is `RESOLVED` over all of them.
    pub settled: Vec<Count>,
    pub merged_with_open: Vec<SeverityCount>,
}

/// Runs of one UTC day; `partial` did not cover their whole scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CoverageDay {
    /// The day's start.
    pub day: i64,
    pub runs: i64,
    pub partial: i64,
}

/// `GET /v1/analytics/honesty`: runs created since `since` that finished a review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Honesty {
    pub since: i64,
    /// Runs by how the plan ended: `PLAN_EXECUTED`, `LIMIT_REACHED`, `RETRIES_EXHAUSTED` or
    /// `INPUT_UNAVAILABLE`.
    pub outcomes: Vec<Count>,
    /// Paths excluded or left unreviewed, by reason code.
    pub gaps: Vec<Count>,
    /// `UNKNOWN` obligations by reason code.
    pub unknown: Vec<Count>,
    /// Runs completed before reasons were recorded: in none of the counts above.
    pub unrecorded_runs: i64,
    pub coverage_by_day: Vec<CoverageDay>,
}

/// The analytics paths of `/v1`, registering their components in `c`; `op` as for the reads.
pub(super) fn paths(
    c: &mut Map<String, Value>,
    op: &dyn Fn(&str, bool, &Value) -> Value,
) -> Map<String, Value> {
    let quality = component::<Quality>(c, "Quality");
    let honesty = component::<Honesty>(c, "Honesty");
    let period = json!([
        {"name": "repository_id", "in": "query", "required": false,
            "description": "only this repository", "schema": {"type": "integer"}},
        {"name": "since", "in": "query", "required": false,
            "description": "Unix seconds; 30 days ago by default", "schema": {"type": "integer"}},
    ]);
    let mut get_quality = op(
        "Judgements and settled findings over readable repositories",
        false,
        &quality,
    );
    get_quality["parameters"] = period.clone();
    let mut get_honesty = op(
        "Why reviews over readable repositories stopped short",
        false,
        &honesty,
    );
    get_honesty["parameters"] = period;
    let paths = json!({
        "/v1/analytics/quality": {"get": get_quality},
        "/v1/analytics/honesty": {"get": get_honesty},
    });
    match paths {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}
