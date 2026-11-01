//! Generated files of a change: never shown to a model nor read by its context requests, but
//! named to it, so a regenerated file is not reported as left stale. A file only the change's
//! own marker calls generated stays hidden, yet keeps coverage partial.

use sych_core::analysis::Context;
use sych_core::broker::{OpStatus, Operation, Ops};
use sych_core::effects::EffectLedger;
use sych_core::{
    Coverage, DEFAULT_ATTENTION_THRESHOLD, ExecutionOutcome, GENERATED_IN_CHANGE, ReduceInput,
    ReviewLanguage, SnapshotReader, SourceSide, reduce,
};

use super::{Reader, limits, manifest, path, run_on};

#[test]
fn a_file_the_change_marks_generated_is_hidden_yet_keeps_coverage_partial() {
    let mut claimed = manifest();
    claimed.files[0].generated_claim = true;
    let (plan, packets) = run_on(&claimed, limits(), &[], &[], &EffectLedger::default(), &[]);
    assert!(
        plan.scope.excluded.is_empty(),
        "a claim is not a trusted exclusion"
    );
    assert!(
        plan.unreviewed
            .iter()
            .any(|g| g.path == path("src/store.rs") && g.reason == GENERATED_IN_CHANGE)
    );
    assert!(
        packets
            .iter()
            .flat_map(|p| &p.fragments)
            .all(|f| f.code_ref.path != path("src/store.rs")),
        "the models never see it"
    );
    let mut bare = plan;
    bare.obligations.clear();
    let reduced = reduce(&ReduceInput {
        plan: &bare,
        assessments: &[],
        evidence: &[],
        findings: &[],
        candidates: &[],
        task_gaps: &[],
        questions: &[],
        outcome: ExecutionOutcome::PlanExecuted,
        attention_threshold: DEFAULT_ATTENTION_THRESHOLD,
        now: "2026-10-25T00:00:00Z".parse().expect("timestamp"),
    });
    assert_eq!(reduced.coverage, Coverage::Partial);
}

#[test]
fn context_requests_neither_read_nor_find_generated_files() {
    let reader = Reader::new();
    let generated = [path("src/store.rs")];
    let ops = Ops {
        generated: &generated,
        facts: &[],
        evidence: &[],
        reader: &reader,
    };
    let code_ref = reader
        .code_ref(SourceSide::Head, &path("src/store.rs"), 1, 1)
        .expect("ref");
    let read = ops.run(&Operation::ReadSpan { code_ref });
    assert_eq!((read.status, read.text), (OpStatus::Unsupported, None));
    let hits = ops.run(&Operation::SearchSource {
        query: "Store".into(),
        paths: Vec::new(),
        side: SourceSide::Head,
        limit: 10,
    });
    assert!(!hits.refs.is_empty());
    assert!(hits.refs.iter().all(|r| r.path != path("src/store.rs")));
    assert!(
        hits.limitations
            .iter()
            .any(|l| l.contains("in generated files are not shown")),
        "{:?}",
        hits.limitations
    );
    let cut = ops.run(&Operation::SearchSource {
        query: "Store".into(),
        paths: Vec::new(),
        side: SourceSide::Head,
        limit: 1,
    });
    assert_eq!(
        cut.status,
        OpStatus::Partial,
        "dropping a generated hit never makes a cut answer whole"
    );
}

#[test]
fn a_secret_path_outranks_a_claim_of_being_generated() {
    let mut claimed = manifest();
    claimed.files[0].generated_claim = true;
    claimed.files[0].limitations.push("SECRET_PATH".into());
    let (plan, _) = run_on(&claimed, limits(), &[], &[], &EffectLedger::default(), &[]);
    assert_eq!(plan.unreviewed[0].reason, "SECRET_PATH");
}

#[test]
fn a_prompt_names_the_generated_files_the_change_touches_without_their_content() {
    let reader = Reader::new();
    let mut changed = manifest();
    changed.files[0].generated = true;
    let mut context = Context::new(ReviewLanguage::En);
    let shown = reader
        .code_ref(SourceSide::Head, &path("src/api.rs"), 1, 2)
        .expect("ref");
    context.add_span(&shown, "required", &reader);
    context.add_changes(&changed);
    let block = context
        .blocks
        .iter()
        .find(|b| {
            b.label
                .starts_with("generated files this change also changes")
        })
        .expect("the generated files are named");
    assert!(block.content.contains("src/store.rs Added"));
    assert!(
        context
            .blocks
            .iter()
            .all(|b| !b.content.contains("pub struct Store")),
        "its content stays out"
    );
}
