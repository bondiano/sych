//! Guided tour (roadmap M5.1): a deterministic plan that never hides a critical finding or a
//! required UNKNOWN, structural validation against the result and the snapshot, one repair,
//! and staleness without mutation.

use sych_core::tour::{
    ExampleSnippet, MAX_ATTEMPTS, RENDERER_VERSION, SessionState, TourArtifact, TourEvent,
    TourSession, TourState, TourStep, next, plan, render, stale, validate,
};
use sych_core::{
    CodeRef, DiffManifest, FindingCategory, FindingId, GitHashAlgorithm, GitObjectId, ReadError,
    RepoPath, ReviewResult, SchemaVersion, Severity, SnapshotReader, SourceSide, SourceSnapshotId,
    SourceSpan, TourId, sha256,
};

const SOURCE: &str = "package orders\n\nfunc Create() error {\n\treturn nil\n}\n";

fn result() -> ReviewResult {
    let text = include_str!("../../../fixtures/review/violated.bundle.json");
    let bundle: serde_json::Value = serde_json::from_str(text).expect("bundle");
    serde_json::from_value(bundle["review_result"].clone()).expect("result")
}

fn code_ref(snapshot: &SourceSnapshotId, path: &str, start: u32, end: u32) -> CodeRef {
    CodeRef {
        snapshot_id: snapshot.clone(),
        side: SourceSide::Head,
        path: path.parse().expect("path"),
        blob_oid: GitObjectId::new(GitHashAlgorithm::Sha1, &"b".repeat(40)).expect("oid"),
        start_line: start,
        end_line: end,
        content_sha256: sha256(b"span"),
    }
}

/// One file of the head; anything else does not resolve.
struct Reader(SourceSnapshotId);

impl SnapshotReader for Reader {
    fn read_span(&self, r: &CodeRef) -> Result<SourceSpan, ReadError> {
        let text = sych_core::line_range(SOURCE, r.start_line, r.end_line)
            .filter(|_| r.snapshot_id == self.0 && r.path.as_str() == "backend/orders/create.go")
            .ok_or_else(|| ReadError::UnknownSnapshot(r.path.to_string()))?;
        Ok(SourceSpan {
            reference: r.clone(),
            text: text.to_owned(),
        })
    }

    fn code_ref(
        &self,
        _: SourceSide,
        path: &RepoPath,
        start: u32,
        end: u32,
    ) -> Result<CodeRef, ReadError> {
        Ok(code_ref(&self.0, path.as_str(), start, end))
    }

    fn search(
        &self,
        _: SourceSide,
        _: &str,
        _: &[RepoPath],
        _: usize,
    ) -> Result<Vec<CodeRef>, ReadError> {
        Ok(Vec::new())
    }

    fn diff(&self, s: &SourceSnapshotId) -> Result<DiffManifest, ReadError> {
        Err(ReadError::UnknownSnapshot(s.0.to_string()))
    }
}

/// Eight HIGH findings in eight files and two nits.
fn crowded() -> ReviewResult {
    let mut r = result();
    let base = r.findings[0].clone();
    r.findings = (0..10)
        .map(|i| {
            let mut f = base.clone();
            f.id = FindingId(uuid::Uuid::from_u128(i + 1));
            f.anchors = vec![code_ref(
                &r.source_snapshot_id,
                &format!("src/f{i}.go"),
                1,
                1,
            )];
            if i >= 8 {
                f.category = FindingCategory::Nit;
                f.severity = Severity::Low;
            }

            f
        })
        .collect();
    r
}

#[test]
fn the_route_is_bounded_but_the_overview_keeps_every_critical_finding() {
    let r = crowded();
    let p = plan(&r, 5);
    assert_eq!(p.stops.len(), 5);
    assert_eq!(
        p.overview.critical_findings.len(),
        8,
        "route length cuts nothing"
    );
    assert_eq!(p.improvements.len(), 2);
    assert_eq!(p.stops[0].id, "stop-1");
    assert_eq!(plan(&r, 20).stops.len(), 6);
}

