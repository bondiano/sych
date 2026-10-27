//! Rule applicability (roadmap M1.4): the generic pack activates only on its changes, a
//! missing domain checker never excludes a rule, a repository rule replaces the generic one,
//! and moved or linked logic keeps its invariant.

use std::collections::BTreeMap;

use sych_core::analysis::{Context, prompts};
use sych_core::config::{EffectKind, PolicyFiles, RepositoryPolicy, ServerConfig};
use sych_core::effects::{EffectLedger, EffectMatch, LedgerEntry, Resolution};
use sych_core::{
    Applicability, BackendStatus, ChangeStatus, CodeRef, Completeness, ContentKind, DiffManifest,
    EdgeSet, EvaluationMode, FileChange, FileSide, GitHashAlgorithm, GitObjectId, Language,
    NOT_ASSESSED, ObligationStatus, PROVIDER_UNAVAILABLE, PathClass, Producer, QueryStatus,
    ReviewPlan, RunFacts, SchemaVersion, SemanticFacts, SourceSide, SourceSnapshotId, SymbolFacts,
    SyntaxFact, SyntaxFacts, SyntaxKind, plan, sha256,
};

const ORDERS_RULE: &str = "schema_version: 1\nrules:\n  - id: EFFECT-01\n    version: 1\n    title: Order event\n    kind: invariant\n    required: true\n    severity_on_violation: high\n    owner: backend\n    applies_to:\n      paths: [\"backend/orders/**\"]\n    statement: A stored order publishes order-created.\n";

fn oid(c: char) -> GitObjectId {
    GitObjectId::new(GitHashAlgorithm::Sha1, &c.to_string().repeat(40)).expect("oid")
}

fn snapshot() -> SourceSnapshotId {
    SourceSnapshotId(sha256(b"rules fixture"))
}

fn side(path: &str) -> FileSide {
    FileSide {
        path: path.parse().expect("path"),
        mode: "100644".into(),
        oid: oid('b'),
        content: ContentKind::Text,
        line_count: Some(10),
    }
}

fn changed(path: &str) -> FileChange {
    FileChange {
        status: ChangeStatus::Modified,
        old: Some(side(path)),
        new: Some(side(path)),
        similarity: None,
        generated: false,
        hunks: Vec::new(),
        limitations: Vec::new(),
    }
}

fn policy(files: &[(&str, &str)]) -> RepositoryPolicy {
    let mut policy = PolicyFiles::default();
    for (path, text) in files {
        policy
            .files
            .insert(path.parse().expect("path"), text.as_bytes().to_vec());
    }
    RepositoryPolicy::parse(&policy, &ServerConfig::engine_default()).expect("policy")
}

fn planned(files: Vec<FileChange>, policy: &RepositoryPolicy, facts: &RunFacts) -> ReviewPlan {
    let manifest = DiffManifest {
        schema_version: SchemaVersion,
        method: "fixture".into(),
        diff_base: oid('a'),
        head: oid('b'),
        files,
        omissions: Vec::new(),
    };
    plan(&snapshot(), &manifest, policy, facts)
}

fn rule<'a>(plan: &'a ReviewPlan, id: &str) -> Option<&'a sych_core::Obligation> {
    plan.obligations
        .iter()
        .map(|o| &o.obligation)
        .find(|o| o.rule_id == id)
}

fn code_ref(path: &str, line: u32) -> CodeRef {
    CodeRef {
        snapshot_id: snapshot(),
        side: SourceSide::Head,
        path: path.parse().expect("path"),
        blob_oid: oid('b'),
        start_line: line,
        end_line: line,
        content_sha256: sha256(b"line"),
    }
}

