//! Model analysis without I/O: what each role is shown, the contract it answers in
//! and the checks that turn its answer into candidates, assessments and findings. Model text
//! never sets a final status by itself; the reducer still decides the result.

mod assess;
mod context;
mod discovery;
pub mod followup;
pub mod prompts;
mod search_context;
pub mod tour_prompt;
mod verify;

pub use assess::{
    AssessmentOutput, AssessmentRecord, CriterionRecord, CriterionResult, CriterionStatus,
    ProposedStatus, not_assessed, unknown, unseen_violation, validate as validate_assessment,
};
pub use context::{Anchor, Context, Move, Moves, moves, packet_evidence};
pub use discovery::{
    Answered, AskOperation, CANDIDATE_OVERFLOW, CandidateDraft, ContextAsk, Discovered,
    DiscoveryOutput, MAX_CONTEXT_ASKS, RejectedDraft, ReviewOutput, context_asks,
    drop_fixture_claims, effect_asks, obligation_id, refuting_asks, validate as validate_discovery,
};
pub use verify::{
    CandidateVerdict, KnownDiagnostics, ProposedChange, Verdict, Verified, VerifyOutput,
    merge_overlapping, validate as validate_verdicts,
};

/// Model-written list items as the engine keeps them: trimmed, without empty ones, each once.
/// An empty open question or `needs:` must never reach a summary.
fn texts(items: Vec<String>) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    for item in items {
        let item = item.trim();
        if !item.is_empty() && !kept.iter().any(|k| k == item) {
            kept.push(item.to_owned());
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    #[test]
    fn blank_and_repeated_items_are_dropped() {
        let items = ["", "  ", " q ", "q", "r"].map(String::from).into();
        assert_eq!(super::texts(items), ["q", "r"]);
    }
}
