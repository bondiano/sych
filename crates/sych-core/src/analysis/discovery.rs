//! Discovery and risk-audit output (spec §16.2): hypotheses with anchors into handed-out
//! spans, typed context asks and open questions. Nothing here decides a final status.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::assess::AssessmentOutput;
use super::context::{Anchor, Context};
use crate::broker::{OpStatus, Operation, Ops, SymbolRef};
use crate::model::DataBlock;
use crate::{
    Candidate, CandidateId, CandidateOrigin, FindingCategory, ObligationId, RepoPath, Severity,
    SnapshotReader, SourceSide, VerificationStatus, derived_uuid,
};

/// Context asks answered per discovery call; the rest are rejected by count.
pub const MAX_CONTEXT_ASKS: usize = 4;
const MAX_ASK_LINES: u32 = 160;
const MAX_CANDIDATES: usize = 20;
/// Problem code of a draft past `MAX_CANDIDATES`; the run's coverage is partial then.
pub const CANDIDATE_OVERFLOW: &str = "CANDIDATE_OVERFLOW";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryOutput {
    pub candidates: Vec<CandidateDraft>,
    pub context_requests: Vec<ContextAsk>,
    pub unresolved_questions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CandidateDraft {
    pub category: FindingCategory,
    /// One short checkable statement.
    pub claim: String,
    pub conditions: Vec<String>,
    /// Steps from the change to the consequence, each tied to a shown span.
    pub causal_chain: Vec<String>,
    pub consequence: String,
    /// At least one: structured-output upstreams otherwise return the claim without any.
    #[schemars(length(min = 1))]
    pub anchors: Vec<Anchor>,
    pub evidence_ids: Vec<String>,
    pub obligation_ids: Vec<String>,
    pub proposed_severity: Severity,
    /// What mechanism could have preserved the behaviour.
    pub disproof_question: String,
}

/// Discovery and the assessments of the listed obligations in one call (`review@1`), for a
/// change whose obligations are fully shown in the discovery spans.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewOutput {
    pub assessments: Vec<AssessmentOutput>,
    pub candidates: Vec<CandidateDraft>,
    pub context_requests: Vec<ContextAsk>,
    pub unresolved_questions: Vec<String>,
}

impl ReviewOutput {
    pub fn split(self) -> (Vec<AssessmentOutput>, DiscoveryOutput) {
        (
            self.assessments,
            DiscoveryOutput {
                candidates: self.candidates,
                context_requests: self.context_requests,
                unresolved_questions: self.unresolved_questions,
            },
        )
    }
}

/// A question for more context and the typed operation that would answer it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextAsk {
    pub question: String,
    pub operation: AskOperation,
}

/// The model-facing form of §9.4 operations: no hashes, ids or free-form commands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum AskOperation {
    ReadLines {
        side: SourceSide,
        path: RepoPath,
        start_line: u32,
        end_line: u32,
    },
    FindReferences {
        side: SourceSide,
        path: RepoPath,
        name: String,
    },
    FindCallers {
        side: SourceSide,
        path: RepoPath,
        name: String,
    },
    FindCallees {
        side: SourceSide,
        path: RepoPath,
        name: String,
    },
    FindImplementations {
        side: SourceSide,
        path: RepoPath,
        name: String,
    },
    /// Fixed string, one line.
    SearchSource { side: SourceSide, query: String },
}

const ASK_LIMIT: usize = 10;

impl AskOperation {
    /// The broker operation; `read_lines` becomes an exact span read through the reader.
    pub fn to_operation(&self, reader: &dyn SnapshotReader) -> Result<Operation, String> {
        let symbol = |side: &SourceSide, path: &RepoPath, name: &str| SymbolRef {
            side: *side,
            path: path.clone(),
            name: name.to_owned(),
        };
        Ok(match self {
            Self::ReadLines {
                side,
                path,
                start_line,
                end_line,
            } => {
                if end_line < start_line || end_line - start_line >= MAX_ASK_LINES {
                    return Err(format!("read_lines takes 1..={MAX_ASK_LINES} lines"));
                }

                let code_ref = reader
                    .code_ref(*side, path, *start_line, *end_line)
                    .map_err(|e| e.to_string())?;
                Operation::ReadSpan { code_ref }
            }
            Self::FindReferences { side, path, name } => Operation::FindReferences {
                symbol: symbol(side, path, name),
                limit: ASK_LIMIT,
            },
            Self::FindCallers { side, path, name } => Operation::FindCallers {
                symbol: symbol(side, path, name),
                limit: ASK_LIMIT,
            },
            Self::FindCallees { side, path, name } => Operation::FindCallees {
                symbol: symbol(side, path, name),
                limit: ASK_LIMIT,
            },
            Self::FindImplementations { side, path, name } => Operation::FindImplementations {
                symbol: symbol(side, path, name),
                limit: ASK_LIMIT,
            },
            Self::SearchSource { side, query } => Operation::SearchSource {
                query: query.clone(),
                paths: Vec::new(),
                side: *side,
                limit: ASK_LIMIT,
            },
        })
    }
}

/// What answering one ask added to a context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answered {
    /// `None` when the ask did not become an operation.
    pub status: Option<OpStatus>,
    pub refs: usize,
    pub limitations: Vec<String>,
}

