//! The coverage audit: over the whole change, what the plan of obligations left out. Each
//! answer that shows the code it applies to becomes an obligation the ordinary assessment
//! decides; none of them is required, so the audit can add checks but never lower coverage.

use std::fmt::Write;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::context::{Anchor, Context};
use super::prompts::{shared_rules, task};
use crate::effects::EffectMatch;
use crate::model::{DataBlock, Prompt};
use crate::{
    Applicability, NOT_ASSESSED, Obligation, ObligationId, ObligationStatus, PlannedObligation,
    ReviewPlan, RiskLevel, RunFacts, SnapshotReader, SourceSnapshotId, derived_uuid,
};

pub const COVERAGE_AUDIT: &str = "coverage_audit@1";
/// Proposals taken from one audit; the rest are dropped by count.
pub const MAX_PROPOSALS: usize = 3;
/// Signals named in the prompt; enough to say why the audit runs.
const MAX_SIGNALS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CoverageOutput {
    pub proposals: Vec<Proposal>,
}

/// One check the planned obligations do not make.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    /// A question about this change, e.g. "does the consumer of `order-paid` still read it?".
    pub question: String,
    /// The changed code that makes the question apply.
    #[schemars(length(min = 1))]
    pub anchors: Vec<Anchor>,
    /// What the code must show for the answer to be yes.
    pub criterion: String,
}

/// Why the change may need checks beyond its plan: changed effects, contract shapes or schema,
/// and rules reached only through a semantic link. Empty means no audit.
pub fn signals(facts: &RunFacts, plan: &ReviewPlan) -> Vec<String> {
    let effects = facts
        .ledger
        .entries
        .iter()
        .filter(|e| e.matched != EffectMatch::Preserved)
        .map(|e| format!("effect `{}` {:?} in {}", e.target, e.matched, e.path));
    let shapes = facts
        .syntax
        .iter()
        .flat_map(|s| &s.facts)
        .filter(|f| f.in_change && f.kind.is_contract())
        .map(|f| format!("changed {:?} in {}", f.kind, f.code_ref.path));
    let schema = (!facts.schema_refs.is_empty()).then(|| {
        format!(
            "{} schema reference(s) to changed tables",
            facts.schema_refs.len()
        )
    });
    let links = plan.obligations.iter().flat_map(|o| {
        o.obligation
            .limitations
            .iter()
            .filter(|l| l.starts_with("applies through a semantic link"))
            .map(move |l| format!("{}: {l}", o.obligation.rule_id))
    });
    let mut out: Vec<String> = Vec::new();
    for signal in effects.chain(shapes).chain(schema).chain(links) {
        if !out.contains(&signal) {
            out.push(signal);
        }
    }
    out.truncate(MAX_SIGNALS);
    out
}

/// The audit over `context`, told the planned obligations with the status their assessment
/// gave, the signals and the titles of what the reviewers already found.
pub fn prompt(
    context: &Context,
    planned: &[(&PlannedObligation, &ObligationStatus)],
    signals: &[String],
    found: &[String],
) -> Prompt {
    let mut obligations = String::new();
    for (o, now) in planned.iter().take(40) {
        let _ = writeln!(
            obligations,
            "- {} ({}): {} — {}",
            o.obligation.rule_id,
            status(now),
            o.rule_title,
            o.obligation.question
        );
    }
    if obligations.is_empty() {
        obligations.push_str("- none\n");
    }

    let list = |items: &[String]| {
        if items.is_empty() {
            "- none\n".to_owned()
        } else {
            items.iter().fold(String::new(), |mut out, s| {
                let _ = writeln!(out, "- {s}");
                out
            })
        }
    };
    let mut data = vec![DataBlock {
        label: "why the change is audited".into(),
        content: list(signals),
    }];
    data.extend(context.blocks.iter().cloned());
    Prompt {
        template: COVERAGE_AUDIT.into(),
        role: "You are the coverage auditor of a code review system. You do not look for \
               defects; you name checks the planned obligations do not make, so the change is \
               judged as a whole: a consumer or caller outside the diff, a path the change \
               touches that no obligation covers, an effect moved without its conditions \
               being compared, a mixed-version deploy state."
            .into(),
        output_contract: format!(
            "Reply with one JSON object {{\"proposals\": [...]}} matching the schema, at most \
             {MAX_PROPOSALS}. Each proposal has question, anchors (at least one, \
             {{ref_id, start_line, end_line}} in a shown span: the changed code that makes it \
             apply) and criterion (what the code must show for the answer to be yes). An empty \
             list is a valid answer."
        ),
        rules: {
            let mut rules = shared_rules(context);
            rules.extend(
                [
                    "Propose only what this change makes necessary, at the code that makes it \
                     so; never a check that any change would need, and never one an obligation \
                     below already makes.",
                    "A proposal is a question for a later assessment, not a finding: do not \
                     claim a defect or decide the answer.",
                ]
                .map(str::to_owned),
            );
            rules
        },
        task: task(
            "Name the checks this change needs that its planned obligations do not make.",
            context,
            &format!(
                "# Planned obligations\n{obligations}\n# Already found\n{}",
                list(found)
            ),
        ),
        data,
    }
}