#[test]
fn generic_rules_activate_only_on_their_changes() {
    let repo = policy(&[]);
    let quiet = planned(vec![changed("src/lib.rs")], &repo, &RunFacts::default());
    assert!(quiet.obligations.is_empty(), "nothing generic is touched");

    let p = planned(
        vec![
            changed("db/migrations/0002_orders.sql"),
            changed("tests/orders.rs"),
        ],
        &repo,
        &RunFacts::default(),
    );
    // Without deploy.yaml the plan is still reviewed; the gaps are named, not assumed away.
    let mig = rule(&p, "MIG-COMPAT").expect("migration rule");
    assert_eq!(mig.applicability, Applicability::Applies);
    assert_eq!(mig.evaluation_mode, Some(EvaluationMode::PlanReview));
    assert!(
        mig.limitations
            .iter()
            .any(|l| l.starts_with("INITIAL_VERSION_UNKNOWN"))
    );
    let contract = rule(&p, "MIG-CONTRACT").expect("migration rule");
    assert!(matches!(&contract.status,
        ObligationStatus::Unknown { reason_code, .. } if reason_code == "DEPLOYMENT_INPUT_MISSING"));
    assert!(
        contract.required,
        "a missing owner input does not make the rule optional"
    );
    let tests = rule(&p, "TEST-INTEGRITY").expect("test integrity");
    assert_eq!(tests.applicability, Applicability::Applies);
    assert!(rule(&p, "TF-CHANGE").is_none() && rule(&p, "EFFECT-01").is_none());
}

#[test]
fn profiles_enable_generic_rules_and_a_repository_rule_replaces_its_twin() {
    let correctness_only = policy(&[(
        ".sych/config.yaml",
        "schema_version: 1\nreview:\n  profiles: [correctness]\n",
    )]);
    let p = planned(
        vec![changed("db/migrations/0002_orders.sql")],
        &correctness_only,
        &RunFacts::default(),
    );
    assert!(rule(&p, "MIG-COMPAT").is_none());

    let own = ORDERS_RULE
        .replace("EFFECT-01", "TEST-INTEGRITY")
        .replace("backend/orders/**", "tests/**");
    let p = planned(
        vec![changed("tests/orders.rs")],
        &policy(&[(".sych/rules.yaml", &own)]),
        &RunFacts::default(),
    );
    let ids: Vec<_> = p.obligations.iter().map(|o| &o.rule_title).collect();
    assert_eq!(ids, ["Order event"], "one TEST-INTEGRITY, the repository's");
}

#[test]
fn a_skipped_test_inside_a_source_file_triggers_test_integrity() {
    let skipped = SyntaxFacts {
        schema_version: SchemaVersion,
        source_snapshot_id: snapshot(),
        side: SourceSide::Head,
        language: Language::Rust,
        producer: Producer {
            name: "syn".into(),
            version: "2".into(),
            config_hash: None,
        },
        files: BTreeMap::new(),
        facts: vec![SyntaxFact {
            kind: SyntaxKind::TestSkipped,
            code_ref: code_ref("src/lib.rs", 40),
            symbol: None,
            detail: "#[ignore]".into(),
            in_change: true,
            related: Vec::new(),
            subject: None,
            guard: None,
        }],
        limitations: Vec::new(),
    };
    let facts = RunFacts {
        syntax: vec![skipped],
        ..RunFacts::default()
    };
    let p = planned(vec![changed("src/lib.rs")], &policy(&[]), &facts);
    assert!(rule(&p, "TEST-INTEGRITY").is_some());
}

