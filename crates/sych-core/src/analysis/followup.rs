//! Bounded follow-up on one tour stop (spec §21.6, roadmap M5.3). The model sees the stop, a
//! brief overview of the review and what typed context queries return; never the whole
//! repository or another reader's questions. Its answer is prose, never evidence, and a defect
//! claim in it is a hypothesis for the regular verifier, never a finding of the review.

use std::fmt::Write;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::context::{Anchor, Context};
use super::discovery::{self, CandidateDraft, ContextAsk, DiscoveryOutput};
use super::prompts::{CANDIDATES_CONTRACT, shared_rules, task};
use crate::model::Prompt;
use crate::tour::{QuestionKind, TourArtifact, TourStep};
use crate::{Candidate, CodeRef, ReviewResult, SchemaVersion, SnapshotReader, Timestamp, TourId};

pub const FOLLOWUP: &str = "followup@1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FollowupOutput {
    pub answer: String,
    pub anchors: Vec<Anchor>,
    /// Defect or rule-violation claims the review did not make; the verifier decides them.
    pub hypotheses: Vec<CandidateDraft>,
    pub context_requests: Vec<ContextAsk>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Reply {
    /// `COMPREHENSION`: generated prose; it can be wrong and is never technical evidence.
    Explanation {
        text: String,
        code_refs: Vec<CodeRef>,
        /// After the regular verifier; none of them is a finding of the review or published.
        hypotheses: Vec<Candidate>,
        limitations: Vec<String>,
        attempts: usize,
        cost_micro_usd: u64,
    },
    /// `FACT_REQUEST`: the actor's own statement; it never rewrites an accepted requirement.
    HumanStatement { text: String },
    /// `RISK_DECISION`: a separate scoped waiver; the review's `UNKNOWN` and `VIOLATED` stay.
    RiskDecision { waiver_id: i64, rule_id: String },
}

/// One question and its reply in one actor's private session; the tour and the result stay
/// as they are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Exchange {
    pub schema_version: SchemaVersion,
    pub id: String,
    pub tour_id: TourId,
    pub step_id: String,
    pub actor: String,
    pub kind: QuestionKind,
    pub question: String,
    pub at: Timestamp,
    pub reply: Reply,
}

/// The ledger scope of one session, so its follow-up cap is its own and no other reader's.
pub fn scope(tour: TourId, actor: &str) -> String {
    format!("followup:{tour}:{actor}")
}

const RULES: [&str; 4] = [
    "Answer the reader's question about this stop only from the shown spans and the review's \
     statements below; say what is not shown instead of guessing.",
    "Statuses, severity and coverage are the review's: never restate them as changed. Your \
     answer is an explanation, not evidence.",
    "A defect or rule violation the review did not report goes into hypotheses, never into the \
     answer as a fact; a separate verifier decides it. Zero hypotheses is the usual answer.",
    "Ask for context only when the spans cannot answer; it is answered once.",
];

/// The stop, the review's statements about it as `ob-N`, and the overview; `context` holds the
/// stop's spans and any answered context requests.
pub fn prompt(
    tour: &TourArtifact,
    step: &TourStep,
    result: &ReviewResult,
    context: &Context,
    question: &str,
) -> Prompt {
    let mut task_text = format!(
        "# Overview\nCoverage {:?}; {} critical findings; {} required obligations UNKNOWN.\n\n\
         # Stop {}: {}\n{}\n{}\n",
        tour.overview.coverage,
        tour.overview.critical_findings.len(),
        tour.overview.important_unknowns.len(),
        step.id,
        step.title,
        step.purpose,
        step.explanation
    );
    for f in result
        .findings
        .iter()
        .filter(|f| step.finding_ids.contains(&f.id))
    {
        let _ = writeln!(
            task_text,
            "- finding ({:?}, {:?}): {} — {}",
            f.severity, f.category, f.title, f.consequence
        );
    }
    for (i, id) in step.obligation_ids.iter().enumerate() {
        if let Some(o) = result.obligations.iter().find(|o| o.id == *id) {
            let _ = writeln!(
                task_text,
                "- ob-{}: rule {} — {} — {:?}",
                i + 1,
                o.rule_id,
                o.question,
                o.status
            );
        }
    }
    let _ = write!(task_text, "\n# The reader's question\n{question}");
    Prompt {
        template: FOLLOWUP.into(),
        role: "You answer a reader's question about one stop of a code review tour. The review \
               is done; you explain it and the shown code."
            .into(),
        output_contract: format!(
            "Reply with one JSON object {{\"answer\": \"...\", \"anchors\": [...], \
             \"hypotheses\": [...], \"context_requests\": [...]}}. The answer is at most four \
             sentences; anchors point at the lines it relies on. Each hypothesis is a \
             candidate: {CANDIDATES_CONTRACT}"
        ),
        rules: {
            let mut rules = shared_rules(context);
            rules.extend(RULES.map(str::to_owned));
            rules
        },
        task: task("Answer the question below.", context, &task_text),
        data: context.blocks.clone(),
    }
}

/// Hypotheses that are checkable candidates (anchored in shown spans, linked only to the stop's
/// obligations), and why the others were dropped.
pub fn hypotheses(
    output: &FollowupOutput,
    key: &str,
    step: &TourStep,
    context: &Context,
    reader: &dyn SnapshotReader,
) -> (Vec<Candidate>, Vec<String>) {
    let found = discovery::validate(
        DiscoveryOutput {
            candidates: output.hypotheses.clone(),
            context_requests: Vec::new(),
            unresolved_questions: Vec::new(),
        },
        key,
        context,
        &step.obligation_ids,
        reader,
    );
    let dropped = found
        .rejected
        .into_iter()
        .map(|r| {
            format!(
                "hypothesis `{}` dropped: {}",
                r.claim,
                r.problems.join("; ")
            )
        })
        .collect();
    (found.candidates, dropped)
}
