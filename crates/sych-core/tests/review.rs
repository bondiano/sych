//! Planner and reducer scenarios. They check obligations, coverage and
//! recommendation, never explanation wording.

use std::path::Path;
use std::{env, fs, slice};

use sych_core::config::{
    EffectiveConfig, PolicyFiles, PolicyInput, RepositoryPolicy, RuleException, ServerConfig,
    ServerInput,
};
use sych_core::{
    Applicability, Assessment, BindingKind, ChangeStatus, Completeness, ContentKind, Coverage,
    DEFAULT_ATTENTION_THRESHOLD, DiffManifest, Evidence, EvidenceBinding, EvidenceId, EvidenceKind,
    EvidenceManifest, ExecutionOutcome, ExternalInputManifest, FileChange, FileSide,
    FindingCategory, FreezeError, GitHashAlgorithm, GitObjectId, ObligationStatus, PolicySource,
    Producer, Provenance, Recommendation, ReduceInput, Reduced, ResultId, ReviewPlan, ReviewResult,
    RunFacts, RunId, RunInfo, SchemaVersion, Severity, SourceSnapshotId, Timestamp, TrustClass,
    UsageSummary, Validity, decode, finalize, plan, reduce, sha256,
};

const RULES: &str = r#"schema_version: 1
rules:
  - id: EFFECT-01
    version: 1
    title: Order event is published
    kind: invariant
    required: true
    severity_on_violation: high
    owner: backend
    applies_to:
      paths: ["backend/orders/**"]
    statement: A stored order publishes order-created.
    acceptance:
      evidence_kinds: ["source", "test"]
      criteria: ["Commit boundary and delivery mechanism are identified."]
  - id: FRONT-01
    version: 1
    title: Frontend keeps the order contract
    kind: invariant
    required: true
    severity_on_violation: medium
    owner: frontend
    applies_to:
      paths: ["frontend/**"]
    statement: The order form posts the accepted contract.
  - id: NAMING-01
    version: 1
    title: Names follow the glossary
    kind: preference
    required: false
    owner: backend
    statement: Public names use glossary terms.
"#;

fn oid(c: char) -> GitObjectId {
    GitObjectId::new(GitHashAlgorithm::Sha1, &c.to_string().repeat(40)).expect("oid")
}

fn snapshot_id() -> SourceSnapshotId {
    SourceSnapshotId(sha256(b"review fixture snapshot"))
}

fn at() -> Timestamp {
    "2026-10-25T00:00:00Z".parse().expect("timestamp")
}

fn file(path: &str, content: ContentKind) -> FileChange {
    FileChange {
        status: ChangeStatus::Modified,
        old: None,
        new: Some(FileSide {
            path: path.parse().expect("path"),
            mode: "100644".into(),
            oid: oid('b'),
            content,
            line_count: Some(10),
        }),
        similarity: None,
        generated: false,
        generated_claim: false,
        hunks: Vec::new(),
        limitations: Vec::new(),
    }
}

fn manifest(files: Vec<FileChange>) -> DiffManifest {
    DiffManifest {
        schema_version: SchemaVersion,
        method: "fixture".into(),
        diff_base: oid('a'),
        head: oid('b'),
        files,
        omissions: Vec::new(),
    }
}

fn effective(max_source_files: Option<u32>) -> EffectiveConfig {
    let mut files = PolicyFiles::default();
    files.files.insert(
        ".sych/rules.yaml".parse().expect("path"),
        RULES.as_bytes().to_vec(),
    );
    if let Some(max) = max_source_files {
        files.files.insert(
            ".sych/config.yaml".parse().expect("path"),
            format!("schema_version: 1\ncontext:\n  max_source_files: {max}\n").into_bytes(),
        );
    }

    EffectiveConfig::build(
        &ServerInput {
            origin: "<engine>",
            text: None,
        },
        &PolicyInput {
            origin: "refs/heads/main",
            files: &files,
        },
    )
    .expect("policy is valid")
}

fn orders_change() -> DiffManifest {
    manifest(vec![
        file("assets/logo.png", ContentKind::Binary),
        file("backend/orders/create.go", ContentKind::Text),
    ])
}

