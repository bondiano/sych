//! Reducer monotonicity: a linked nit never softens a proven violation, and a waiver scoped to
//! one file never lifts the rule for another.

use std::slice;

use sych_core::config::{PolicyFiles, RepositoryPolicy, RuleException, ServerConfig};
use sych_core::{
    BindingKind, ChangeStatus, CodeRef, Completeness, ContentKind, DEFAULT_ATTENTION_THRESHOLD,
    DiffManifest, Evidence, EvidenceBinding, EvidenceId, EvidenceKind, ExecutionOutcome,
    FileChange, FileSide, Finding, FindingCategory, FindingId, GitHashAlgorithm, GitObjectId,
    ObligationStatus, Producer, Recommendation, ReduceInput, Reduced, ReviewPlan, RunFacts,
    SchemaVersion, Severity, SourceSide, SourceSnapshotId, Timestamp, TrustClass, Validity, plan,
    reduce, sha256,
};

const RULE: &str = "schema_version: 1\nrules:\n  - id: LIVE-01\n    version: 1\n    title: Live path\n    kind: invariant\n    required: true\n    severity_on_violation: high\n    owner: backend\n    applies_to:\n      paths: [\"src/**\"]\n    statement: Live code keeps the guard.\n";

fn oid(c: char) -> GitObjectId {
    GitObjectId::new(GitHashAlgorithm::Sha1, &c.to_string().repeat(40)).expect("oid")
}

fn snapshot() -> SourceSnapshotId {
    SourceSnapshotId(sha256(b"reduce fixture"))
}

fn at() -> Timestamp {
    "2026-10-25T00:00:00Z".parse().expect("timestamp")
}

fn changed(path: &str) -> FileChange {
    FileChange {
        status: ChangeStatus::Modified,
        old: None,
        new: Some(FileSide {
            path: path.parse().expect("path"),
            mode: "100644".into(),
            oid: oid('b'),
            content: ContentKind::Text,
            line_count: Some(10),
        }),
        similarity: None,
        generated: false,
        hunks: Vec::new(),
        limitations: Vec::new(),
    }
}

fn two_file_plan() -> ReviewPlan {
    let mut files = PolicyFiles::default();
    files.files.insert(
        ".sych/rules.yaml".parse().expect("path"),
        RULE.as_bytes().to_vec(),
    );
    let policy = RepositoryPolicy::parse(&files, &ServerConfig::engine_default()).expect("policy");
    let manifest = DiffManifest {
        schema_version: SchemaVersion,
        method: "fixture".into(),
        diff_base: oid('a'),
        head: oid('b'),
        files: vec![changed("src/legacy.go"), changed("src/live.go")],
        omissions: Vec::new(),
    };
    plan(&snapshot(), &manifest, &policy, &RunFacts::default())
}

fn anchored(path: &str) -> Evidence {
    Evidence {
        schema_version: SchemaVersion,
        id: EvidenceId(uuid::Uuid::from_u128(1)),
        kind: EvidenceKind::Source,
        source_snapshot_id: snapshot(),
        producer: Producer {
            name: "fixture".into(),
            version: "1".into(),
            config_hash: None,
        },
        binding: EvidenceBinding {
            kind: BindingKind::Source,
            head_oid: Some(oid('b')),
            target_tip_oid: None,
            tested_oid: None,
            environment_id: None,
        },
        validity: Validity::Valid,
        completeness: Completeness::CompleteForQuery,
        trust_class: TrustClass::SourceObservation,
        artifact_sha256: sha256(b"1"),
        code_refs: vec![CodeRef {
            snapshot_id: snapshot(),
            side: SourceSide::Head,
            path: path.parse().expect("path"),
            blob_oid: oid('b'),
            start_line: 3,
            end_line: 3,
            content_sha256: sha256(b"line"),
        }],
        limitations: Vec::new(),
        observed_at: at(),
    }
}

fn violated(plan: &ReviewPlan, ev: &Evidence, findings: &[Finding]) -> Reduced {
    let assessment = sych_core::Assessment {
        rule_id: "LIVE-01".into(),
        status: ObligationStatus::Violated,
        evidence_ids: vec![ev.id],
        limitations: Vec::new(),
    };
    reduce(&ReduceInput {
        plan,
        assessments: slice::from_ref(&assessment),
        evidence: slice::from_ref(ev),
        findings,
        candidates: &[],
        task_gaps: &[],
        questions: &[],
        outcome: ExecutionOutcome::PlanExecuted,
        attention_threshold: DEFAULT_ATTENTION_THRESHOLD,
        now: at(),
    })
}

#[test]
fn a_linked_nit_does_not_hide_a_high_violation() {
    let plan = two_file_plan();
    let ev = anchored("src/live.go");
    let mut nit = violated(&plan, &ev, &[]).findings.remove(0);
    nit.id = FindingId(uuid::Uuid::from_u128(9));
    nit.category = FindingCategory::Nit;
    nit.severity = Severity::Low;
    let r = violated(&plan, &ev, slice::from_ref(&nit));
    assert_eq!(r.recommendation, Recommendation::AttentionRequired);
    assert!(
        r.findings
            .iter()
            .any(|f| f.category == FindingCategory::RuleViolation && f.severity == Severity::High)
    );
}

#[test]
fn a_waiver_for_one_file_does_not_lift_the_rule_for_another() {
    let waived = |anchor: &str| {
        let mut plan = two_file_plan();
        plan.obligations[0].exceptions = vec![RuleException {
            paths: vec!["src/legacy.go".into()],
            reason: "legacy is retired next sprint".into(),
            owner: "backend".into(),
            author: "dev".into(),
            expires_at: Some("2026-11-01T00:00:00Z".parse().expect("timestamp")),
            until: None,
        }];
        violated(&plan, &anchored(anchor), &[])
    };
    let live = waived("src/live.go");
    assert!(live.obligations[0].obligation.waivers.is_empty());
    assert_eq!(live.recommendation, Recommendation::AttentionRequired);

    let legacy = waived("src/legacy.go");
    assert_eq!(legacy.obligations[0].obligation.waivers.len(), 1);
    assert_eq!(
        legacy.obligations[0].obligation.status,
        ObligationStatus::Violated
    );
    assert_eq!(
        legacy.recommendation,
        Recommendation::NoBlockingFindingsInScope
    );
}
