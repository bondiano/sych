//! Versioned prompt templates: trusted role, output contract and accepted rules
//! first; repository text only in fenced data blocks.

use std::fmt::Write;

use super::context::{Context, side_name};
use super::prompt_rules::{ASSESSMENT_RULES, DEFECT_RULES, NIT_RULES, QUESTION_RULE, VERIFY_RULES};
use crate::model::{DataBlock, Prompt};
use crate::{Candidate, PlannedObligation};

pub use super::prompt_rules::SECURITY_RULE;
pub(super) use super::prompt_rules::{CANDIDATES_CONTRACT, shared_rules};

pub const DISCOVERY: &str = "discovery@11";
pub const RISK_AUDIT: &str = "risk_audit@9";
pub const ASSESSMENT: &str = "assessment@6";
pub const VERIFY: &str = "verify@7";
pub const REVIEW: &str = "review@11";

/// The fields of one assessment; `ob` names the obligation id the answer must carry.
fn assessment_fields(ob: &str) -> String {
    format!(
        "obligation_id ({ob}), applicability (APPLIES, DOES_NOT_APPLY, UNKNOWN), \
         proposed_status (SUPPORTED, VIOLATED, UNKNOWN, NOT_APPLICABLE), criterion_results \
         (one per c-N: criterion_id, status MET, NOT_MET or UNKNOWN, anchors, evidence_ids), \
         evidence_ids, rationale, limitations, missing_inputs, not_applicable_basis (a \
         checkable basis, only with NOT_APPLICABLE, otherwise null) and violating_case (only \
         with VIOLATED, otherwise null)"
    )
}

fn criteria_text(obligation: &PlannedObligation) -> String {
    let mut criteria = String::new();
    for (i, c) in obligation.criteria.iter().enumerate() {
        let _ = writeln!(criteria, "  - c-{}: {c}", i + 1);
    }
    criteria
}

/// Accepted rules as trusted policy text, under `ob-N`.
fn obligations_text(obligations: &[&PlannedObligation]) -> String {
    let mut out = String::new();
    for (i, o) in obligations.iter().enumerate() {
        let ob = &o.obligation;
        let _ = writeln!(
            out,
            "- ob-{}: rule {} v{} ({}, {:?} risk) — {}\n  acceptance: {}",
            i + 1,
            ob.rule_id,
            ob.rule_version,
            o.rule_title,
            ob.risk_level,
            ob.question,
            ob.acceptance_criterion
        );
        for (path, sha256, text) in &o.contracts {
            let shown = text.floor_char_boundary(CONTRACT_BYTES);
            let cut = if shown < text.len() {
                format!(
                    ", first {shown} of {} bytes; the rest is not shown",
                    text.len()
                )
            } else {
                String::new()
            };
            let _ = writeln!(out, "  contract {path} ({sha256}{cut}):");
            for line in text[..shown].lines() {
                let _ = writeln!(out, "    {line}");
            }
        }
    }
    out
}

/// shortcut: a contract is cut at a fixed size; give it its own budgeted span when contracts grow.
const CONTRACT_BYTES: usize = 4000;

pub(super) fn task(intro: &str, context: &Context, rest: &str) -> String {
    format!(
        "{intro}\n\n# Evidence ids\n{}\n{rest}",
        context.evidence_index()
    )
}

/// Hypotheses over one review unit; obligations are listed so candidates can link to them.
pub fn discovery(context: &Context, obligations: &[&PlannedObligation], unit: &str) -> Prompt {
    let mut data = vec![DataBlock {
        label: "review unit".into(),
        content: unit.to_owned(),
    }];
    data.extend(context.blocks.iter().cloned());
    Prompt {
        template: DISCOVERY.into(),
        role: "You are the discovery reviewer of a code review system. You propose checkable \
               hypotheses about defects, rule violations and useful improvements in a change; \
               a separate verifier decides which hold."
            .into(),
        output_contract: format!(
            "Reply with one JSON object {{\"candidates\": [...], \"context_requests\": [...], \
             \"unresolved_questions\": [...]}} matching the schema. {CANDIDATES_CONTRACT}"
        ),
        rules: {
            let mut rules = shared_rules(context);
            rules.push(QUESTION_RULE.to_owned());
            rules.extend(DEFECT_RULES.map(str::to_owned));
            rules.extend(NIT_RULES.map(str::to_owned));
            rules
        },
        task: task(
            "Review the change in the spans below. Old (BASE) and new (HEAD) sides are both \
             shown where they exist.",
            context,
            &format!(
                "# Obligations of this change\n{}",
                obligations_text(obligations)
            ),
        ),
        data,
    }
}

/// An independent search for one high-risk obligation; it does not see discovery's output.
pub fn risk_audit(context: &Context, obligation: &PlannedObligation) -> Prompt {
    let mut prompt = discovery(context, &[obligation], "high-risk obligation audit");
    prompt.template = RISK_AUDIT.into();
    prompt.role = "You are the risk auditor of a code review system. You look only for ways \
                   the change could break the obligation below, including guards or effects \
                   that were removed, moved or weakened; a separate verifier decides which hold."
        .into();
    prompt.task = task(
        "Search the spans for violations of ob-1. Return candidates linked to ob-1, or none.",
        context,
        &format!("# Obligation\n{}", obligations_text(&[obligation])),
    );
    prompt
}

