//! The tour generation pass (spec §21, roadmap M5.1). The model writes only the prose of the
//! stops the deterministic plan chose; ids, code references, statuses and the overview come
//! from the plan and the typed result, so it cannot invent a location or raise a severity.

use std::fmt::Write;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::{DataBlock, Prompt};
use crate::tour::{
    ExampleSnippet, PLANNER_VERSION, Question, RENDERER_VERSION, TourArtifact, TourPlan, TourStep,
};
use crate::{ReviewResult, SchemaVersion, TourId};

pub const TOUR: &str = "tour@1";

/// The model's prose for one stop of the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DraftStep {
    pub stop_id: String,
    pub title: String,
    pub purpose: String,
    pub explanation: String,
    pub question: Option<Question>,
    #[serde(default)]
    pub example_snippets: Vec<ExampleSnippet>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TourDraft {
    /// What the change is for, in one sentence, as the code and findings show it.
    pub intent: String,
    pub steps: Vec<DraftStep>,
}

const RULES: [&str; 5] = [
    "Explain only what the stop's findings, obligations and code show. Severity, support, \
     coverage and statuses are fixed by the review: never raise, lower or restate them as new \
     facts.",
    "A claim the review did not make is a hypothesis: say so in the text (\"possibly\", \"to \
     check\"), never as a finding.",
    "Write one step per stop, with its stop_id; no other stops.",
    "Illustrative code goes only into example_snippets with label \"Example\"; never quote the \
     data blocks as if they were your own code.",
    "Each text field is at most three sentences; a question is optional and has a short reveal.",
];

fn stops_text(plan: &TourPlan, result: &ReviewResult) -> String {
    let mut text = String::new();
    for stop in &plan.stops {
        let _ = writeln!(text, "- {}: {}", stop.id, stop.title);
        for f in result
            .findings
            .iter()
            .filter(|f| stop.finding_ids.contains(&f.id))
        {
            let _ = writeln!(
                text,
                "  finding ({:?}, {:?}): {} — {}",
                f.severity, f.category, f.title, f.consequence
            );
        }
        for o in result
            .obligations
            .iter()
            .filter(|o| stop.obligation_ids.contains(&o.id))
        {
            let _ = writeln!(
                text,
                "  obligation {}: {} — {:?}",
                o.rule_id, o.question, o.status
            );
        }
    }
    text
}

/// `spans` are the stops' code, read from the snapshot by the caller; `problems` are what a
/// previous answer got wrong, for the one regeneration.
pub fn prompt(
    plan: &TourPlan,
    result: &ReviewResult,
    spans: Vec<DataBlock>,
    problems: &[String],
) -> Prompt {
    let mut rules: Vec<String> = RULES.map(str::to_owned).into();
    if !problems.is_empty() {
        rules.push(format!(
            "The previous answer was rejected: {}. Fix exactly that.",
            problems.join("; ")
        ));
    }

    Prompt {
        template: TOUR.into(),
        role: "You are the guide of a code review tour. You explain a review that is already \
               done to a reader walking through the change, stop by stop."
            .into(),
        output_contract: "Reply with one JSON object with intent and steps; each step has \
                          stop_id, title, purpose, explanation, question (null or kind \
                          COMPREHENSION, FACT_REQUEST or RISK_DECISION, text, reveal) and \
                          example_snippets (label, text)."
            .into(),
        rules,
        task: format!(
            "Write the tour of these stops.\n# Stops\n{}",
            stops_text(plan, result)
        ),
        data: spans,
    }
}

/// The artifact of a draft: route, ids, code and overview from the plan; prose from the model.
/// A step for a stop the plan does not have is a problem, as is a stop left without a step.
pub fn assemble(
    plan: &TourPlan,
    result: &ReviewResult,
    draft: TourDraft,
    provenance: String,
) -> (TourArtifact, Vec<String>) {
    let mut problems = Vec::new();
    let mut steps = Vec::new();
    for d in draft.steps {
        let Some(stop) = plan.stops.iter().find(|s| s.id == d.stop_id) else {
            problems.push(format!("{} is not a stop of the plan", d.stop_id));
            continue;
        };
        steps.push(TourStep {
            id: stop.id.clone(),
            title: d.title,
            purpose: d.purpose,
            finding_ids: stop.finding_ids.clone(),
            obligation_ids: stop.obligation_ids.clone(),
            code_refs: stop.code_refs.clone(),
            explanation: d.explanation,
            question: d.question,
            example_snippets: d.example_snippets,
        });
    }
    problems.extend(
        plan.stops
            .iter()
            .filter(|s| !steps.iter().any(|t| t.id == s.id))
            .map(|s| format!("{} has no step", s.id)),
    );
    let tour = TourArtifact {
        schema_version: SchemaVersion,
        tour_id: TourId::new_v7(),
        result_id: result.result_id,
        review_revision: result.review_revision,
        source_snapshot_id: result.source_snapshot_id.clone(),
        policy_snapshot_id: result.policy_snapshot_id.clone(),
        language: result.language,
        planner_version: PLANNER_VERSION.into(),
        prompt_version: TOUR.into(),
        renderer_version: RENDERER_VERSION.into(),
        generation_provenance: provenance,
        overview: plan.overview.clone(),
        steps,
        limitations: vec![format!("intent as the guide read it: {}", draft.intent)],
    };
    (tour, problems)
}
