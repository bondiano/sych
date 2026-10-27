//! Guided tour of one saved result (spec §21, roadmap M5.1). The plan is deterministic; the
//! artifact is immutable, validated against the result and the snapshot, and rendered with
//! code read from Git, never from model text. Statuses come from the typed result only.

use std::cmp::Reverse;
use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    CodeRef, Coverage, FindingCategory, FindingId, Obligation, ObligationId, ObligationStatus,
    PolicySnapshotId, ResultId, ReviewLanguage, ReviewResult, SchemaVersion, Severity,
    SnapshotReader, SourceSnapshotId, TourId,
};

pub const PLANNER_VERSION: &str = "tour-planner@1";
pub const RENDERER_VERSION: &str = "tour-renderer@1";
/// Stops of an ordinary PR (spec §21.2); the overview is never cut to fit them.
const MIN_STOPS: usize = 3;
const MAX_STOPS: usize = 6;
/// Generation attempts per tour, the first included: one repair at most.
pub const MAX_ATTEMPTS: u8 = 2;

/// What every tour of the result must show, whatever its route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Overview {
    pub coverage: Coverage,
    /// Every HIGH and CRITICAL finding.
    pub critical_findings: Vec<FindingId>,
    /// Every required obligation left `UNKNOWN`.
    pub important_unknowns: Vec<ObligationId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Stop {
    pub id: String,
    pub title: String,
    pub finding_ids: Vec<FindingId>,
    pub obligation_ids: Vec<ObligationId>,
    pub code_refs: Vec<CodeRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TourPlan {
    pub result_id: ResultId,
    pub review_revision: u64,
    pub source_snapshot_id: SourceSnapshotId,
    pub planner_version: String,
    pub overview: Overview,
    pub stops: Vec<Stop>,
    /// Improvements and nits, apart from the route.
    pub improvements: Vec<FindingId>,
}

fn material(c: FindingCategory) -> bool {
    matches!(c, FindingCategory::Defect | FindingCategory::RuleViolation)
}

fn required_unknown(o: &Obligation) -> bool {
    o.required && matches!(o.status, ObligationStatus::Unknown { .. })
}

/// The route: material findings by severity, one stop per file, then required UNKNOWNs
/// with a location; `target` (the repository's `tour.target_steps`) is kept within 3–6.
pub fn plan(result: &ReviewResult, target: usize) -> TourPlan {
    let limit = target.clamp(MIN_STOPS, MAX_STOPS);
    let mut findings: Vec<_> = result
        .findings
        .iter()
        .filter(|f| material(f.category))
        .collect();
    findings.sort_by_key(|f| Reverse(f.severity));
    let mut stops: Vec<Stop> = Vec::new();
    for f in findings {
        let file = f.anchors.first().map(|a| a.path.to_string());
        let at = stops.iter().position(|s| {
            file.is_some() && s.code_refs.first().map(|r| r.path.to_string()) == file
        });
        match at {
            Some(i) => {
                stops[i].finding_ids.push(f.id);
                stops[i].code_refs.extend(f.anchors.iter().cloned());
            }
            None if stops.len() < limit => stops.push(Stop {
                id: String::new(),
                title: f.title.clone(),
                finding_ids: vec![f.id],
                obligation_ids: f.obligation_ids.clone(),
                code_refs: f.anchors.clone(),
            }),
            None => {}
        }
    }
    for o in result
        .obligations
        .iter()
        .filter(|o| required_unknown(o) && !o.code_refs.is_empty())
    {
        if stops.len() == limit {
            break;
        }

        stops.push(Stop {
            id: String::new(),
            title: format!("{} is not decided", o.rule_id),
            finding_ids: Vec::new(),
            obligation_ids: vec![o.id],
            code_refs: o.code_refs.clone(),
        });
    }
    for (i, s) in stops.iter_mut().enumerate() {
        s.id = format!("stop-{}", i + 1);
        let mut seen = BTreeSet::new();
        s.code_refs
            .retain(|r| seen.insert((r.path.to_string(), r.start_line, r.end_line)));
    }
    TourPlan {
        result_id: result.result_id,
        review_revision: result.review_revision,
        source_snapshot_id: result.source_snapshot_id.clone(),
        planner_version: PLANNER_VERSION.into(),
        overview: overview(result),
        stops,
        improvements: result
            .findings
            .iter()
            .filter(|f| !material(f.category))
            .map(|f| f.id)
            .collect(),
    }
}

fn overview(result: &ReviewResult) -> Overview {
    Overview {
        coverage: result.coverage,
        critical_findings: result
            .findings
            .iter()
            .filter(|f| f.severity >= Severity::High)
            .map(|f| f.id)
            .collect(),
        important_unknowns: result
            .obligations
            .iter()
            .filter(|o| required_unknown(o))
            .map(|o| o.id)
            .collect(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum QuestionKind {
    Comprehension,
    FactRequest,
    RiskDecision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Question {
    pub kind: QuestionKind,
    pub text: String,
    pub reveal: String,
}

/// Illustrative code the generator wrote; shown only under the `Example` label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExampleSnippet {
    pub label: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TourStep {
    pub id: String,
    pub title: String,
    pub purpose: String,
    pub finding_ids: Vec<FindingId>,
    pub obligation_ids: Vec<ObligationId>,
    pub code_refs: Vec<CodeRef>,
    /// Free text; it can be wrong, and it never changes a status.
    pub explanation: String,
    pub question: Option<Question>,
    #[serde(default)]
    pub example_snippets: Vec<ExampleSnippet>,
}

/// One tour of one result revision (spec §21.3); never mutated, a regeneration is a new id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TourArtifact {
    pub schema_version: SchemaVersion,
    pub tour_id: TourId,
    pub result_id: ResultId,
    pub review_revision: u64,
    pub source_snapshot_id: SourceSnapshotId,
    pub policy_snapshot_id: PolicySnapshotId,
    pub language: ReviewLanguage,
    pub planner_version: String,
    pub prompt_version: String,
    pub renderer_version: String,
    /// Route and attempts of the generation, e.g. `openrouter/deepseek, 2 attempts`.
    pub generation_provenance: String,
    pub overview: Overview,
    pub steps: Vec<TourStep>,
    #[serde(default)]
    pub limitations: Vec<String>,
}

/// Structural problems (spec §21.4); empty means valid structure, not a true explanation.
pub fn validate(
    tour: &TourArtifact,
    result: &ReviewResult,
    reader: &dyn SnapshotReader,
) -> Vec<String> {
    let mut problems = Vec::new();
    if tour.result_id != result.result_id
        || tour.review_revision != result.review_revision
        || tour.source_snapshot_id != result.source_snapshot_id
    {
        problems.push("the tour belongs to another result or revision".to_owned());
    }

    let findings: BTreeSet<FindingId> = result.findings.iter().map(|f| f.id).collect();
    let obligations: BTreeSet<ObligationId> = result.obligations.iter().map(|o| o.id).collect();
    // The overview is the typed result's, not the generator's.
    if tour.overview != overview(result) {
        problems
            .push("the overview differs from the result's critical findings and UNKNOWNs".into());
    }

    for step in &tour.steps {
        problems.extend(
            step.finding_ids
                .iter()
                .filter(|id| !findings.contains(id))
                .map(|id| format!("{}: unknown finding {id}", step.id)),
        );
        problems.extend(
            step.obligation_ids
                .iter()
                .filter(|id| !obligations.contains(id))
                .map(|id| format!("{}: unknown obligation {id}", step.id)),
        );
        for r in &step.code_refs {
            if r.snapshot_id != result.source_snapshot_id || reader.read_span(r).is_err() {
                problems.push(format!(
                    "{}: {}:{}-{} does not resolve in the snapshot",
                    step.id, r.path, r.start_line, r.end_line
                ));
            }
        }
        if step.example_snippets.iter().any(|e| e.label != "Example") {
            problems.push(format!(
                "{}: an illustrative snippet is not labelled Example",
                step.id
            ));
        }
    }
    problems
}

/// The lines a step shows, read from the pinned blobs.
pub fn render(step: &TourStep, reader: &dyn SnapshotReader) -> Vec<(CodeRef, String)> {
    step.code_refs
        .iter()
        .filter_map(|r| reader.read_span(r).ok().map(|s| (r.clone(), s.text)))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TourState {
    NotRequested,
    Generating { attempt: u8 },
    Validating { attempt: u8 },
    Ready,
    Failed { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TourEvent {
    Requested,
    Generated,
    Valid,
    Invalid,
    Error,
}

/// Spec §21.5: an invalid tour gets one repair, the second invalid one fails.
pub fn next(state: &TourState, event: TourEvent) -> TourState {
    use TourEvent::{Error, Generated, Invalid, Requested, Valid};
    use TourState::{Failed, Generating, NotRequested, Ready, Validating};
    match (state, event) {
        (NotRequested | Failed { .. }, Requested) => Generating { attempt: 1 },
        (Generating { attempt }, Generated) => Validating { attempt: *attempt },
        (Validating { .. }, Valid) => Ready,
        (Validating { attempt }, Invalid) if *attempt < MAX_ATTEMPTS => Generating {
            attempt: attempt + 1,
        },
        (Validating { .. }, Invalid) => Failed {
            reason: "still invalid after the repair".into(),
        },
        (Generating { .. }, Error) => Failed {
            reason: "generation failed".into(),
        },
        (s, _) => s.clone(),
    }
}

/// A tour of an older revision is `STALE`; it stays readable and is never rewritten.
pub fn stale(tour: &TourArtifact, current_revision: u64) -> bool {
    tour.review_revision != current_revision
}
