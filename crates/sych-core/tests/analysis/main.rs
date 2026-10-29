//! Model output checks (roadmap M1.5–M1.7): what the engine accepts from discovery,
//! assessment and verification answers ([`verify`]), and what it refuses or downgrades.

use std::collections::BTreeMap;

use serde_json::json;
use sych_core::analysis::{
    Anchor, AssessmentOutput, Context, CriterionResult, CriterionStatus, DiscoveryOutput,
    ProposedStatus, unseen_violation, validate_assessment, validate_discovery,
};
use sych_core::broker::EvidencePacket;
mod anchors;
mod answers;
mod removals;
mod verify;

use sych_core::{
    Applicability, CodeRef, DiffManifest, Evidence, GitHashAlgorithm, GitObjectId, Obligation,
    ObligationId, ObligationStatus, PlannedObligation, ReadError, RepoPath, RiskLevel, Severity,
    SnapshotReader, SourceSide, SourceSnapshotId, SourceSpan, VerificationStatus, line_range,
    sha256,
};

const ORDER_BASE: &str = "export function total(items) {\n  if (!items) {\n    return 0;\n  }\n  return items.reduce(sum, 0);\n}\n";
const ORDER_HEAD: &str = "export function total(items) {\n  return items!.reduce(sum, 0);\n}\n";
const TEST: &str = "it('totals', () => {\n  expect(total([])).toBe(0);\n});\n";

fn snapshot() -> SourceSnapshotId {
    SourceSnapshotId(sha256(b"analysis fixture"))
}

fn path(p: &str) -> RepoPath {
    p.parse().expect("path")
}