fn evidence(
    n: u128,
    kind: EvidenceKind,
    completeness: Completeness,
    trust: TrustClass,
) -> Evidence {
    Evidence {
        schema_version: SchemaVersion,
        id: EvidenceId(uuid::Uuid::from_u128(n)),
        kind,
        source_snapshot_id: snapshot_id(),
        producer: Producer {
            name: "fixture".into(),
            version: "1".into(),
            config_hash: None,
        },
        binding: EvidenceBinding {
            kind: if trust == TrustClass::AuthenticatedCi {
                BindingKind::Ci
            } else {
                BindingKind::Source
            },
            head_oid: Some(oid('b')),
            target_tip_oid: Some(oid('c')),
            tested_oid: None,
            environment_id: None,
        },
        validity: Validity::Valid,
        completeness,
        trust_class: trust,
        artifact_sha256: sha256(&n.to_be_bytes()),
        code_refs: Vec::new(),
        limitations: Vec::new(),
        observed_at: at(),
    }
}

fn source_evidence() -> Evidence {
    evidence(
        1,
        EvidenceKind::Source,
        Completeness::CompleteForQuery,
        TrustClass::SourceObservation,
    )
}

fn assess(status: ObligationStatus, evidence: &[&Evidence]) -> Assessment {
    Assessment {
        rule_id: "EFFECT-01".into(),
        status,
        evidence_ids: evidence.iter().map(|e| e.id).collect(),
        limitations: Vec::new(),
    }
}

fn run(
    plan: &ReviewPlan,
    assessments: &[Assessment],
    evidence: &[Evidence],
    outcome: ExecutionOutcome,
) -> Reduced {
    reduce(&ReduceInput {
        task_gaps: &[],
        questions: &[],
        plan,
        assessments,
        evidence,
        findings: &[],
        candidates: &[],
        outcome,
        attention_threshold: DEFAULT_ATTENTION_THRESHOLD,
        now: at(),
    })
}

fn status_of<'a>(reduced: &'a Reduced, rule: &str) -> &'a ObligationStatus {
    &reduced
        .obligations
        .iter()
        .find(|o| o.obligation.rule_id == rule)
        .expect("obligation exists")
        .obligation
        .status
}

fn orders_plan() -> ReviewPlan {
    plan(
        &snapshot_id(),
        &orders_change(),
        &effective(None).repository,
        &RunFacts::default(),
    )
}

#[test]
fn plan_fixes_units_obligations_and_exclusions_before_discovery() {
    let plan = orders_plan();
    assert_eq!(plan.scope.units.len(), 2, "one unit per directory");
    assert_eq!(plan.scope.excluded[0].reason, "BINARY");
    assert!(plan.unreviewed.is_empty());
    let rules: Vec<_> = plan
        .obligations
        .iter()
        .map(|o| (o.obligation.rule_id.as_str(), o.obligation.applicability))
        .collect();
    // Preferences do not become obligations.
    assert_eq!(
        rules,
        [
            ("EFFECT-01", Applicability::Applies),
            ("FRONT-01", Applicability::DoesNotApply),
        ]
    );
    assert_eq!(plan.scope.required_obligation_ids.len(), 2);
    assert_eq!(plan, orders_plan(), "planning is deterministic");
}

#[test]
fn safe_change_has_no_blocking_findings_and_complete_coverage() {
    let ev = source_evidence();
    let r = run(
        &orders_plan(),
        &[assess(ObligationStatus::Supported, &[&ev])],
        slice::from_ref(&ev),
        ExecutionOutcome::PlanExecuted,
    );
    assert_eq!(status_of(&r, "EFFECT-01"), &ObligationStatus::Supported);
    assert!(matches!(
        status_of(&r, "FRONT-01"),
        ObligationStatus::NotApplicable { .. }
    ));
    assert_eq!(r.coverage, Coverage::CompleteForScope);
    assert_eq!(r.recommendation, Recommendation::NoBlockingFindingsInScope);
    assert!(r.findings.is_empty());
}

