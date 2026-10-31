//! Model analysis without I/O: what each role is shown, the contract it answers in
//! and the checks that turn its answer into candidates, assessments and findings. Model text
//! never sets a final status by itself; the reducer still decides the result.

mod assess;
mod context;
mod discovery;
pub mod followup;
pub mod prompts;
pub mod tour_prompt;
mod verify;

pub use assess::{
    AssessmentOutput, AssessmentRecord, CriterionRecord, CriterionResult, CriterionStatus,
    ProposedStatus, not_assessed, unknown, unseen_violation, validate as validate_assessment,
};
pub use context::{Anchor, Context, packet_evidence};
pub use discovery::{
    Answered, AskOperation, CANDIDATE_OVERFLOW, CandidateDraft, ContextAsk, Discovered,
    DiscoveryOutput, MAX_CONTEXT_ASKS, RejectedDraft, ReviewOutput, context_asks, obligation_id,
    refuting_asks, validate as validate_discovery,
};
pub use verify::{
    CandidateVerdict, KnownDiagnostics, ProposedChange, Verdict, Verified, VerifyOutput,
    merge_overlapping, validate as validate_verdicts,
};
