//! Analyzer errors are context only while nothing proves the head compiles: once a CI report
//! bound to the head built it, they are artefacts and stay out of the packet.

use sych_core::effects::EffectLedger;
use sych_core::{DiagnosticFact, SnapshotReader, SourceSide};

use super::{Reader, facts, limits, manifest, path, run_built};

fn reasons(head_compiled: bool) -> Vec<String> {
    let reader = Reader::new();
    let mut facts = facts(&reader);
    let at = |severity: &str, code: &str| DiagnosticFact {
        code_ref: reader
            .code_ref(SourceSide::Head, &path("src/store.rs"), 6, 6)
            .expect("ref"),
        severity: Some(severity.into()),
        code: Some(code.into()),
        source: Some("rust-analyzer".into()),
        message: "mismatched types".into(),
    };
    facts.diagnostics = vec![at("error", "E0308"), at("warning", "unused_variables")];
    let (_, packets) = run_built(
        &manifest(),
        limits(),
        &[facts],
        &[],
        &EffectLedger::default(),
        &[],
        head_compiled,
    );
    packets
        .iter()
        .flat_map(|p| &p.fragments)
        .flat_map(|f| f.reason_selected.iter().cloned())
        .filter(|r| r.contains("diagnostic"))
        .collect()
}

#[test]
fn analyzer_errors_drop_out_once_ci_compiled_the_head() {
    let unproven = reasons(false);
    assert!(unproven.iter().any(|r| r.contains("E0308")), "{unproven:?}");

    let compiled = reasons(true);
    assert!(
        !compiled.iter().any(|r| r.contains("E0308")),
        "{compiled:?}"
    );
    assert!(
        compiled.iter().any(|r| r.contains("unused_variables")),
        "{compiled:?}"
    );
}