#[test]
fn checked_violation_is_kept_without_findings() {
    let ev = source_evidence();
    let r = run(
        &orders_plan(),
        &[assess(ObligationStatus::Violated, &[&ev])],
        slice::from_ref(&ev),
        ExecutionOutcome::PlanExecuted,
    );
    assert_eq!(r.recommendation, Recommendation::AttentionRequired);
    assert_eq!(r.findings.len(), 1);
    assert_eq!(r.findings[0].category, FindingCategory::RuleViolation);
    assert_eq!(r.findings[0].severity, Severity::High);
    assert_eq!(r.findings[0].rule_id.as_deref(), Some("EFFECT-01"));
}

#[test]
fn a_waiver_keeps_the_violation_but_lowers_the_risk_decision() {
    let ev = source_evidence();
    let waived = |expires: &str| {
        let mut plan = orders_plan();
        plan.obligations[0].exceptions = vec![RuleException {
            paths: vec!["backend/orders/**".into()],
            reason: "legacy importer is retired next sprint".into(),
            owner: "backend".into(),
            author: "dev".into(),
            expires_at: Some(expires.parse().expect("timestamp")),
            until: None,
        }];
        run(
            &plan,
            &[assess(ObligationStatus::Violated, &[&ev])],
            slice::from_ref(&ev),
            ExecutionOutcome::PlanExecuted,
        )
    };
    let r = waived("2026-11-01T00:00:00Z");
    assert_eq!(status_of(&r, "EFFECT-01"), &ObligationStatus::Violated);
    assert_eq!(r.obligations[0].obligation.waivers.len(), 1);
    assert_eq!(r.findings.len(), 1, "the violation finding is kept");
    assert_eq!(r.recommendation, Recommendation::NoBlockingFindingsInScope);

    let expired = waived("2026-10-01T00:00:00Z");
    assert!(expired.obligations[0].obligation.waivers.is_empty());
    assert!(
        expired.obligations[0]
            .obligation
            .limitations
            .iter()
            .any(|l| l.contains("expired"))
    );
    assert_eq!(expired.recommendation, Recommendation::AttentionRequired);
}

#[test]
fn empty_discovery_and_weak_evidence_leave_obligations_unknown() {
    let not_assessed = run(&orders_plan(), &[], &[], ExecutionOutcome::PlanExecuted);
    assert!(matches!(
        status_of(&not_assessed, "EFFECT-01"),
        ObligationStatus::Unknown { reason_code, .. } if reason_code == "NOT_ASSESSED"
    ));
    assert_eq!(not_assessed.recommendation, Recommendation::NeedsEvidence);
    assert_eq!(not_assessed.coverage, Coverage::Partial);
    assert!(!not_assessed.unresolved_questions.is_empty());

    let author = evidence(
        2,
        EvidenceKind::Test,
        Completeness::CompleteForQuery,
        TrustClass::AuthorClaim,
    );
    let partial = evidence(
        3,
        EvidenceKind::Source,
        Completeness::Partial,
        TrustClass::SourceObservation,
    );
    let wrong_kind = evidence(
        4,
        EvidenceKind::Human,
        Completeness::CompleteForQuery,
        TrustClass::HumanAttestation,
    );
    for cited in [vec![], vec![&author], vec![&partial], vec![&wrong_kind]] {
        let all = [author.clone(), partial.clone(), wrong_kind.clone()];
        let r = run(
            &orders_plan(),
            &[assess(ObligationStatus::Supported, &cited)],
            &all,
            ExecutionOutcome::PlanExecuted,
        );
        assert!(
            matches!(
                status_of(&r, "EFFECT-01"),
                ObligationStatus::Unknown { reason_code, .. } if reason_code == "INSUFFICIENT_EVIDENCE"
            ),
            "{cited:?}"
        );
        assert_eq!(r.recommendation, Recommendation::NeedsEvidence);
    }
}

#[test]
fn not_applicable_is_not_reopened_by_an_assessment() {
    let ev = source_evidence();
    let mut violated = assess(ObligationStatus::Violated, &[&ev]);
    violated.rule_id = "FRONT-01".into();
    let r = run(
        &orders_plan(),
        &[violated],
        slice::from_ref(&ev),
        ExecutionOutcome::PlanExecuted,
    );
    assert!(matches!(
        status_of(&r, "FRONT-01"),
        ObligationStatus::NotApplicable { .. }
    ));
    assert!(r.findings.is_empty());
}