#[test]
fn moving_logic_out_of_the_rule_paths_keeps_the_rule() {
    let rules = policy(&[(".sych/rules.yaml", ORDERS_RULE)]);
    let mut moved = changed("lib/create.go");
    moved.status = ChangeStatus::Renamed;
    moved.old = Some(side("backend/orders/create.go"));
    let p = planned(vec![moved], &rules, &RunFacts::default());
    assert_eq!(
        rule(&p, "EFFECT-01").expect("rule").applicability,
        Applicability::Applies
    );

    // A helper outside the paths that order code calls is linked semantically.
    let answered = |refs| EdgeSet {
        status: QueryStatus::Answered,
        completeness: Completeness::Partial,
        refs,
        external: Vec::new(),
        unresolved: Vec::new(),
    };
    let facts = RunFacts {
        semantic: vec![SemanticFacts {
            schema_version: SchemaVersion,
            source_snapshot_id: snapshot(),
            side: SourceSide::Head,
            language: Language::Rust,
            status: BackendStatus::Ready,
            producer: None,
            position_encoding: None,
            build_configuration: BTreeMap::new(),
            query_scope: Vec::new(),
            symbols: vec![SymbolFacts {
                name: "publish".into(),
                kind: "function".into(),
                container: Vec::new(),
                definition: code_ref("lib/events.go", 3),
                path_class: PathClass::Production,
                references: answered(Vec::new()),
                readers: EdgeSet::not_applicable(),
                writers: EdgeSet::not_applicable(),
                callers: answered(vec![code_ref("backend/orders/create.go", 12)]),
                calling_constructs: Vec::new(),
                callees: answered(Vec::new()),
                type_definitions: answered(Vec::new()),
                implementations: EdgeSet::not_applicable(),
            }],
            diagnostics: Vec::new(),
            limitations: Vec::new(),
        }],
        ..RunFacts::default()
    };
    let linked = planned(vec![changed("lib/events.go")], &rules, &facts);
    let ob = rule(&linked, "EFFECT-01").expect("rule");
    assert_eq!(ob.applicability, Applicability::Applies);
    assert!(ob.limitations[0].contains("backend/orders/create.go:12"));
    let unlinked = planned(vec![changed("lib/events.go")], &rules, &RunFacts::default());
    assert_eq!(
        rule(&unlinked, "EFFECT-01").expect("rule").applicability,
        Applicability::DoesNotApply
    );

    // Renamed helper: only the base side links its old path to order code.
    let mut base = facts.clone();
    base.semantic[0].side = SourceSide::DiffBase;
    base.semantic[0].symbols[0].definition.side = SourceSide::DiffBase;
    let mut renamed = changed("lib/moved.go");
    renamed.status = ChangeStatus::Renamed;
    renamed.old = Some(side("lib/events.go"));
    let p = planned(vec![renamed], &rules, &base);
    assert_eq!(
        rule(&p, "EFFECT-01").expect("rule").applicability,
        Applicability::Applies
    );
}

#[test]
fn a_rule_contract_reaches_its_obligation_and_the_prompt_as_trusted_text() {
    let rules = ORDERS_RULE.replace(
        "    statement:",
        "    contract_refs: [\".sych/contracts/orders.md\"]\n    statement:",
    );
    let repo = policy(&[
        (".sych/rules.yaml", &rules),
        (
            ".sych/contracts/orders.md",
            "order-created carries id and total\n",
        ),
    ]);
    let p = planned(
        vec![changed("backend/orders/store.go")],
        &repo,
        &RunFacts::default(),
    );
    let planned = &p.obligations[0];
    let (path, hash, text) = &planned.contracts[0];
    assert_eq!(path.as_str(), ".sych/contracts/orders.md");
    assert_eq!(*hash, sha256(b"order-created carries id and total\n"));
    let prompt = prompts::assessment(&Context::default(), planned);
    assert!(prompt.task.contains(&format!("contract {path} ({hash}):")));
    assert!(prompt.task.contains(&format!("    {}", text.trim())));
    // The review language is a rule of every prompt, so it is part of the checkpoint key.
    let english = sych_core::ReviewLanguage::En.instruction();
    assert!(prompt.rules.iter().any(|r| r == english));
    let russian = prompts::assessment(&Context::new(sych_core::ReviewLanguage::Ru), planned);
    assert!(russian.rules.iter().any(|r| r.contains("in Russian")));
    assert!(!russian.rules.iter().any(|r| r == english));
}

