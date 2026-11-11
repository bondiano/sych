//! Pinned linter findings on packet spans: changed lines refute, the rest stays out, and a
//! linter that could not run is a gap.

use sych_core::broker::Priority;
use sych_core::effects::EffectLedger;
use sych_core::{
    BackendStatus, CiDiagnostic, LintFacts, Producer, SchemaVersion, SnapshotReader, SourceSide,
};

use super::{Reader, limits, path, run_full, snapshot};

fn ruff(status: BackendStatus) -> LintFacts {
    let reader = Reader::new();
    let at = |line| CiDiagnostic {
        code_ref: reader
            .code_ref(SourceSide::Head, &path("src/store.rs"), line, line)
            .ok(),
        path: "src/store.rs".into(),
        line,
        level: "error".into(),
        code: Some("F821".into()),
        message: format!("undefined name at {line}"),
    };
    LintFacts {
        schema_version: SchemaVersion,
        source_snapshot_id: snapshot(),
        side: SourceSide::Head,
        producer: Producer {
            name: "ruff".into(),
            version: "0.17.2".into(),
            config_hash: None,
        },
        status,
        files: vec!["src/store.rs".into()],
        config: Vec::new(),
        diagnostics: vec![at(6), at(1)],
        introduced: Vec::new(),
        limitations: vec!["download failed: offline".into()],
    }
}

#[test]
fn findings_on_changed_lines_refute_and_a_missing_linter_is_a_gap() {
    let (_, packets) = run_full(
        limits(),
        &[],
        &[],
        &EffectLedger::default(),
        &[ruff(BackendStatus::Ready)],
    );
    let reasons: Vec<(&String, Priority)> = packets[0]
        .fragments
        .iter()
        .flat_map(|f| f.reason_selected.iter().map(move |r| (r, f.priority)))
        .filter(|(r, _)| r.starts_with("lint ruff"))
        .collect();
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    assert!(reasons[0].0.contains("F821: undefined name at 6"));

    let (_, packets) = run_full(
        limits(),
        &[],
        &[],
        &EffectLedger::default(),
        &[ruff(BackendStatus::Failed)],
    );
    assert!(
        packets[0]
            .gaps
            .iter()
            .any(|g| g == "LINT_UNAVAILABLE: ruff for src/store.rs: download failed: offline")
    );
}

#[test]
fn a_finding_new_on_head_enters_the_packet_of_its_file_at_an_unchanged_line() {
    let mut facts = ruff(BackendStatus::Ready);
    facts.introduced = vec![facts.diagnostics[1].clone()];
    let mut elsewhere = facts.diagnostics[1].clone();
    elsewhere.path = "src/other.rs".into();
    if let Some(r) = elsewhere.code_ref.as_mut() {
        r.path = path("src/other.rs");
    }

    facts.introduced.push(elsewhere);
    let (_, packets) = run_full(limits(), &[], &[], &EffectLedger::default(), &[facts]);
    let reasons: Vec<&String> = packets[0]
        .fragments
        .iter()
        .flat_map(|f| &f.reason_selected)
        .filter(|r| r.starts_with("lint ruff"))
        .collect();
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("F821 (new on head): undefined name at 1")),
        "{reasons:?}"
    );
    assert!(!reasons.iter().any(|r| r.contains("other")), "{reasons:?}");
}