#[test]
fn change_kinds_without_semantic_provider_stay_unknown() {
    let rules = RULES.replace(
        "      paths: [\"backend/orders/**\"]\n",
        "      paths: [\"backend/orders/**\"]\n      change_kinds: [\"call_change\"]\n",
    );
    let mut files = PolicyFiles::default();
    files.files.insert(
        ".sych/rules.yaml".parse().expect("path"),
        rules.into_bytes(),
    );
    let policy = RepositoryPolicy::parse(&files, &ServerConfig::engine_default()).expect("valid");
    let plan = plan(
        &snapshot_id(),
        &orders_change(),
        &policy,
        &RunFacts::default(),
    );
    let effect = &plan.obligations[0].obligation;
    assert_eq!(effect.applicability, Applicability::Unknown);
    assert!(matches!(
        &effect.status,
        ObligationStatus::Unknown { reason_code, .. } if reason_code == "CHANGE_KIND_UNRESOLVED"
    ));
}

#[test]
fn no_op_diff_has_an_explicit_empty_scope() {
    let plan = plan(
        &snapshot_id(),
        &manifest(Vec::new()),
        &effective(None).repository,
        &RunFacts::default(),
    );
    assert!(plan.scope.units.is_empty() && plan.obligations.is_empty());
    let r = run(&plan, &[], &[], ExecutionOutcome::PlanExecuted);
    assert_eq!(r.coverage, Coverage::CompleteForScope);
    assert_eq!(r.recommendation, Recommendation::NoBlockingFindingsInScope);
}

#[test]
fn overflow_lists_unreviewed_files_and_is_never_complete() {
    let change = manifest(vec![
        file("backend/orders/a.go", ContentKind::Text),
        file("backend/orders/b.go", ContentKind::Text),
    ]);
    let plan = plan(
        &snapshot_id(),
        &change,
        &effective(Some(1)).repository,
        &RunFacts::default(),
    );
    assert!(plan.limit_reached);
    assert_eq!(plan.unreviewed[0].path.as_str(), "backend/orders/b.go");
    assert_eq!(plan.unreviewed[0].reason, "LIMIT_MAX_SOURCE_FILES");
    let ev = source_evidence();
    let supported = [assess(ObligationStatus::Supported, &[&ev])];
    let r = run(
        &plan,
        &supported,
        slice::from_ref(&ev),
        ExecutionOutcome::LimitReached,
    );
    assert_eq!(r.coverage, Coverage::Partial);
    assert!(
        r.excluded_or_unreviewed
            .iter()
            .any(|g| g.reason == "LIMIT_MAX_SOURCE_FILES")
    );
    // Mandatory scope is not narrowed to the reviewed part.
    assert_eq!(plan.obligations[0].obligation.review_unit_ids.len(), 1);
}

#[test]
fn unavailable_input_needs_evidence() {
    let r = run(&orders_plan(), &[], &[], ExecutionOutcome::InputUnavailable);
    assert_eq!(r.coverage, Coverage::Unavailable);
    assert_eq!(r.recommendation, Recommendation::NeedsEvidence);
}

#[test]
fn late_external_input_is_rejected_at_finalization() {
    let policy = effective(None)
        .policy_snapshot(PolicySource::EngineDefaults, at())
        .expect("policy");
    let ci = evidence(
        5,
        EvidenceKind::Test,
        Completeness::CompleteForQuery,
        TrustClass::AuthenticatedCi,
    );
    let frozen =
        ExternalInputManifest::freeze(&snapshot_id(), &policy.id, &[], at()).expect("freeze");
    assert_eq!(
        EvidenceManifest::freeze(&frozen, &[source_evidence(), ci.clone()], at()),
        Err(FreezeError::LateExternalInput(ci.id))
    );
    // A definition found in the pinned snapshot after the freeze is derived evidence of this
    // run, never a late external input.
    let definition = evidence(
        6,
        EvidenceKind::Semantic,
        Completeness::Partial,
        TrustClass::DerivedLocal,
    );
    assert!(EvidenceManifest::freeze(&frozen, &[source_evidence(), definition], at()).is_ok());
    let with_ci =
        ExternalInputManifest::freeze(&snapshot_id(), &policy.id, slice::from_ref(&ci), at())
            .expect("freeze");
    assert!(EvidenceManifest::freeze(&with_ci, &[source_evidence(), ci], at()).is_ok());
    assert_eq!(
        ExternalInputManifest::freeze(&snapshot_id(), &policy.id, &[source_evidence()], at()),
        Err(FreezeError::NotExternal(source_evidence().id))
    );
}

