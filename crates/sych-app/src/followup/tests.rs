use std::cell::RefCell;
use std::collections::VecDeque;

use sych_core::analysis::tour_prompt::{self, DraftStep, TourDraft};
use sych_core::analysis::{
    Anchor, AskOperation, CandidateDraft, CandidateVerdict, ContextAsk, Verdict,
};
use sych_core::{
    CodeRef, DiffManifest, FindingCategory, GitHashAlgorithm, GitObjectId, ReadError, RepoPath,
    Severity, SourceSide, SourceSnapshotId, SourceSpan, Support, sha256, tour,
};

use super::*;

const SOURCE: &str = "fn save(order: &Order) {\n    db.insert(order);\n    publish(order);\n}\n";

/// Every path of the run's head reads as `SOURCE`; another snapshot does not resolve.
struct Reader(SourceSnapshotId);

impl SnapshotReader for Reader {
    fn read_span(&self, r: &CodeRef) -> Result<SourceSpan, ReadError> {
        let text = sych_core::line_range(SOURCE, r.start_line, r.end_line)
            .filter(|_| r.snapshot_id == self.0)
            .ok_or_else(|| ReadError::UnknownSnapshot(r.path.to_string()))?;
        Ok(SourceSpan {
            reference: r.clone(),
            text: text.to_owned(),
        })
    }

    fn code_ref(
        &self,
        side: SourceSide,
        path: &RepoPath,
        start_line: u32,
        end_line: u32,
    ) -> Result<CodeRef, ReadError> {
        Ok(CodeRef {
            snapshot_id: self.0.clone(),
            side,
            path: path.clone(),
            blob_oid: GitObjectId::new(GitHashAlgorithm::Sha1, &"b".repeat(40)).expect("oid"),
            start_line,
            end_line,
            content_sha256: sha256(b"span"),
        })
    }

    fn search(
        &self,
        _: SourceSide,
        _: &str,
        _: &[RepoPath],
        _: usize,
    ) -> Result<Vec<CodeRef>, ReadError> {
        Ok(Vec::new())
    }

    fn diff(&self, s: &SourceSnapshotId) -> Result<DiffManifest, ReadError> {
        Err(ReadError::UnknownSnapshot(s.0.to_string()))
    }
}

/// Answers in order; every prompt it was shown is kept.
#[derive(Default)]
struct Script {
    followups: RefCell<VecDeque<Result<FollowupOutput, String>>>,
    verdicts: RefCell<VecDeque<Result<VerifyOutput, String>>>,
    shown: RefCell<Vec<Prompt>>,
}

impl Model for Script {
    async fn followup(&self, prompt: &Prompt) -> Result<FollowupOutput, String> {
        self.shown.borrow_mut().push(prompt.clone());
        self.followups
            .borrow_mut()
            .pop_front()
            .expect("a scripted answer")
    }

    async fn verify(&self, prompt: &Prompt) -> Result<VerifyOutput, String> {
        self.shown.borrow_mut().push(prompt.clone());
        self.verdicts
            .borrow_mut()
            .pop_front()
            .expect("a scripted verdict")
    }
}

fn result() -> ReviewResult {
    let text = include_str!("../../../../fixtures/review/violated.bundle.json");
    let bundle: serde_json::Value = serde_json::from_str(text).expect("bundle");
    serde_json::from_value(bundle["review_result"].clone()).expect("result")
}

/// The fixture's one stop, pointing at the first three lines of `SOURCE`.
fn tour_of(result: &ReviewResult, reader: &Reader) -> TourArtifact {
    let plan = tour::plan(result, 5);
    let draft = TourDraft {
        intent: "orders publish their event".into(),
        steps: vec![DraftStep {
            stop_id: "stop-1".into(),
            title: "The lost event".into(),
            purpose: "Why the event is not published".into(),
            explanation: "The rule requires the event in the transaction.".into(),
            question: None,
            example_snippets: Vec::new(),
        }],
    };
    let (mut t, problems) = tour_prompt::assemble(&plan, result, draft, "fake".into());
    assert!(problems.is_empty(), "{problems:?}");
    let path: RepoPath = "src/orders.rs".parse().expect("path");
    t.steps[0].code_refs = vec![reader.code_ref(SourceSide::Head, &path, 1, 3).expect("ref")];
    t
}

fn output(answer: &str) -> FollowupOutput {
    FollowupOutput {
        answer: answer.into(),
        anchors: vec![Anchor {
            ref_id: "ref-1".into(),
            start_line: 2,
            end_line: 2,
        }],
        hypotheses: Vec::new(),
        context_requests: Vec::new(),
    }
}