struct Reader(BTreeMap<(SourceSide, RepoPath), &'static str>);

impl Reader {
    fn new() -> Self {
        Self(BTreeMap::from([
            ((SourceSide::DiffBase, path("src/order.ts")), ORDER_BASE),
            ((SourceSide::Head, path("src/order.ts")), ORDER_HEAD),
            ((SourceSide::Head, path("src/order.test.ts")), TEST),
        ]))
    }

    fn at(&self, side: SourceSide, p: &str, start: u32, end: u32) -> CodeRef {
        self.code_ref(side, &path(p), start, end).expect("ref")
    }
}

impl SnapshotReader for Reader {
    fn read_span(&self, r: &CodeRef) -> Result<SourceSpan, ReadError> {
        let fresh = self.code_ref(r.side, &r.path, r.start_line, r.end_line)?;
        if &fresh != r {
            return Err(ReadError::HashMismatch(r.path.to_string()));
        }

        let text =
            line_range(self.0[&(r.side, r.path.clone())], r.start_line, r.end_line).expect("range");
        Ok(SourceSpan {
            reference: fresh,
            text: text.to_owned(),
        })
    }

    fn code_ref(
        &self,
        side: SourceSide,
        p: &RepoPath,
        start: u32,
        end: u32,
    ) -> Result<CodeRef, ReadError> {
        let text = self
            .0
            .get(&(side, p.clone()))
            .ok_or_else(|| ReadError::Unavailable(p.to_string()))?;
        let span = line_range(text, start, end).ok_or(ReadError::LineRange {
            start,
            end,
            line_count: 0,
        })?;
        Ok(CodeRef {
            snapshot_id: snapshot(),
            side,
            path: p.clone(),
            blob_oid: GitObjectId::new(
                GitHashAlgorithm::Sha1,
                &sha256(text.as_bytes()).hex()[..40],
            )
            .expect("oid"),
            start_line: start,
            end_line: end,
            content_sha256: sha256(span.as_bytes()),
        })
    }

    fn search(
        &self,
        side: SourceSide,
        literal: &str,
        _: &[RepoPath],
        limit: usize,
    ) -> Result<Vec<CodeRef>, ReadError> {
        let mut hits = Vec::new();
        for ((s, p), text) in &self.0 {
            for (i, line) in (1..).zip(text.lines()) {
                if *s == side && line.contains(literal) {
                    hits.push(self.code_ref(side, p, i, i)?);
                }
            }
        }
        hits.truncate(limit);
        Ok(hits)
    }

    fn diff(&self, _: &SourceSnapshotId) -> Result<DiffManifest, ReadError> {
        unreachable!("not used")
    }
}

fn source_evidence() -> Evidence {
    let mut ev: Evidence =
        serde_json::from_str(include_str!("../../../../fixtures/core/v1/evidence.json"))
            .expect("evidence fixture");
    ev.source_snapshot_id = snapshot();
    ev
}

/// A packet showing base 1-6, head 1-3 and the head test (`ref-1`, `ref-2`, `ref-3`).
fn context(reader: &Reader) -> (Context, Evidence) {
    context_of(
        reader,
        &[
            reader.at(SourceSide::DiffBase, "src/order.ts", 1, 6),
            reader.at(SourceSide::Head, "src/order.ts", 1, 3),
            reader.at(SourceSide::Head, "src/order.test.ts", 1, 3),
        ],
    )
}

/// A packet showing `spans` as `ref-1`, `ref-2`, … in order.
fn context_of(reader: &Reader, spans: &[CodeRef]) -> (Context, Evidence) {
    let fragments: Vec<_> = spans
        .iter()
    .enumerate()
    .map(|(i, r)| {
        json!({"id": format!("f{i}"), "code_ref": r, "priority": "REQUIRED",
               "reason_selected": ["diff hunk"], "evidence_ids": [], "completeness": "COMPLETE_FOR_QUERY",
               "tokens": 10, "transformation": "none"})
    })
    .collect();
    let packet: EvidencePacket = serde_json::from_value(json!({
        "schema_version": 1,
        "request": {"run_id": "01900000-0000-7000-8000-000000000001", "snapshot_id": snapshot(),
            "policy_snapshot_id": sha256(b"policy"), "review_unit_ids": [], "obligation_id": null,
            "question": "src", "purpose": "DISCOVERY", "seed_refs": [], "requested_operations": [],
            "remaining_budget": 6000, "access_scope_id": "local:fixture"},
        "fragments": fragments, "links": [], "queries": [], "excluded": [], "gaps": [],
        "usage": {"tokens": 30, "files": 2, "semantic_items": 0}, "resolution": "SATISFIED"
    }))
    .expect("packet");
    let source = source_evidence();
    let mut context = Context::default();
    context.add_packet(&packet, &source, reader);
    (context, source)
}

fn anchor(ref_id: &str, start: u32, end: u32) -> Anchor {
    Anchor {
        ref_id: ref_id.into(),
        start_line: start,
        end_line: end,
    }
}

fn obligation(criteria: &[&str]) -> PlannedObligation {
    let criteria: Vec<String> = criteria.iter().map(|c| (*c).to_owned()).collect();
    PlannedObligation {
        obligation: Obligation {
            id: ObligationId(uuid::Uuid::from_u128(7)),
            rule_id: "ORDER-01".into(),
            rule_version: 1,
            question: "total() returns 0 for missing items".into(),
            required: true,
            risk_level: RiskLevel::High,
            review_unit_ids: Vec::new(),
            code_refs: Vec::new(),
            applicability: Applicability::Applies,
            acceptance_criterion: criteria.join("; "),
            acceptable_evidence_kinds: Vec::new(),
            status: ObligationStatus::Unknown {
                reason_code: "NOT_ASSESSED".into(),
                next_evidence_needed: "an assessment".into(),
            },
            evidence_ids: Vec::new(),
            limitations: Vec::new(),
            waivers: Vec::new(),
            evaluation_mode: None,
        },
        rule_title: "Missing items total zero".into(),
        criteria,
        severity_on_violation: Some(Severity::High),
        exceptions: Vec::new(),
        contracts: Vec::new(),
        about_tests: false,
    }
}

fn discovery(value: serde_json::Value) -> DiscoveryOutput {
    serde_json::from_value(value).expect("discovery output")
}

fn draft(
    anchors: &serde_json::Value,
    evidence: &[&str],
    obligations: &[&str],
) -> serde_json::Value {
    json!({"category": "DEFECT", "claim": "guard removed", "conditions": ["items undefined"],
           "causal_chain": ["guard gone", "reduce on undefined"], "consequence": "throws",
           "anchors": anchors, "evidence_ids": evidence, "obligation_ids": obligations,
           "proposed_severity": "HIGH", "disproof_question": "is items always defined?"})
}

#[test]
fn discovery_keeps_only_handed_out_references() {
    let reader = Reader::new();
    let (context, source) = context(&reader);
    let obligations = [ObligationId(uuid::Uuid::from_u128(7))];
    let output = discovery(json!({
        "candidates": [
            draft(&json!([{"ref_id": "ref-2", "start_line": 2, "end_line": 2}]), &["ev-1"], &["ob-1"]),
            draft(&json!([{"ref_id": "ref-9", "start_line": 1, "end_line": 1}]), &[], &[]),
            draft(&json!([{"ref_id": "ref-2", "start_line": 2, "end_line": 2}]), &["ev-7"], &[]),
            draft(&json!([{"ref_id": "ref-2", "start_line": 2, "end_line": 2}]), &[], &["ob-3"]),
            draft(&json!([{"ref_id": "ref-2", "start_line": 3, "end_line": 9}]), &[], &[]),
            draft(&json!([]), &[], &[]),
        ],
        "context_requests": (0..6).map(|i| json!({"question": format!("q{i}"),
            "operation": {"op": "search_source", "side": "HEAD", "query": "total("}})).collect::<Vec<_>>(),
        "unresolved_questions": ["is items typed optional?"]
    }));
    let found = validate_discovery(output, "discovery@1:0", &context, &obligations, &reader);
    assert_eq!(found.candidates.len(), 1, "{:?}", found.rejected);
    let candidate = &found.candidates[0];
    assert_eq!(
        candidate.anchors,
        [reader.at(SourceSide::Head, "src/order.ts", 2, 2)]
    );
    assert_eq!(candidate.evidence_ids, [source.id]);
    assert_eq!(candidate.obligation_ids, obligations);
    assert_eq!(candidate.verification_status, VerificationStatus::Pending);
    let problems: Vec<&str> = found
        .rejected
        .iter()
        .flat_map(|r| r.problems.iter().map(String::as_str))
        .collect();
    for expected in [
        "unknown ref id `ref-9`",
        "unknown evidence id `ev-7`",
        "unknown obligation id `ob-3`",
        "lines 3-9 are outside ref-2 (1-3)",
        "no anchor in the shown spans",
    ] {
        assert!(problems.contains(&expected), "{expected} in {problems:?}");
    }
    assert_eq!(found.asks.len(), 4);
    assert_eq!(found.rejected_asks.len(), 2);
    assert_eq!(found.unresolved_questions, ["is items typed optional?"]);
}

#[test]
fn empty_discovery_is_a_valid_answer() {
    let reader = Reader::new();
    let (context, _) = context(&reader);
    let output =
        discovery(json!({"candidates": [], "context_requests": [], "unresolved_questions": []}));
    let found = validate_discovery(output, "k", &context, &[], &reader);
    assert!(found.candidates.is_empty() && found.rejected.is_empty());
}

#[test]
fn candidate_overflow_is_refused_by_count() {
    let reader = Reader::new();
    let (context, _) = context(&reader);
    let one = draft(
        &json!([{"ref_id": "ref-2", "start_line": 2, "end_line": 2}]),
        &[],
        &[],
    );
    let output = discovery(
        json!({"candidates": vec![one; 22], "context_requests": [], "unresolved_questions": []}),
    );
    let found = validate_discovery(output, "k", &context, &[], &reader);
    assert_eq!(found.candidates.len(), 20);
    assert_eq!(found.rejected.len(), 2);
    assert!(found.rejected[0].problems[0].starts_with("CANDIDATE_OVERFLOW"));
}

fn assessment(status: ProposedStatus, results: Vec<CriterionResult>) -> AssessmentOutput {
    AssessmentOutput {
        obligation_id: "ob-1".into(),
        applicability: Applicability::Applies,
        proposed_status: status,
        criterion_results: results,
        evidence_ids: Vec::new(),
        rationale: "short".into(),
        limitations: Vec::new(),
        missing_inputs: Vec::new(),
        not_applicable_basis: None,
    }
}

fn criterion(id: &str, status: CriterionStatus, anchors: Vec<Anchor>) -> CriterionResult {
    CriterionResult {
        criterion_id: id.into(),
        status,
        anchors,
        evidence_ids: Vec::new(),
    }
}

fn reason(status: &ObligationStatus) -> &str {
    match status {
        ObligationStatus::Unknown { reason_code, .. } => reason_code,
        ObligationStatus::Supported => "SUPPORTED",
        ObligationStatus::Violated => "VIOLATED",
        ObligationStatus::NotApplicable { .. } => "NOT_APPLICABLE",
    }
}

#[test]
fn the_hunks_of_shown_changed_files_are_listed_and_others_are_not() {
    let reader = Reader::new();
    let (mut ctx, _) = context(&reader);
    let mut manifest: DiffManifest = serde_json::from_str(include_str!(
        "../../../../fixtures/publication/diff-manifest.json"
    ))
    .expect("manifest");
    let file = &mut manifest.files[0];
    for side in file.old.iter_mut().chain(file.new.iter_mut()) {
        side.path = path("src/order.ts");
    }
    ctx.add_changes(&manifest);
    let block = ctx.blocks.last().expect("changes block");
    assert!(block.label.starts_with("every hunk"), "{}", block.label);
    assert_eq!(
        block.content,
        "src/order.ts Modified: 5 lines removed, 7 added; -41,5 +41,7\n"
    );
}

#[test]
fn a_repeated_criterion_id_is_not_settled_by_its_first_answer() {
    let reader = Reader::new();
    let (context, _) = context(&reader);
    let planned = obligation(&["returns 0 when items is undefined", "never throws"]);
    let met = |id| criterion(id, CriterionStatus::Met, vec![anchor("ref-1", 2, 4)]);
    let output = assessment(
        ProposedStatus::Supported,
        vec![
            met("c-1"),
            criterion("c-1", CriterionStatus::NotMet, vec![anchor("ref-2", 2, 2)]),
            met("c-2"),
        ],
    );
    let record = validate_assessment(output, &planned, "ob-1", &context, &reader);
    assert_ne!(reason(&record.assessment.status), "SUPPORTED");
    assert!(
        record
            .problems
            .iter()
            .any(|p| p.contains("no single result for c-1")),
        "{:?}",
        record.problems
    );
}

#[test]
fn assessment_statuses_follow_the_criteria() {
    let reader = Reader::new();
    let (context, source) = context(&reader);
    let planned = obligation(&["returns 0 when items is undefined", "never throws"]);
    let met = |id| criterion(id, CriterionStatus::Met, vec![anchor("ref-1", 2, 4)]);
    let check = |output: AssessmentOutput| {
        validate_assessment(output, &planned, "ob-1", &context, &reader).assessment
    };

    let supported = check(assessment(
        ProposedStatus::Supported,
        vec![met("c-1"), met("c-2")],
    ));
    assert_eq!(reason(&supported.status), "SUPPORTED");
    assert_eq!(
        supported.evidence_ids,
        [source.id],
        "anchors cite the shown source"
    );

    let missing = check(assessment(ProposedStatus::Supported, vec![met("c-1")]));
    assert_eq!(reason(&missing.status), "CRITERIA_NOT_MET");

    let contradictory = check(assessment(
        ProposedStatus::Supported,
        vec![
            met("c-1"),
            criterion("c-2", CriterionStatus::NotMet, vec![anchor("ref-2", 2, 2)]),
        ],
    ));
    assert_eq!(reason(&contradictory.status), "CONTRADICTORY_ASSESSMENT");

    let violated = check(assessment(
        ProposedStatus::Violated,
        vec![
            met("c-1"),
            criterion("c-2", CriterionStatus::NotMet, vec![anchor("ref-2", 2, 2)]),
        ],
    ));
    assert_eq!(reason(&violated.status), "VIOLATED");

    let unanchored = check(assessment(
        ProposedStatus::Violated,
        vec![
            met("c-1"),
            criterion("c-2", CriterionStatus::NotMet, Vec::new()),
        ],
    ));
    assert_eq!(reason(&unanchored.status), "CONTRADICTORY_ASSESSMENT");

    let test_only = |id| criterion(id, CriterionStatus::Met, vec![anchor("ref-3", 2, 2)]);
    let oracle = check(assessment(
        ProposedStatus::Supported,
        vec![test_only("c-1"), test_only("c-2")],
    ));
    assert_eq!(reason(&oracle.status), "TEST_ORACLE_ONLY");
    let mut integrity = planned.clone();
    integrity.obligation.rule_id = "TEST-INTEGRITY".into();
    let on_tests = |p: &PlannedObligation| {
        let output = assessment(
            ProposedStatus::Supported,
            vec![test_only("c-1"), test_only("c-2")],
        );
        let status = validate_assessment(output, p, "ob-1", &context, &reader)
            .assessment
            .status;
        reason(&status).to_owned()
    };
    assert_eq!(
        on_tests(&integrity),
        "TEST_ORACLE_ONLY",
        "the id alone exempts nothing"
    );
    integrity.about_tests = true;
    assert_eq!(
        on_tests(&integrity),
        "SUPPORTED",
        "test files are the evidence of the shipped rule about tests"
    );

    let mut no_basis = assessment(ProposedStatus::NotApplicable, Vec::new());
    no_basis.applicability = Applicability::DoesNotApply;
    assert_eq!(
        reason(&check(no_basis.clone()).status),
        "APPLICABILITY_UNKNOWN"
    );
    no_basis.not_applicable_basis = Some("no caller passes undefined: see ref-1".into());
    assert_eq!(reason(&check(no_basis).status), "NOT_APPLICABLE");

    let mut unknown = assessment(ProposedStatus::Unknown, Vec::new());
    unknown.missing_inputs = vec!["callers of total()".into()];
    let ObligationStatus::Unknown {
        next_evidence_needed,
        ..
    } = check(unknown).status
    else {
        panic!("unknown");
    };
    assert_eq!(next_evidence_needed, "callers of total()");
}

#[test]
fn invalid_references_never_raise_an_assessment() {
    let reader = Reader::new();
    let (context, _) = context(&reader);
    let planned = obligation(&["returns 0 when items is undefined"]);
    let check = |output: AssessmentOutput| {
        validate_assessment(output, &planned, "ob-1", &context, &reader).assessment
    };
    let met = || criterion("c-1", CriterionStatus::Met, vec![anchor("ref-1", 2, 4)]);

    let mut other = assessment(ProposedStatus::Supported, vec![met()]);
    other.obligation_id = "ob-7".into();
    let mut ghost_ref = met();
    ghost_ref.anchors.push(anchor("ref-999", 1, 1));
    let mut ghost_ev = met();
    ghost_ev.evidence_ids = vec!["ev-999".into()];
    for output in [
        other,
        assessment(ProposedStatus::Supported, vec![ghost_ref]),
        assessment(ProposedStatus::Supported, vec![ghost_ev]),
    ] {
        let a = check(output);
        assert_eq!(
            reason(&a.status),
            "INVALID_REFERENCE",
            "{:?}",
            a.limitations
        );
    }
}

#[test]
fn a_violation_resting_only_on_unseen_code_is_unknown() {
    let reader = Reader::new();
    let (context, _) = context(&reader);
    let planned = obligation(&["a failed publication fails the save"]);
    let violated = |r: &str, line| {
        validate_assessment(
            assessment(
                ProposedStatus::Violated,
                vec![criterion(
                    "c-1",
                    CriterionStatus::NotMet,
                    vec![anchor(r, line, line)],
                )],
            ),
            &planned,
            "ob-1",
            &context,
            &reader,
        )
    };
    let unseen = reader.at(SourceSide::Head, "src/order.ts", 2, 2);
    let status = |r, line| {
        unseen_violation(violated(r, line), &[&unseen])
            .assessment
            .status
    };
    assert_eq!(reason(&status("ref-2", 2)), "SEMANTIC_GAP");
    assert_eq!(
        reason(&status("ref-2", 3)),
        "VIOLATED",
        "shown elsewhere in the change"
    );
    assert_eq!(
        reason(&status("ref-1", 2)),
        "VIOLATED",
        "a removal on the base side is shown"
    );
}