fn tour(r: &ReviewResult) -> TourArtifact {
    let p = plan(r, 5);
    TourArtifact {
        schema_version: SchemaVersion,
        tour_id: TourId::new_v7(),
        result_id: r.result_id,
        review_revision: r.review_revision,
        source_snapshot_id: r.source_snapshot_id.clone(),
        policy_snapshot_id: r.policy_snapshot_id.clone(),
        language: r.language,
        planner_version: p.planner_version,
        prompt_version: "tour@1".into(),
        renderer_version: RENDERER_VERSION.into(),
        generation_provenance: "fake, 1 attempt".into(),
        overview: p.overview,
        steps: vec![TourStep {
            id: "stop-1".into(),
            title: "Order creation".into(),
            purpose: "Where the event is lost".into(),
            finding_ids: vec![r.findings[0].id],
            obligation_ids: r.findings[0].obligation_ids.clone(),
            code_refs: vec![code_ref(
                &r.source_snapshot_id,
                "backend/orders/create.go",
                3,
                4,
            )],
            explanation: "The event is no longer written in the transaction.".into(),
            question: None,
            example_snippets: Vec::new(),
        }],
        limitations: Vec::new(),
    }
}

#[test]
fn a_valid_tour_renders_lines_from_the_blob() {
    let r = result();
    let reader = Reader(r.source_snapshot_id.clone());
    let t = tour(&r);
    assert!(validate(&t, &r, &reader).is_empty());
    let shown = render(&t.steps[0], &reader);
    assert_eq!(
        shown[0].1.as_deref(),
        Some("func Create() error {\n\treturn nil\n")
    );
    // A pruned or GC'd source keeps the location and says it is unavailable.
    let gone = Reader(SourceSnapshotId(sha256(b"other")));
    assert_eq!(render(&t.steps[0], &gone)[0].1, None);
}

#[test]
fn a_session_finishes_once_and_only_records_progress() {
    let s = TourSession::start(TourId::new_v7(), "dev")
        .reveal("stop-1")
        .reveal("stop-1");
    assert_eq!(s.revealed, ["stop-1"]);
    let done = s.clone().finish(SessionState::Completed).expect("complete");
    assert!(done.clone().finish(SessionState::Dismissed).is_err());
    assert!(done.clone().finish(SessionState::InProgress).is_err());
    assert_eq!(
        done.clone().reveal("stop-2").revealed,
        ["stop-1"],
        "a finished session only reads"
    );
    assert_eq!(
        s.finish(SessionState::Dismissed).expect("dismiss").state,
        SessionState::Dismissed
    );
}

#[test]
fn fake_refs_unknown_ids_hidden_unknowns_and_unlabelled_examples_are_rejected() {
    let r = result();
    let reader = Reader(r.source_snapshot_id.clone());
    let mut t = tour(&r);
    t.steps[0]
        .code_refs
        .push(code_ref(&r.source_snapshot_id, "invented.go", 1, 2));
    t.steps[0]
        .finding_ids
        .push(FindingId(uuid::Uuid::from_u128(99)));
    t.steps[0].example_snippets.push(ExampleSnippet {
        label: "Fix".into(),
        text: "publish()".into(),
    });
    t.overview.critical_findings.clear();
    let problems = validate(&t, &r, &reader);
    assert_eq!(problems.len(), 4, "{problems:?}");
}

#[test]
fn one_repair_then_failure_and_a_new_revision_is_stale() {
    let mut s = next(&TourState::NotRequested, TourEvent::Requested);
    for _ in 0..MAX_ATTEMPTS {
        s = next(&next(&s, TourEvent::Generated), TourEvent::Invalid);
    }
    assert!(matches!(s, TourState::Failed { .. }), "{s:?}");
    let ready = next(
        &next(
            &next(&TourState::NotRequested, TourEvent::Requested),
            TourEvent::Generated,
        ),
        TourEvent::Valid,
    );
    assert_eq!(ready, TourState::Ready);

    let r = result();
    let t = tour(&r);
    assert!(!stale(&t, r.review_revision));
    assert!(stale(&t, r.review_revision + 1));
}

/// Two findings on the same lines make one stop that shows those lines once.
#[test]
fn a_stop_shows_each_location_once() {
    let mut r = crowded();
    r.findings.truncate(2);
    let anchor = r.findings[0].anchors.clone();
    r.findings[1].anchors.clone_from(&anchor);
    let p = plan(&r, 5);
    assert_eq!(p.stops[0].code_refs, anchor);
}