impl ContextAsk {
    /// Answers through the broker's typed operations: at most `max_refs` spans join `context`,
    /// or a block says what the query showed.
    pub fn answer(&self, ops: &Ops<'_>, context: &mut Context, max_refs: usize) -> Answered {
        let operation = match self.operation.to_operation(ops.reader) {
            Ok(operation) => operation,
            Err(problem) => {
                return Answered {
                    status: None,
                    refs: 0,
                    limitations: vec![problem],
                };
            }
        };
        let result = ops.run(&operation);
        let reason = format!("answer to: {}", self.question.replace('\n', " "));
        let refs = result.refs.len().min(max_refs);
        let (mut status, mut limitations) = (result.status, result.limitations);
        if refs < result.refs.len() {
            if status == OpStatus::CompleteForQuery {
                status = OpStatus::Partial;
            }

            limitations.push(format!("{refs} of {} matches shown", result.refs.len()));
        }

        for r in &result.refs[..refs] {
            context.add_span(r, &reason, ops.reader);
        }
        // An empty complete answer is evidence too: "nothing reads it" can refute a claim; an
        // incomplete one shows nothing either way.
        let mut body = match (refs, status) {
            (0, OpStatus::CompleteForQuery) => "no matches".to_owned(),
            (0, status) => format!("no answer ({status:?}): absence is not shown"),
            (n, OpStatus::CompleteForQuery) => format!("{n} matches shown: the whole answer"),
            (n, status) => {
                format!("{n} matches shown ({status:?}): absence of others is not shown")
            }
        };
        for limitation in &limitations {
            body.push('\n');
            body.push_str(limitation);
        }
        context.blocks.push(DataBlock {
            label: reason,
            content: body,
        });

        Answered {
            status: Some(status),
            refs,
            limitations,
        }
    }
}

/// A draft the engine refused, with every reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedDraft {
    pub claim: String,
    pub problems: Vec<String>,
}

/// Validated discovery: candidates, refused drafts and the asks still to answer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discovered {
    pub candidates: Vec<Candidate>,
    pub rejected: Vec<RejectedDraft>,
    pub asks: Vec<ContextAsk>,
    pub rejected_asks: Vec<String>,
    pub unresolved_questions: Vec<String>,
}

/// Obligations a prompt may link to, under local ids `ob-N`.
pub fn obligation_id(obligations: &[ObligationId], local: &str) -> Option<ObligationId> {
    let n: usize = local.strip_prefix("ob-")?.parse().ok()?;
    obligations.get(n.checked_sub(1)?).copied()
}

/// Keeps drafts whose anchors, evidence and obligation ids were all handed out; a draft with
/// any invented reference is refused whole rather than repaired.
pub fn validate(
    output: DiscoveryOutput,
    origin_key: &str,
    context: &Context,
    obligations: &[ObligationId],
    reader: &dyn SnapshotReader,
) -> Discovered {
    let mut found = Discovered {
        unresolved_questions: output.unresolved_questions,
        ..Discovered::default()
    };
    for (i, draft) in output.candidates.into_iter().enumerate() {
        let mut problems = Vec::new();
        if i >= MAX_CANDIDATES {
            problems.push(format!(
                "{CANDIDATE_OVERFLOW}: more than {MAX_CANDIDATES} candidates"
            ));
        }

        if draft.claim.trim().is_empty() {
            problems.push("empty claim".into());
        }

        let anchors = context.anchors(&draft.anchors, reader, &mut problems);
        if anchors.is_empty() {
            problems.push("no anchor in the shown spans".into());
        }

        let evidence_ids = context.evidence_ids(&draft.evidence_ids, &mut problems);
        let mut obligation_ids = Vec::new();
        for local in &draft.obligation_ids {
            match obligation_id(obligations, local) {
                Some(id) => obligation_ids.push(id),
                None => problems.push(format!("unknown obligation id `{local}`")),
            }
        }
        if !problems.is_empty() {
            found.rejected.push(RejectedDraft {
                claim: draft.claim,
                problems,
            });
            continue;
        }

        found.candidates.push(Candidate {
            id: CandidateId(derived_uuid(&[
                "candidate",
                origin_key,
                &i.to_string(),
                &draft.claim,
            ])),
            origin: CandidateOrigin::Model,
            category: draft.category,
            proposed_severity: draft.proposed_severity,
            claim: draft.claim,
            causal_chain: draft.causal_chain,
            anchors,
            consequences: vec![draft.consequence],
            conditions: draft.conditions,
            refutation: draft.disproof_question,
            evidence_ids,
            obligation_ids,
            verification_status: VerificationStatus::Pending,
            limitations: Vec::new(),
        });
    }
    (found.asks, found.rejected_asks) = context_asks(output.context_requests);
    found
}

/// The asks of one answer that are answered, at most `MAX_CONTEXT_ASKS`, and why the rest
/// are not.
pub fn context_asks(requests: Vec<ContextAsk>) -> (Vec<ContextAsk>, Vec<String>) {
    let mut asks = Vec::new();
    let mut rejected = Vec::new();
    for (i, ask) in requests.into_iter().enumerate() {
        if i >= MAX_CONTEXT_ASKS {
            rejected.push(format!(
                "`{}`: more than {MAX_CONTEXT_ASKS} context requests",
                ask.question
            ));
        } else if ask.question.trim().is_empty() {
            rejected.push("context request without a question".into());
        } else {
            asks.push(ask);
        }
    }
    (asks, rejected)
}