fn ledger_facts(matched: EffectMatch, notes: &[&str]) -> RunFacts {
    let entry = LedgerEntry {
        effect_id: "db_write:ExecContext@backend/orders/create.go#Store.Create".into(),
        kind: EffectKind::DbWrite,
        target: "ExecContext".into(),
        shape: SyntaxKind::DbCall,
        language: Language::Go,
        path: "backend/orders/create.go".parse().expect("path"),
        symbol: Some("Store.Create".into()),
        registry_id: None,
        durable_with_transaction: None,
        contract_refs: Vec::new(),
        base: Vec::new(),
        head: Vec::new(),
        matched,
        resolution: Resolution::Resolved,
        notes: notes.iter().map(|n| (*n).to_owned()).collect(),
    };
    RunFacts {
        ledger: EffectLedger {
            entries: vec![entry],
            ..EffectLedger::default()
        },
        ..RunFacts::default()
    }
}

#[test]
fn ledger_changes_activate_the_effect_rules_with_a_provider() {
    let repo = policy(&[]);
    let files = || vec![changed("backend/orders/create.go")];
    let preserved = planned(files(), &repo, &ledger_facts(EffectMatch::Preserved, &[]));
    assert!(rule(&preserved, "EFFECT-01").is_none());

    let removed = planned(files(), &repo, &ledger_facts(EffectMatch::Removed, &[]));
    let effect = rule(&removed, "EFFECT-01").expect("activated by the ledger");
    assert_eq!(effect.applicability, Applicability::Applies);
    assert!(!matches!(&effect.status,
        ObligationStatus::Unknown { reason_code, .. } if reason_code == PROVIDER_UNAVAILABLE));
    assert!(rule(&removed, "EFFECT-02").is_none());

    let lifetime = ledger_facts(
        EffectMatch::Changed,
        &["lifetime: caller → detached: go s.publish()"],
    );
    assert!(rule(&planned(files(), &repo, &lifetime), "EFFECT-02").is_some());
}

const DEPLOY: &str = r#"schema_version: 1
environment: staging
owner: payments
recovery: {value: roll_forward_repair, source: runbook RB-12}
excluded_states:
  - {value: contract_phase, source: "contract ships in a separate release (RFC-31)"}
"#;

/// Migration rules are judged over the reachable rollout states (roadmap M3.5): a declared
/// recovery path lets MIG-RECOVERY be assessed, a sourced exclusion drops MIG-CONTRACT.
#[test]
fn migration_rules_follow_the_deployment_model() {
    let repo = policy(&[(".sych/deploy.yaml", DEPLOY)]);
    let p = planned(
        vec![changed("db/migrations/0002_orders.sql")],
        &repo,
        &RunFacts::default(),
    );
    let recovery = rule(&p, "MIG-RECOVERY").expect("migration rule");
    assert!(matches!(&recovery.status,
        ObligationStatus::Unknown { reason_code, .. } if reason_code == NOT_ASSESSED));
    assert!(
        recovery
            .acceptance_criterion
            .contains("RecoveryAfterNewWrites")
    );
    assert!(recovery.acceptance_criterion.contains("PLAN_REVIEW"));

    let contract = rule(&p, "MIG-CONTRACT").expect("migration rule");
    assert_eq!(contract.applicability, Applicability::DoesNotApply);
    assert!(matches!(&contract.status,
        ObligationStatus::NotApplicable { basis } if basis.contains("RFC-31")));

    // Not declaring a recovery path keeps MIG-RECOVERY UNKNOWN on exactly that input.
    let bare = policy(&[(".sych/deploy.yaml", "schema_version: 1\n")]);
    let p = planned(
        vec![changed("db/migrations/0002_orders.sql")],
        &bare,
        &RunFacts::default(),
    );
    let recovery = rule(&p, "MIG-RECOVERY").expect("migration rule");
    assert!(matches!(&recovery.status,
        ObligationStatus::Unknown { reason_code, next_evidence_needed }
            if reason_code == "DEPLOYMENT_INPUT_MISSING"
                && next_evidence_needed.starts_with("RECOVERY_UNKNOWN")));
}