/// One obligation, criterion by criterion.
pub fn assessment(context: &Context, obligation: &PlannedObligation) -> Prompt {
    Prompt {
        template: ASSESSMENT.into(),
        role: "You are the obligation assessor of a code review system. You decide, for one \
               accepted rule, whether the change meets each acceptance criterion, with anchors."
            .into(),
        output_contract: format!(
            "Reply with one JSON object with {}.",
            assessment_fields("ob-1")
        ),
        rules: {
            let mut rules = shared_rules(context);
            rules.extend(ASSESSMENT_RULES.map(str::to_owned));
            rules
        },
        task: task(
            "Assess ob-1 against the spans below.",
            context,
            &format!(
                "# Obligation\n{}\n# Criteria\n{}",
                obligations_text(&[obligation]),
                criteria_text(obligation)
            ),
        ),
        data: context.blocks.clone(),
    }
}

/// Discovery and the assessment of the obligations at `assessed` (indices into
/// `obligations`) in one call, for a small change that shows them whole.
pub fn review(
    context: &Context,
    obligations: &[&PlannedObligation],
    assessed: &[usize],
    unit: &str,
) -> Prompt {
    let mut prompt = discovery(context, obligations, unit);
    prompt.template = REVIEW.into();
    prompt.role = "You are the reviewer of a code review system. You assess accepted rules \
                   criterion by criterion and propose checkable hypotheses about defects, rule \
                   violations and useful improvements; a separate verifier decides which \
                   hypotheses hold."
        .into();
    prompt.output_contract = format!(
        "Reply with one JSON object {{\"assessments\": [...], \"candidates\": [...], \
         \"context_requests\": [...], \"unresolved_questions\": [...]}} matching the schema. \
         One assessment per obligation listed under # Assess, each with {}. {}",
        assessment_fields("ob-N"),
        CANDIDATES_CONTRACT
    );
    prompt.rules.extend(ASSESSMENT_RULES.map(str::to_owned));
    let mut assess = String::new();
    for &i in assessed {
        let _ = writeln!(assess, "- ob-{}\n{}", i + 1, criteria_text(obligations[i]));
    }
    prompt.task = task(
        "Assess the listed obligations and review the change in the spans below. Old (BASE) \
         and new (HEAD) sides are both shown where they exist.",
        context,
        &format!(
            "# Obligations of this change\n{}\n# Assess\n{assess}",
            obligations_text(obligations)
        ),
    );
    prompt
}

/// Verdicts for candidates of one unit; candidate text is model output, so it is data too.
/// `obligations` are the unit's accepted rules, so a violation is judged against their text.
pub fn verify(
    context: &Context,
    candidates: &[Candidate],
    obligations: &[&PlannedObligation],
) -> Prompt {
    let listing: Vec<DataBlock> = candidates
        .iter()
        .enumerate()
        .map(|(i, c)| DataBlock {
            label: format!("cand-{} (unverified hypothesis)", i + 1),
            content: serde_json::json!({
                "category": c.category,
                "claim": c.claim,
                "conditions": c.conditions,
                "causal_chain": c.causal_chain,
                "consequences": c.consequences,
                "proposed_severity": c.proposed_severity,
                "disproof_question": c.refutation,
                "anchors": c.anchors.iter()
                    .map(|a| format!("{} {}:{}-{}", side_name(a.side), a.path, a.start_line, a.end_line))
                    .collect::<Vec<_>>(),
            })
            .to_string(),
        })
        .collect();
    let mut data = listing;
    data.extend(context.blocks.iter().cloned());
    Prompt {
        template: VERIFY.into(),
        role: "You are the verifier of a code review system. For each hypothesis you try to \
               refute it first: look for a wrapper, guard, policy, recovery path or intentional \
               change that preserves the behaviour, then decide."
            .into(),
        output_contract: "Reply with one JSON object {\"verdicts\": [...], \
            \"context_requests\": [...]}, one verdict per cand-N: candidate_id, verdict \
            (CONFIRMED, REJECTED, UNRESOLVED), category, severity, support (STATIC_TRACE, \
            SEMANTIC_ARGUMENT, EXPLICIT_RULE, PREFERENCE), title, explanation, anchors, \
            evidence_ids, alternatives_checked, suggested_action, validation_expectation, \
            limitations, suggested_change (null, or {anchor, replacement}: the full new text of \
            exactly the anchor's lines) and unverified_external_assumptions (array of external \
            library, ORM, framework, dialect or protocol behaviours assumed from memory rather \
            than shown in the spans; [] when none). Be brief: every text one sentence, \
            alternatives_checked short phrases; the hypothesis already states conditions and \
            consequence. context_requests ask for code a verdict needs and the spans do not show, \
            in the discovery form; they are answered once and the UNRESOLVED hypotheses verified \
            again."
            .into(),
        rules: {
            let mut rules = shared_rules(context);
            rules.extend(VERIFY_RULES.map(str::to_owned));
            rules
        },
        task: task(
            "Verify each hypothesis against the spans below.",
            context,
            &if obligations.is_empty() {
                String::new()
            } else {
                format!(
                    "# Obligations of this change\n{}",
                    obligations_text(obligations)
                )
            },
        ),
        data,
    }
}
