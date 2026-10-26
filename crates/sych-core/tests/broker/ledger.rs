//! Effect ledger on packet spans: a changed effect refutes, an unresolved one and a registry
//! guarantee whose implementation changed are gaps no obligation is SUPPORTED over.

use sych_core::broker::{Priority, context_limited};
use sych_core::config::EffectKind;
use sych_core::effects::{
    BindingStatus, EffectLedger, EffectMatch, ImplementationRef, LedgerEntry, Occurrence,
    RegistryBinding, Resolution,
};
use sych_core::{
    Assessment, Language, ObligationStatus, SchemaVersion, SnapshotReader, SourceSide, SyntaxKind,
};

use super::{Reader, limits, path, run_full, snapshot};

fn ledger(resolution: Resolution, status: BindingStatus) -> EffectLedger {
    let reader = Reader::new();
    let occurrence = Occurrence {
        code_ref: reader
            .code_ref(SourceSide::Head, &path("src/store.rs"), 6, 7)
            .expect("ref"),
        symbol: Some("Store.save".into()),
        payload: "append_event(order)".into(),
        guard: None,
        transaction_scope: "transaction (Tx)".into(),
        lifetime_scope: "caller".into(),
        failure_behavior: "error returned to the caller".into(),
        in_change: true,
    };
    EffectLedger {
        schema_version: SchemaVersion,
        source_snapshot_id: Some(snapshot()),
        entries: vec![LedgerEntry {
            effect_id: "event:append_event@src/store.rs#Store.save".into(),
            kind: EffectKind::Event,
            target: "append_event".into(),
            shape: SyntaxKind::RegisteredEffectCall,
            language: Language::Rust,
            path: path("src/store.rs"),
            symbol: Some("Store.save".into()),
            registry_id: Some("order-outbox".into()),
            durable_with_transaction: None,
            contract_refs: Vec::new(),
            base: Vec::new(),
            head: vec![occurrence],
            matched: EffectMatch::Added,
            resolution,
            notes: Vec::new(),
        }],
        registry: vec![RegistryBinding {
            registry_id: "order-outbox".into(),
            symbol: "Store.append_event".into(),
            kind: EffectKind::Event,
            durable_with_transaction: Some(true),
            implementation: vec![ImplementationRef {
                path: path("src/store.rs"),
                base_oid: None,
                head_oid: None,
                changed: status != BindingStatus::Current,
            }],
            contract_refs: Vec::new(),
            status,
        }],
        dependencies: Vec::new(),
        limitations: Vec::new(),
    }
}

fn supported_becomes(ledger: &EffectLedger) -> ObligationStatus {
    let (plan, packets) = run_full(limits(), &[], &[], ledger, &[]);
    let assessment = Assessment {
        rule_id: "EFFECT-01".into(),
        status: ObligationStatus::Supported,
        evidence_ids: Vec::new(),
        limitations: Vec::new(),
    };
    context_limited(&plan, &packets, vec![assessment])
        .remove(0)
        .status
}

#[test]
fn resolved_current_effect_supports_without_a_gap() {
    let ledger = ledger(Resolution::Resolved, BindingStatus::Current);
    let (_, packets) = run_full(limits(), &[], &[], &ledger, &[]);
    let fragment = packets[0]
        .fragments
        .iter()
        .find(|f| {
            f.reason_selected
                .iter()
                .any(|r| r.starts_with("effect ledger added event `append_event`"))
        })
        .expect("ledger span");
    assert_eq!(
        fragment.evidence_ids,
        vec![ledger.evidence_id().expect("id")]
    );
    assert!(packets[0].gaps.iter().all(|g| !g.starts_with("EFFECT_")));
    assert_eq!(supported_becomes(&ledger), ObligationStatus::Supported);
}

#[test]
fn unresolved_effect_and_changed_registry_keep_the_obligation_unknown() {
    for ledger in [
        ledger(Resolution::DynamicUnknown, BindingStatus::Current),
        ledger(Resolution::Resolved, BindingStatus::ImplementationChanged),
    ] {
        assert!(matches!(
            supported_becomes(&ledger),
            ObligationStatus::Unknown { reason_code, .. } if reason_code == "SEMANTIC_GAP"
        ));
    }
    let (_, packets) = run_full(
        limits(),
        &[],
        &[],
        &ledger(Resolution::Resolved, BindingStatus::ImplementationChanged),
        &[],
    );
    assert!(
        packets[0]
            .gaps
            .iter()
            .any(|g| g.starts_with("REGISTRY_RECHECK: registry `order-outbox`"))
    );
    let mut removed = ledger(Resolution::Partial, BindingStatus::Current);
    removed.entries[0].matched = EffectMatch::Removed;
    let (_, packets) = run_full(limits(), &[], &[], &removed, &[]);
    assert!(
        packets[0]
            .fragments
            .iter()
            .any(|f| f.priority <= Priority::Refuting
                && f.reason_selected.iter().any(|r| r.contains("removed")))
    );
}
