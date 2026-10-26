//! Syntax facts on packet spans: import blocks and contract shapes of changed lines, and
//! changed code the snapshot does not show, which no obligation can be SUPPORTED over.

use std::collections::BTreeMap;

use sych_core::broker::{Priority, context_gaps, context_limited};
use sych_core::{
    Assessment, Language, ObligationStatus, Producer, SchemaVersion, SnapshotReader, SourceSide,
    SyntaxFact, SyntaxFacts, SyntaxKind,
};

use super::{Reader, limits, path, run_with, snapshot};

#[test]
fn adjacent_imports_of_the_changed_file_are_one_supporting_span() {
    let reader = Reader::new();
    let import = |line| SyntaxFact {
        kind: SyntaxKind::Import,
        code_ref: reader
            .code_ref(SourceSide::Head, &path("src/store.rs"), line, line)
            .expect("ref"),
        symbol: None,
        detail: "crate::tx::Tx".into(),
        in_change: false,
        related: Vec::new(),
        subject: None,
        guard: None,
    };
    let syntax = SyntaxFacts {
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
        facts: vec![import(1), import(2)],
        limitations: Vec::new(),
    };
    let (_, packets) = run_with(limits(), &[], &[syntax]);
    let imports: Vec<_> = packets[0]
        .fragments
        .iter()
        .filter(|f| {
            f.reason_selected
                .iter()
                .any(|r| r == "imports (2 statements)")
        })
        .collect();
    assert_eq!(imports.len(), 1, "{:?}", packets[0].fragments);
    assert_eq!(imports[0].code_ref.start_line, 1);
}

#[test]
fn a_changed_contract_shape_is_named_on_its_span() {
    let reader = Reader::new();
    let syntax = SyntaxFacts {
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
            kind: SyntaxKind::SerdeAttribute,
            code_ref: reader
                .code_ref(SourceSide::Head, &path("src/store.rs"), 6, 6)
                .expect("ref"),
            symbol: None,
            detail: "#[serde(rename_all = \"camelCase\")]".into(),
            in_change: true,
            related: Vec::new(),
            subject: None,
            guard: None,
        }],
        limitations: Vec::new(),
    };
    let (_, packets) = run_with(limits(), &[], &[syntax]);
    let changed = packets[0]
        .fragments
        .iter()
        .find(|f| f.code_ref.start_line <= 6 && 6 <= f.code_ref.end_line)
        .expect("changed span");
    assert!(
        changed
            .reason_selected
            .iter()
            .any(|r| r.starts_with("syntax serde_attribute: ")),
        "{:?}",
        changed.reason_selected
    );
    assert_eq!(changed.priority, Priority::Required);
}

#[test]
fn an_unexpanded_macro_in_the_change_keeps_the_obligation_unknown() {
    let reader = Reader::new();
    let syntax = SyntaxFacts {
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
            kind: SyntaxKind::UnexpandedMacro,
            code_ref: reader
                .code_ref(SourceSide::Head, &path("src/store.rs"), 6, 6)
                .expect("ref"),
            symbol: None,
            detail: "log_err! is not expanded".into(),
            in_change: true,
            related: Vec::new(),
            subject: None,
            guard: None,
        }],
        limitations: Vec::new(),
    };
    let (plan, packets) = run_with(limits(), &[], &[syntax]);
    assert!(
        packets.iter().all(|p| p
            .gaps
            .iter()
            .any(|g| g.starts_with("MACRO_UNEXPANDED: src/store.rs:6-6"))),
        "{:?}",
        packets.iter().map(|p| &p.gaps).collect::<Vec<_>>()
    );
    let assessment = |status| Assessment {
        rule_id: "EFFECT-01".into(),
        status,
        evidence_ids: Vec::new(),
        limitations: Vec::new(),
    };
    let supported = context_limited(
        &plan,
        &packets,
        vec![assessment(ObligationStatus::Supported)],
    );
    assert!(matches!(
        &supported[0].status,
        ObligationStatus::Unknown { reason_code, .. } if reason_code == "SEMANTIC_GAP"
    ));
    assert!(supported[0].limitations[0].starts_with("MACRO_UNEXPANDED"));
    let violated = context_limited(
        &plan,
        &packets,
        vec![assessment(ObligationStatus::Violated)],
    );
    assert_eq!(
        violated[0].status,
        ObligationStatus::Violated,
        "a violation the change shows stands"
    );
    assert_eq!(
        context_gaps(&packets).len(),
        1,
        "one gap however many packets show it"
    );
}
