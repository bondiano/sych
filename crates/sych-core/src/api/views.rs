//! Read models of the dashboard: lists, a change and a finding with their history, a run's
//! model attempts and period totals. Times are Unix seconds; money is micro-USD.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{RunStatus, component};
use crate::Finding;
use crate::config::ApiRole;

/// One page, newest first; pass `next_before` as `before` for the next one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// `None` on the last page.
    pub next_before: Option<i64>,
}

/// A repository the caller may read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Repository {
    pub id: i64,
    pub connection_id: String,
    /// `owner/name`.
    pub full_name: String,
}

/// A readable repository and the caller's role on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Grant {
    pub repository: Repository,
    pub role: ApiRole,
}

/// `GET /v1/me`: who the caller is and what it may do where.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Me {
    /// `github:<login>` for a person, the client name for an API client.
    pub actor: String,
    /// `user` or `client`.
    pub kind: String,
    pub tenant_id: String,
    /// A person's roles as GitHub granted them at sign-in; every change is checked again live.
    pub repositories: Vec<Grant>,
}

/// `GET /auth/github`: a connection people sign in through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SignIn {
    pub connection_id: String,
    pub login_url: String,
}

/// `GET /v1/audit`: one recorded action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuditEvent {
    pub id: i64,
    pub actor: String,
    pub action: String,
    /// `run:<id>`, `finding:<id>`, `waiver:<id>`, `publication_operation:<id>` or `session`.
    pub target: String,
    /// `None` for actions of no single repository, such as signing in.
    pub repository_id: Option<i64>,
    pub metadata: serde_json::Value,
    pub at: i64,
}

/// The newest generation's run of a change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LatestRun {
    pub run_id: String,
    pub generation: i64,
    pub state: String,
    pub stage: String,
    pub coverage: Option<String>,
    /// Obligations the run could not assess; `None` before it finished.
    pub unknown_obligations: Option<i64>,
    pub cost_known_micro_usd: i64,
    pub unknown_cost_attempts: i64,
    pub created_at: i64,
    pub finished_at: Option<i64>,
}

/// `GET /v1/changes`: one pull request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChangeSummary {
    pub id: i64,
    pub repository: Repository,
    pub number: Option<i64>,
    /// `OPEN`, `DRAFT`, `MERGED` or `CLOSED`.
    pub scm_state: Option<String>,
    pub head_sha: Option<String>,
    pub synced_at: Option<i64>,
    pub latest_run: Option<LatestRun>,
    pub open_findings: i64,
}

/// One logged event of a change: generation, skip, SCM failure, repair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChangeEvent {
    pub kind: String,
    pub detail: String,
    pub run_id: Option<String>,
    pub at: i64,
}

/// One delivery attempt of a summary, inline comment or check run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PublicationEntry {
    pub id: i64,
    /// `SUMMARY`, `INLINE` or `CHECK`.
    pub kind: String,
    pub state: String,
    /// `CURRENT` or `STALE`.
    pub relevance: String,
    pub generation: i64,
    pub remote_id: Option<String>,
    pub last_error: Option<String>,
    pub attempts: i64,
    pub created_at: i64,
    pub finished_at: Option<i64>,
}

/// `GET /v1/changes/:id`; its findings are `GET /v1/findings?change_id=`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChangeDetail {
    pub change: ChangeSummary,
    /// Every generation, oldest first.
    pub runs: Vec<RunStatus>,
    pub events: Vec<ChangeEvent>,
    pub publications: Vec<PublicationEntry>,
}

/// `GET /v1/findings`: one finding's lineage row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FindingSummary {
    /// The id feedback is posted to.
    pub id: i64,
    pub change_id: i64,
    pub repository: Repository,
    pub number: Option<i64>,
    /// The finding's id inside its run's result.
    pub finding_id: String,
    pub category: String,
    /// `None` for findings recorded before severities were kept.
    pub severity: Option<String>,
    pub title: Option<String>,
    pub rule_id: Option<String>,
    pub path: Option<String>,
    /// `OPEN`, `RESOLVED`, `DISMISSED` or `OBSOLETE`.
    pub state: String,
    pub first_run_id: String,
    pub last_run_id: String,
    pub updated_at: i64,
}

/// One lifecycle transition of a finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FindingEvent {
    pub run_id: Option<String>,
    pub from_state: Option<String>,
    pub to_state: String,
    pub reason: String,
    pub actor: Option<String>,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FeedbackEntry {
    pub actor: String,
    pub judgement: String,
    pub scope: String,
    pub rationale: String,
    pub at: i64,
}

/// `GET /v1/findings/:id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FindingDetail {
    pub summary: FindingSummary,
    pub history: Vec<FindingEvent>,
    pub feedback: Vec<FeedbackEntry>,
    /// The finding as its latest run's result states it; `None` once that result was pruned.
    pub finding: Option<Finding>,
}

/// `GET /v1/runs/:id/attempts`: one model call as the ledger recorded it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Attempt {
    pub id: String,
    pub purpose: String,
    pub kind: String,
    pub profile: String,
    pub provider: String,
    pub requested_model: String,
    pub resolved_model: Option<String>,
    pub billing_kind: String,
    /// `RESERVED`, `SETTLED`, `UNKNOWN` or `RELEASED`.
    pub state: String,
    pub reserved_micro_usd: i64,
    /// `None` while unknown; never shown as zero.
    pub charge_micro_usd: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub created_at: i64,
    pub settled_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Count {
    pub key: String,
    pub count: i64,
}

