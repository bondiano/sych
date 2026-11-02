//! Analytics over the repositories a caller may read: how people judged findings, how the
//! findings of finished pull requests settled, why reviews stopped short, what they cost and
//! how long they took. Counts only: a share is the reader's to compute,
//! shown beside its `n`. Times are Unix seconds.

use jiff::tz::TimeZone;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::component;
use super::report::WeeklyReport;
use super::views::Count;
use crate::Timestamp;

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

/// Model calls of one repository, model or purpose; an unknown charge is counted, never added
/// as zero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CostRow {
    pub key: String,
    pub attempts: i64,
    pub cost_known_micro_usd: i64,
    pub unknown_cost_attempts: i64,
}

/// Model calls of one pull request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChangeCost {
    pub change_id: i64,
    /// `owner/name`.
    pub repository: String,
    pub number: Option<i64>,
    pub attempts: i64,
    pub cost_known_micro_usd: i64,
    pub unknown_cost_attempts: i64,
}

/// The tenant's spend against its budgets today and this month (UTC): settled charges and
/// reservations still held, as the ledger counts them before a call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BudgetUse {
    pub day_spent_micro_usd: i64,
    pub day_limit_micro_usd: Option<i64>,
    pub month_spent_micro_usd: i64,
    pub month_limit_micro_usd: Option<i64>,
    /// The month's spend so far over its elapsed days, times the days of the month.
    pub month_forecast_micro_usd: i64,
}

/// `GET /v1/analytics/cost`: model calls made since `since`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CostBreakdown {
    pub since: i64,
    pub by_repository: Vec<CostRow>,
    /// By the model that answered, else the one requested.
    pub by_model: Vec<CostRow>,
    pub by_purpose: Vec<CostRow>,
    /// The ten pull requests with the highest known cost.
    pub top_changes: Vec<ChangeCost>,
    /// Known cost of runs a newer revision superseded.
    pub superseded_micro_usd: i64,
    /// Findings someone judged `USEFUL` in the period, for the cost per useful finding.
    pub useful_findings: i64,
    /// Only for a caller of every repository without `repository_id`: budgets are the tenant's.
    pub budget: Option<BudgetUse>,
}

/// Durations of one kind in seconds; percentiles by nearest rank, `None` without samples.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Spread {
    pub n: i64,
    pub p50_seconds: Option<i64>,
    pub p90_seconds: Option<i64>,
}

impl Spread {
    pub fn of(mut seconds: Vec<i64>) -> Self {
        seconds.sort_unstable();
        let rank = |p: usize| {
            let n = seconds.len();
            (n > 0).then(|| seconds[(n * p).div_ceil(100).max(1) - 1])
        };
        Self {
            n: i64::try_from(seconds.len()).unwrap_or(i64::MAX),
            p50_seconds: rank(50),
            p90_seconds: rank(90),
        }
    }
}

/// `GET /v1/analytics/latency`: runs and jobs created since `since`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Latency {
    pub since: i64,
    /// From a run's creation to its first delivered publication.
    pub to_delivery: Spread,
    /// From a run's creation to its completion.
    pub run_duration: Spread,
    /// From a job's creation to its first claim; jobs are kept 30 days.
    pub queue_wait: Spread,
}

/// The UTC month's `spent` so far at `at`, extended over its remaining days.
pub fn month_forecast(spent: i64, at: Timestamp) -> i64 {
    let date = at.to_zoned(TimeZone::UTC).date();
    spent.saturating_mul(i64::from(date.days_in_month())) / i64::from(date.day().max(1))
}

/// The analytics paths of `/v1`, registering their components in `c`; `op` as for the reads.
pub(super) fn paths(
    c: &mut Map<String, Value>,
    op: &dyn Fn(&str, bool, &Value) -> Value,
) -> Map<String, Value> {
    let quality = component::<Quality>(c, "Quality");
    let honesty = component::<Honesty>(c, "Honesty");
    let cost = component::<CostBreakdown>(c, "CostBreakdown");
    let latency = component::<Latency>(c, "Latency");
    let weekly = component::<WeeklyReport>(c, "WeeklyReport");
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
    get_honesty["parameters"] = period.clone();
    let mut get_cost = op(
        "Model cost over readable repositories, and the tenant's budget use",
        false,
        &cost,
    );
    get_cost["parameters"] = period.clone();
    let mut get_latency = op(
        "Time to delivery, run duration and queue wait",
        false,
        &latency,
    );
    get_latency["parameters"] = period;
    let mut get_weekly = op(
        "The analytics of the last seven days as JSON or Markdown",
        false,
        &weekly,
    );
    get_weekly["parameters"] = json!([
        {"name": "repository_id", "in": "query", "required": false,
            "description": "only this repository", "schema": {"type": "integer"}},
        {"name": "format", "in": "query", "required": false,
            "description": "`json` (default) or `markdown`, served as text/markdown",
            "schema": {"type": "string", "enum": ["json", "markdown"]}},
    ]);
    let paths = json!({
        "/v1/analytics/quality": {"get": get_quality},
        "/v1/analytics/honesty": {"get": get_honesty},
        "/v1/analytics/cost": {"get": get_cost},
        "/v1/analytics/latency": {"get": get_latency},
        "/v1/reports/weekly": {"get": get_weekly},
    });
    match paths {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Spread, month_forecast};

    #[test]
    fn percentiles_by_nearest_rank() {
        let spread = Spread::of((1..=10).rev().collect());
        assert_eq!(
            (spread.n, spread.p50_seconds, spread.p90_seconds),
            (10, Some(5), Some(9))
        );
        assert_eq!(Spread::of(vec![7]).p90_seconds, Some(7));
        assert_eq!(Spread::of(Vec::new()).p50_seconds, None);
    }

    #[test]
    fn a_month_is_forecast_from_its_elapsed_days() {
        let at = |text: &str| text.parse().expect("time");
        assert_eq!(month_forecast(10_000, at("2026-11-10T12:00:00Z")), 30_000);
        assert_eq!(month_forecast(500, at("2026-12-01T00:00:00Z")), 15_500);
    }
}