#[test]
fn assessments_reject_unknown_fields() {
    let ok = r#"{"rule_id":"EFFECT-01","status":"SUPPORTED","evidence_ids":[]}"#;
    assert!(serde_json::from_str::<Assessment>(ok).is_ok());
    let extra = r#"{"rule_id":"EFFECT-01","status":"SUPPORTED","confidence":0.99}"#;
    assert!(serde_json::from_str::<Assessment>(extra).is_err());
    let unknown_without_reason = r#"{"rule_id":"EFFECT-01","status":"UNKNOWN"}"#;
    assert!(serde_json::from_str::<Assessment>(unknown_without_reason).is_err());
    let round = assess(ObligationStatus::Supported, &[]);
    let json = serde_json::to_string(&round).expect("json");
    assert_eq!(
        serde_json::from_str::<Assessment>(&json).expect("decodes"),
        round
    );
}

/// One saved run: scope, policy, external inputs, evidence and result, explainable from the
/// rule criterion to the recommendation. Regenerate with `SYCH_BLESS=1`.
#[test]
fn violated_run_bundle_matches_fixture() {
    let config = effective(None);
    let commit = oid('c');
    let policy = config
        .policy_snapshot(
            PolicySource::TrustedRef {
                ref_name: "main".into(),
                commit,
            },
            at(),
        )
        .expect("policy");
    let plan = orders_plan();
    let ci = evidence(
        5,
        EvidenceKind::Test,
        Completeness::CompleteForQuery,
        TrustClass::AuthenticatedCi,
    );
    let all = [source_evidence(), ci.clone()];
    let external =
        ExternalInputManifest::freeze(&snapshot_id(), &policy.id, &[ci], at()).expect("freeze");
    let evidence_manifest = EvidenceManifest::freeze(&external, &all, at()).expect("freeze");
    let reduced = run(
        &plan,
        &[assess(ObligationStatus::Violated, &[&all[0], &all[1]])],
        &all,
        ExecutionOutcome::PlanExecuted,
    );
    let result = finalize(
        RunInfo {
            run_id: RunId(uuid::Uuid::from_u128(100)),
            result_id: ResultId(uuid::Uuid::from_u128(101)),
            review_revision: 1,
            parent_result_id: None,
            usage: UsageSummary::default(),
            provenance: Provenance {
                engine_version: "fixture".into(),
                model_adapters: vec!["fake".into()],
            },
            created_at: at(),
            language: sych_core::ReviewLanguage::En,
        },
        &plan,
        &policy,
        &evidence_manifest,
        ExecutionOutcome::PlanExecuted,
        reduced,
    );
    assert_eq!(result.recommendation, Recommendation::AttentionRequired);
    assert_eq!(result.coverage, Coverage::CompleteForScope);
    let bundle = serde_json::json!({
        "scope_manifest": plan.scope,
        "policy_snapshot": policy,
        "external_input_manifest": external,
        "evidence_manifest": evidence_manifest,
        "review_result": result,
    });
    let text = serde_json::to_string_pretty(&bundle).expect("json") + "\n";
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/review/violated.bundle.json");
    if env::var_os("SYCH_BLESS").is_some() {
        fs::create_dir_all(path.parent().expect("dir")).expect("mkdir");
        fs::write(&path, &text).expect("write");
    }

    assert_eq!(
        fs::read_to_string(&path).unwrap_or_default(),
        text,
        "{} differs; run with SYCH_BLESS=1",
        path.display()
    );
    let saved: ReviewResult =
        decode(&serde_json::to_string(&bundle["review_result"]).expect("json"))
            .expect("saved result decodes");
    assert_eq!(saved, result);
}