fn read_more() -> ContextAsk {
    ContextAsk {
        question: "who calls save?".into(),
        operation: AskOperation::ReadLines {
            side: SourceSide::Head,
            path: "src/orders.rs".parse().expect("path"),
            start_line: 3,
            end_line: 4,
        },
    }
}

fn ask(script: &Script) -> Result<Reply, String> {
    let r = result();
    let reader = Reader(r.source_snapshot_id.clone());
    let t = tour_of(&r, &reader);
    let ops = Ops {
        facts: &[],
        evidence: &[],
        reader: &reader,
    };
    block_on(answer(
        script,
        (&t, &t.steps[0]),
        &r,
        &ops,
        "what if publish fails?",
    ))
    .expect("runtime")
}

#[test]
fn a_follow_up_sees_its_stop_and_one_round_of_context() {
    let script = Script::default();
    let first = FollowupOutput {
        context_requests: vec![read_more()],
        ..output("")
    };
    let second = FollowupOutput {
        context_requests: vec![read_more()],
        ..output("The insert is not rolled back.")
    };
    script
        .followups
        .borrow_mut()
        .extend([Ok(first), Ok(second)]);
    let Ok(Reply::Explanation {
        text,
        code_refs,
        limitations,
        ..
    }) = ask(&script)
    else {
        panic!("an explanation");
    };
    assert_eq!(text, "The insert is not rolled back.");
    assert_eq!(code_refs.len(), 1, "the anchor resolves");
    assert!(
        limitations.iter().any(|l| l.contains("one round")),
        "{limitations:?}"
    );
    let shown = script.shown.borrow();
    assert_eq!(shown.len(), 2, "no third call for the second request");
    assert_eq!(
        shown[0].data.len(),
        1,
        "only the stop's span: {:?}",
        shown[0].data
    );
    assert_eq!(shown[1].data.len(), 2, "plus the answered request");
    assert!(shown[0].task.contains("what if publish fails?"));
}

#[test]
fn a_spent_session_budget_gives_no_answer() {
    let script = Script::default();
    script
        .followups
        .borrow_mut()
        .push_back(Err("Budget(Money)".into()));
    assert_eq!(ask(&script), Err("Budget(Money)".into()));
}

#[test]
fn a_new_defect_claim_goes_to_the_verifier() {
    let claim = CandidateDraft {
        category: FindingCategory::Defect,
        claim: "a failed publish leaves the order without its event".into(),
        conditions: vec!["publish fails".into()],
        causal_chain: vec!["insert commits first".into()],
        consequence: "consumers never see the order".into(),
        anchors: vec![Anchor {
            ref_id: "ref-1".into(),
            start_line: 2,
            end_line: 3,
        }],
        evidence_ids: Vec::new(),
        obligation_ids: Vec::new(),
        proposed_severity: Severity::Medium,
        disproof_question: "is publish transactional?".into(),
    };
    let verdict = CandidateVerdict {
        candidate_id: "cand-1".into(),
        verdict: Verdict::Rejected,
        category: FindingCategory::Defect,
        severity: Severity::Medium,
        support: Support::SemanticArgument,
        title: "Lost event".into(),
        explanation: "An outbox retries publish.".into(),
        anchors: vec![Anchor {
            ref_id: "ref-1".into(),
            start_line: 3,
            end_line: 3,
        }],
        evidence_ids: Vec::new(),
        alternatives_checked: vec!["outbox".into()],
        suggested_action: String::new(),
        validation_expectation: String::new(),
        limitations: Vec::new(),
        suggested_change: None,
    };
    for verified in [
        Ok(VerifyOutput {
            verdicts: vec![verdict],
            context_requests: Vec::new(),
        }),
        Err("quota".into()),
    ] {
        let script = Script::default();
        script.followups.borrow_mut().push_back(Ok(FollowupOutput {
            hypotheses: vec![claim.clone()],
            ..output("Possibly.")
        }));
        let expected = if verified.is_ok() {
            VerificationStatus::Rejected
        } else {
            VerificationStatus::Unresolved
        };
        script.verdicts.borrow_mut().push_back(verified);
        let Ok(Reply::Explanation { hypotheses, .. }) = ask(&script) else {
            panic!("an explanation");
        };
        assert_eq!(hypotheses.len(), 1);
        assert_eq!(hypotheses[0].verification_status, expected);
        assert_eq!(
            script.shown.borrow()[1].template,
            prompts::VERIFY,
            "the regular verifier"
        );
    }
}