/// `GET /v1/summary`: totals since `since` over the repositories the caller may read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Summary {
    pub since: i64,
    pub runs_by_state: Vec<Count>,
    pub runs_by_coverage: Vec<Count>,
    pub unknown_obligations: i64,
    /// Lifecycle transitions in the period by the state entered: found, resolved, dismissed.
    pub finding_events: Vec<Count>,
    pub cost_known_micro_usd: i64,
    /// Attempts whose charge is not known: never counted as zero cost.
    pub unknown_cost_attempts: i64,
    /// Money still reserved by attempts in flight.
    pub reserved_micro_usd: i64,
    /// Current queue, not limited to the period.
    pub queued_jobs: i64,
    pub oldest_queued_age_seconds: i64,
    pub uncertain_publications: i64,
}

/// One UTC day of model usage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageDay {
    /// `YYYY-MM-DD`.
    pub day: String,
    pub attempts: i64,
    pub cost_known_micro_usd: i64,
    pub unknown_cost_attempts: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// A fixed subscription fee of the tenant; not attributed to a repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubscriptionFee {
    pub provider: String,
    /// `YYYY-MM`.
    pub month: String,
    pub amount_micro_usd: i64,
}

/// `GET /v1/usage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Usage {
    pub from: String,
    pub to: String,
    pub days: Vec<UsageDay>,
    /// Only for a caller that reads every repository and asked for no single one.
    pub subscription_fees: Vec<SubscriptionFee>,
}

/// `GET /v1/waivers`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Waiver {
    pub id: i64,
    pub repository: Repository,
    pub rule_id: String,
    pub paths: Vec<String>,
    pub reason: String,
    pub owner: String,
    pub author: String,
    pub expires_at: Option<i64>,
    pub until: Option<String>,
    pub created_at: i64,
    pub revoked_at: Option<i64>,
}

/// An operation with optional query parameters `(name, JSON type, description)`.
fn with_query(mut operation: Value, params: &[(&str, &str, &str)]) -> Value {
    operation["parameters"] = params
        .iter()
        .map(|(name, kind, about)| {
            json!({"name": name, "in": "query", "required": false, "description": about,
                "schema": {"type": kind}})
        })
        .collect();
    operation
}

/// The read paths of `/v1`, registering their components in `c`. `op` describes an operation
/// by summary, whether it mutates, and its response.
pub(super) fn paths(
    c: &mut Map<String, Value>,
    op: &dyn Fn(&str, bool, &Value) -> Value,
) -> Map<String, Value> {
    let read = |summary: &str, response: &Value| op(summary, false, response);
    let repositories = component::<Vec<Repository>>(c, "Repositories");
    let changes = component::<Page<ChangeSummary>>(c, "ChangePage");
    let change = component::<ChangeDetail>(c, "ChangeDetail");
    let findings = component::<Page<FindingSummary>>(c, "FindingPage");
    let finding = component::<FindingDetail>(c, "FindingDetail");
    let attempts = component::<Vec<Attempt>>(c, "Attempts");
    let summary = component::<Summary>(c, "Summary");
    let usage = component::<Usage>(c, "Usage");
    let waivers = component::<Vec<Waiver>>(c, "Waivers");
    let waiver = component::<Waiver>(c, "Waiver");
    let me = component::<Me>(c, "Me");
    let audit = component::<Page<AuditEvent>>(c, "AuditPage");
    let sign_ins = component::<Vec<SignIn>>(c, "SignIns");
    let object = json!({"type": "object"});
    let mut sign_in = read("Connections people sign in through", &sign_ins);
    sign_in["security"] = json!([]);
    let repository = ("repository_id", "integer", "only this repository");
    let before = (
        "before",
        "integer",
        "the `next_before` of the previous page",
    );
    let limit = ("limit", "integer", "1 to 200, 50 by default");
    let paths = json!({
        "/v1/repositories": {"get": read("Repositories the caller may read", &repositories)},
        "/v1/changes": {"get": with_query(read("Pull requests, newest first", &changes),
            &[repository, ("state", "string", "`OPEN`, `DRAFT`, `MERGED` or `CLOSED`"), before, limit])},
        "/v1/changes/{id}": {"get": read("A pull request with its runs, events and publications", &change)},
        "/v1/findings": {"get": with_query(read("Findings, newest first", &findings),
            &[repository, ("change_id", "integer", "only this change"), ("state", "string", "lifecycle state"),
              ("category", "string", "finding category"), ("severity", "string", "finding severity"),
              ("rule_id", "string", "only this rule"), before, limit])},
        "/v1/findings/{id}": {"get": read("A finding with its history, feedback and stated details", &finding)},
        "/v1/runs/{id}/attempts": {"get": read("Model attempts of a run as the ledger recorded them", &attempts)},
        "/v1/summary": {"get": with_query(read("Totals since a time over readable repositories", &summary),
            &[repository, ("since", "integer", "Unix seconds; 30 days ago by default")])},
        "/v1/usage": {"get": with_query(read("Model usage by UTC day", &usage),
            &[repository, ("from", "string", "`YYYY-MM-DD`; 30 days ago by default"),
              ("to", "string", "`YYYY-MM-DD`; today by default")])},
        "/v1/waivers": {"get": with_query(read("Waivers of the repositories the caller may read", &waivers),
            &[repository, ("state", "string", "`active`, `expired` or `revoked`")])},
        "/v1/waivers/{id}/revoke": {"post": op("Revoke a waiver; a revoked one keeps its time", true, &waiver)},
        "/v1/me": {"get": read("The caller and its role on each readable repository", &me)},
        "/v1/audit": {"get": with_query(read("Recorded actions on repositories the caller maintains", &audit),
            &[("action", "string", "only this action"), before, limit])},
        "/auth/github": {"get": sign_in},
        "/auth/token": {"post": read("Exchange an API client's bearer token for a browser session", &object)},
        "/auth/logout": {"post": read("End the caller's browser session", &object)},
    });
    match paths {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}
