//! Architecture findings on packet spans: one the change introduced or made worse refutes in
//! its file's packet; existing debt stays out.

use sych_core::architecture::{ArchitectureFacts, ArchitectureFinding, Change, Check, Limits};
use sych_core::broker::{BrokerInput, Priority, build_packets};
use sych_core::config::{PolicyFiles, RepositoryPolicy, ServerConfig};
use sych_core::effects::EffectLedger;
use sych_core::{
    BackendStatus, Language, PolicySnapshotId, Producer, RunFacts, RunId, SchemaVersion,
    SnapshotReader, SourceSide, plan, sha256,
};

use super::{RULES, Reader, limits, manifest, path, snapshot};

fn facts(change: Change) -> ArchitectureFacts {
    let reader = Reader::new();
    ArchitectureFacts {
        schema_version: SchemaVersion,
        source_snapshot_id: snapshot(),
        producer: Producer {
            name: "rust-analyzer".into(),
            version: "2026-10-19".into(),
            config_hash: None,
        },
        language: Language::Rust,
        project: String::new(),
        status: BackendStatus::Ready,
        base_checked: true,
        limits: Limits::default(),
        coverage: Vec::new(),
        findings: vec![ArchitectureFinding {
            check: Check::FieldLeak,
            key: "p::store::Store::items".into(),
            message: "`Store::items` is known outside `Store`: 1 place in 1 module".into(),
            notes: vec!["src/store.rs:6:1: `p::app::run`".into()],
            picture: Vec::new(),
            size: 1,
            change: Some(change),
            base_size: None,
            code_ref: reader
                .code_ref(SourceSide::Head, &path("src/store.rs"), 6, 6)
                .ok(),
        }],
        fixed: Vec::new(),
        limitations: Vec::new(),
    }
}

fn reasons(architecture: &[ArchitectureFacts]) -> Vec<(String, Priority)> {
    let manifest = manifest();
    let mut files = PolicyFiles::default();
    files
        .files
        .insert(path(".sych/rules.yaml"), RULES.as_bytes().to_vec());
    let policy = RepositoryPolicy::parse(&files, &ServerConfig::engine_default()).expect("policy");
    let plan = plan(&snapshot(), &manifest, &policy, &RunFacts::default());
    let policy_id = PolicySnapshotId(sha256(b"policy"));
    let input = BrokerInput {
        run_id: RunId(uuid::Uuid::from_u128(1)),
        snapshot_id: &snapshot(),
        policy_snapshot_id: &policy_id,
        access_scope: "local:fixture",
        plan: &plan,
        manifest: &manifest,
        facts: &[],
        syntax: &[],
        ci: &[],
        head_compiled: false,
        ledger: &EffectLedger::default(),
        lint: &[],
        architecture,
        limits: limits(),
    };
    // The first packet is the file's discovery packet.
    build_packets(&input, &Reader::new())[0]
        .fragments
        .iter()
        .flat_map(|f| {
            f.reason_selected
                .iter()
                .map(move |r| (r.clone(), f.priority))
        })
        .filter(|(r, _)| r.starts_with("architecture"))
        .collect()
}

#[test]
fn a_finding_the_change_introduced_refutes_in_its_file_and_existing_debt_stays_out() {
    let new = reasons(&[facts(Change::New)]);
    assert_eq!(new.len(), 1, "{new:?}");
    assert!(
        new[0]
            .0
            .starts_with("architecture field-leak (new): `Store::items`"),
        "{}",
        new[0].0
    );
    assert!(new[0].0.contains("p::app::run"), "the places that make it");
    // Line 6 is changed too: the span keeps the higher priority of the two.
    assert!(matches!(new[0].1, Priority::Refuting | Priority::Required));
    assert!(reasons(&[facts(Change::Worse)])[0].0.contains("(worse)"));
    assert_eq!(reasons(&[facts(Change::Same)]), Vec::new());
    assert_eq!(reasons(&[facts(Change::Better)]), Vec::new());
}
