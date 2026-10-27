//! Versioned prompt templates (spec §16.5): trusted role, output contract and accepted rules
//! first; repository text only in fenced data blocks.

use std::fmt::Write;

use super::context::Context;
use crate::model::{DataBlock, Prompt};
use crate::{Candidate, PlannedObligation};

pub const DISCOVERY: &str = "discovery@6";
pub const RISK_AUDIT: &str = "risk_audit@5";
pub const ASSESSMENT: &str = "assessment@4";
pub const VERIFY: &str = "verify@4";
pub const REVIEW: &str = "review@5";

/// The fields of one assessment; `ob` names the obligation id the answer must carry.
fn assessment_fields(ob: &str) -> String {
    format!(
        "obligation_id ({ob}), applicability (APPLIES, DOES_NOT_APPLY, UNKNOWN), \
         proposed_status (SUPPORTED, VIOLATED, UNKNOWN, NOT_APPLICABLE), criterion_results \
         (one per c-N: criterion_id, status MET, NOT_MET or UNKNOWN, anchors, evidence_ids), \
         evidence_ids, rationale, limitations, missing_inputs and not_applicable_basis (a \
         checkable basis, only with NOT_APPLICABLE, otherwise null)"
    )
}

const ASSESSMENT_RULES: [&str; 3] = [
    "SUPPORTED needs every criterion MET with an anchor; VIOLATED needs a NOT_MET criterion \
     with an anchor. Otherwise answer UNKNOWN and list missing_inputs.",
    "A test written in the same change is not the accepted requirement; it can support a \
     criterion only together with the production code it checks.",
    "Not finding a defect is not evidence that a criterion is met.",
];

fn criteria_text(obligation: &PlannedObligation) -> String {
    let mut criteria = String::new();
    for (i, c) in obligation.criteria.iter().enumerate() {
        let _ = writeln!(criteria, "  - c-{}: {c}", i + 1);
    }
    criteria
}

const CANDIDATES_CONTRACT: &str = "Each candidate has category (DEFECT, RULE_VIOLATION, \
    IMPROVEMENT, NIT), claim, conditions, causal_chain, consequence, anchors, evidence_ids, \
    obligation_ids (ob-N or empty), proposed_severity and disproof_question. A context request \
    has a concrete question and one typed operation (read_lines, find_references, find_callers, \
    find_callees, find_implementations or search_source). Be brief: at most 4 candidates, the \
    strongest first, and 4 context requests; each text field one sentence.";

/// How a seasoned reviewer picks small remarks (docs/research/review-comments.md).
const NIT_RULES: [&str; 4] = [
    "A NIT or IMPROVEMENT is at most LOW, does not change behaviour and names the place, the \
     future mistake or confusion it prevents, and the concrete edit; without all three it is not \
     a candidate. Zero nits is a fine answer; praise is never a candidate.",
    "The strongest nits keep the change consistent with what the repository already has (an \
     existing helper, pattern, neighbouring test or convention): anchor the existing example. \
     Stale or misleading comments, a magic number without its reason, a shadowed name, a \
     mutated argument and a test that cannot fail are worth a nit too.",
    "Never remark on what rustfmt, clippy, gofmt, go vet, eslint, prettier or tsc already report \
     (formatting, import order, unused code). Prefer removing or inlining code over a new \
     abstraction; a performance nit needs a hot path or input size. Code the change does not \
     touch gets no nits.",
    "At most 3 nits per review unit, the most useful first. When unsure whether something is a \
     defect, ask about one concrete scenario in the claim instead of asserting it.",
];

const ANCHORS: &str = "Anchors name a handed-out span and lines inside it: \
    {\"ref_id\": \"ref-N\", \"start_line\": a, \"end_line\": b}, using the line numbers printed in \
    that span. Every candidate, criterion result and verdict that claims something needs at \
    least one anchor; one without anchors is discarded. Evidence ids are the `ev-N` listed in \
    the task. Use no other ids, paths or hashes.";

fn shared_rules(context: &Context) -> Vec<String> {
    [
        "Claims must follow from the shown source; say UNKNOWN or leave a question when they do not.",
        "Do not claim that any test, build or command was run; nothing was executed.",
        "An empty answer is valid. Do not invent problems to have something to report.",
        "Keep explanations short and causal. Do not include hidden reasoning.",
        ANCHORS,
        context.language.instruction(),
    ]
    .map(str::to_owned)
    .to_vec()
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

fn task(intro: &str, context: &Context, rest: &str) -> String {
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
                    .map(|a| format!("{}:{}-{}", a.path, a.start_line, a.end_line))
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
            \"context_requests\": [...]}, one verdict per cand-N: candidate_id, verdict (CONFIRMED, REJECTED, UNRESOLVED), category, severity, support \
            (STATIC_TRACE, SEMANTIC_ARGUMENT, EXPLICIT_RULE, PREFERENCE), title, explanation, \
            anchors, evidence_ids, alternatives_checked, suggested_action, validation_expectation \
            and limitations. Be brief: every text one sentence, alternatives_checked short \
            phrases; the hypothesis already states conditions and consequence. \
            context_requests ask for code a verdict needs and the spans do not show, \
            in the discovery form; they are answered once and the UNRESOLVED hypotheses \
            verified again."
            .into(),
        rules: {
            let mut rules = shared_rules(context);
            rules.extend(
                [
                    "CONFIRMED only when the shown spans establish the claim; UNRESOLVED when \
                     context is missing; REJECTED when a mechanism preserves the behaviour.",
                    "HIGH or CRITICAL needs a STATIC_TRACE: one anchor per step from the change \
                     to the consequence. Agreement with the hypothesis is not evidence.",
                    "Severity and support are separate: a likely but untraced problem is \
                     UNRESOLVED, not a lower severity.",
                    "An unfounded nit, or one that proposes no change, is REJECTED, never raised \
                     to a defect.",
                    "A claim that a changed default, constant or literal changes behaviour is \
                     CONFIRMED only when a shown span reads that value on the path to the \
                     consequence. With no reader shown, ask search_source on BASE for its name \
                     and answer UNRESOLVED; when a complete answer shows no reader outside \
                     its definition and default and the value is not public API, it is \
                     REJECTED: an unread value changes nothing.",
                ]
                .map(str::to_owned),
            );
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