const fn status(status: &ObligationStatus) -> &'static str {
    match status {
        ObligationStatus::Supported => "SUPPORTED",
        ObligationStatus::Violated => "VIOLATED",
        ObligationStatus::Unknown { .. } => "UNKNOWN",
        ObligationStatus::NotApplicable { .. } => "NOT_APPLICABLE",
    }
}

/// Proposals whose anchors all resolve, as non-required obligations `AUDIT-n` on the units of
/// their anchors; each refused one with its reason.
pub fn accept(
    output: CoverageOutput,
    context: &Context,
    reader: &dyn SnapshotReader,
    plan: &ReviewPlan,
    snapshot: &SourceSnapshotId,
) -> (Vec<PlannedObligation>, Vec<String>) {
    let mut accepted = Vec::new();
    let mut refused = Vec::new();
    for (i, proposal) in output.proposals.into_iter().enumerate() {
        let mut problems = Vec::new();
        let anchors = context.anchors(&proposal.anchors, reader, &mut problems);
        if i >= MAX_PROPOSALS {
            problems.push(format!("more than {MAX_PROPOSALS} proposals"));
        }

        if anchors.is_empty() {
            problems.push("no anchor in the shown spans".into());
        }

        if !problems.is_empty() {
            refused.push(format!(
                "coverage audit proposal refused ({}): {}",
                problems.join("; "),
                proposal.question
            ));
            continue;
        }

        let rule_id = format!("AUDIT-{}", accepted.len() + 1);
        let units = plan
            .scope
            .units
            .iter()
            .filter(|u| anchors.iter().any(|a| u.paths.contains(&a.path)))
            .map(|u| u.id)
            .collect();
        accepted.push(PlannedObligation {
            obligation: Obligation {
                id: ObligationId(derived_uuid(&[
                    "obligation",
                    &snapshot.0.to_string(),
                    &rule_id,
                    &proposal.question,
                ])),
                rule_id,
                rule_version: 1,
                question: proposal.question.clone(),
                required: false,
                risk_level: RiskLevel::Medium,
                review_unit_ids: units,
                code_refs: anchors,
                applicability: Applicability::Applies,
                acceptance_criterion: proposal.criterion.clone(),
                acceptable_evidence_kinds: Vec::new(),
                status: ObligationStatus::Unknown {
                    reason_code: NOT_ASSESSED.into(),
                    next_evidence_needed: "an assessment of the proposed check".into(),
                },
                evidence_ids: Vec::new(),
                limitations: vec![
                    "proposed by the coverage audit, not an accepted rule; never required".into(),
                ],
                waivers: Vec::new(),
                evaluation_mode: None,
            },
            rule_title: "Coverage audit".into(),
            criteria: vec![proposal.criterion],
            severity_on_violation: None,
            exceptions: Vec::new(),
            contracts: Vec::new(),
            about_tests: false,
        });
    }
    (accepted, refused)
}
