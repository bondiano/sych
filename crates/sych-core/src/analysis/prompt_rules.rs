//! Trusted rule text shared across prompt templates.

use super::context::Context;

pub(super) const ASSESSMENT_RULES: [&str; 5] = [
    "SUPPORTED needs every criterion MET with an anchor; VIOLATED needs a NOT_MET criterion \
     with an anchor. Otherwise answer UNKNOWN and list missing_inputs.",
    "VIOLATED also needs violating_case: one sentence naming the concrete input, state or event \
     under which the changed code breaks the criterion, and the error, task or resource that \
     then behaves differently. Restating the criterion is not a case; without one answer UNKNOWN.",
    "A test written in the same change is not the accepted requirement; it can support a \
     criterion only together with the production code it checks.",
    "Not finding a defect is not evidence that a criterion is met.",
    "A file with status Added (0 lines removed) did not exist at BASE: never answer UNKNOWN \
     asking for its BASE revision or pre-change behaviour. Assess its HEAD code directly, or \
     answer NOT_APPLICABLE with a checkable basis when the rule only compares against pre-change \
     behaviour.",
];

pub(super) const CANDIDATES_CONTRACT: &str = "Each candidate has category (DEFECT, RULE_VIOLATION, \
    IMPROVEMENT, NIT), claim, conditions, causal_chain, consequence, anchors, evidence_ids, \
    obligation_ids (ob-N or empty), proposed_severity and disproof_question. A context request \
    has a concrete question and one typed operation (read_lines, read_diff of another changed file, \
    find_references, find_callers, find_callees, find_implementations or search_source). Be brief: at most 4 candidates, the \
    strongest first, and 4 context requests; each text field one sentence.";

/// How a seasoned reviewer picks small remarks.
pub(super) const NIT_RULES: [&str; 4] = [
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

/// How a defect is hunted and stated: most missed bugs were on shown lines, with the wrong
/// mechanism claimed, no candidate at all, or a nit in its place.
pub(super) const DEFECT_RULES: [&str; 4] = [
    "Before any nit, check each changed function for defects: the first, last, empty and \
     boundary inputs; what each early return, error or exception path skips; state kept across \
     calls or iterations; each non-default flag or configuration branch.",
    "A DEFECT names one concrete input or state that triggers it and traces it through the HEAD \
     code to the wrong result. \"The change lost X\" holds only if HEAD does not do X another \
     way; a premise (a default, a caller, a guard) is checked against the spans first, and a \
     claim the spans contradict is dropped.",
    "Code that does something other than its comment, message, name or documentation says is a \
     DEFECT, not a nit, unless the spans show the code is the intended behaviour.",
    "The limit of 4 candidates counts DEFECT and RULE_VIOLATION only; nits come on top, as the \
     nit rules allow.",
];

/// The `security` profile's hunt in discovery: one rule for every language, since the syntax
/// facts mark SQL and network calls only in Go and Python.
pub const SECURITY_RULE: &str = "Security: follow data the changed code takes from a request, a \
    file, the environment or another process to where it is used. Data that reaches a SQL \
    statement, a shell or process command, a file path, an outgoing URL, HTML or a deserializer \
    without a bound parameter, an allowlist or validation is a DEFECT; so is a new or changed \
    request handler that acts on a resource without checking that the caller may, and a secret \
    written to a log, an error or a response. Name the input and the place it reaches.";

/// An open question the shown code already answers is a miss, not an UNKNOWN (#95).
pub(super) const QUESTION_RULE: &str = "Before leaving an open question about what the code is \
    meant to do, look for the answer in the spans: removed BASE lines, doc comments, messages \
    and names state the intent. If they answer it and the change breaks that intent, propose a \
    candidate instead; if they answer it otherwise, drop the question. Keep only questions the \
    spans cannot answer.";

/// A BASE search never sees what the change adds (#160: "unused" for a dependency used on HEAD).
const ABSENCE_RULE: &str =
    "\"Unused\" or \"missing\" needs a complete HEAD search: BASE predates this change.";

const EXTERNAL_GROUNDING_RULE: &str = "Never assert unshown external library, ORM, framework, \
    SQL dialect or protocol behaviour from memory; leave an open question or answer UNRESOLVED.";

const ANCHORS: &str = "Anchors name a handed-out span and lines inside it: \
    {\"ref_id\": \"ref-N\", \"start_line\": a, \"end_line\": b}, using the line numbers printed in \
    that span. Every candidate, criterion result and verdict that claims something needs at \
    least one anchor; one without anchors is discarded. Evidence ids are the `ev-N` listed in \
    the task. Use no other ids, paths or hashes.";

pub(super) const VERIFY_RULES: [&str; 8] = [
    "CONFIRMED only when the shown spans establish the claim; UNRESOLVED when context is missing; \
     REJECTED when a mechanism preserves the behaviour.",
    "HIGH or CRITICAL needs a STATIC_TRACE: one anchor per step from the change to the \
     consequence. Agreement with the hypothesis is not evidence.",
    "Severity and support are separate: a likely but untraced problem is UNRESOLVED, not a lower \
     severity.",
    "An unfounded nit, or one that proposes no change, is REJECTED, never raised to a defect.",
    "If confirming a hypothesis depends on external library, ORM, framework, SQL dialect or \
     protocol behaviour not shown in the repository spans (such as tag semantics, default naming, \
     reflection rules, FK ownership or third-party API contracts), list each unverified assumption \
     in unverified_external_assumptions and answer UNRESOLVED, never CONFIRMED.",
    "A claim that a changed default, constant or literal changes behaviour is CONFIRMED only when \
     a shown span reads that value on the path to the consequence. With no reader shown, ask \
     search_source on BASE for its name and answer UNRESOLVED; when a complete answer shows no \
     reader outside its definition and default and the value is not public API, it is REJECTED: \
     an unread value changes nothing.",
    "A claim that nothing replaces lines the change removes (a BASE anchor): when the block of \
     names those lines lose shows the change also removes or moves what they use (the field, type \
     or function they read), the removal belongs to that change and the claim is REJECTED unless \
     a shown HEAD span still relies on the old behaviour. A HEAD search with no match for a \
     removed name shows the name is gone, not that nothing replaces it.",
    "suggested_change only for a CONFIRMED finding whose fix is local to lines of a shown HEAD \
     span: the anchor names those lines and replacement is their complete new text with the \
     original indentation. Otherwise null; never write code the spans do not show or a name the \
     change removes.",
];

pub(super) fn shared_rules(context: &Context) -> Vec<String> {
    [
        "Claims must follow from the shown source; say UNKNOWN or leave a question when they do not.",
        "Do not claim that any test, build or command was run; nothing was executed.",
        "An empty answer is valid. Do not invent problems to have something to report.",
        "Keep explanations short and causal. Do not include hidden reasoning.",
        EXTERNAL_GROUNDING_RULE,
        ABSENCE_RULE,
        ANCHORS,
        context.language.instruction(),
    ]
    .map(str::to_owned)
    .to_vec()
}
