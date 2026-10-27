//! CI evidence of a server run (roadmap M2.3): reports come from the trusted selector's exact
//! run, job and artifact; the checkout manifest decides `SOURCE_HEAD`, `MERGE_RESULT` or
//! `UNVERIFIED`; anything missing is a concrete gap and never a pass.

use serde_json::json;
use sych_core::{
    Coverage, Evidence, EvidenceManifest, EvidenceOrigin, ExternalInputManifest, RunId, TrustClass,
    Validity,
};

mod fake_github;
mod world;

use fake_github::{lock, stored_zip};
use world::{World, advance, git, world};

const POLICY: &str = "schema_version: 1\nci:\n  wait_seconds: 180\n  expected_reports:\n    \
    - id: tests\n      applies_to: [\"src/**\"]\n      format: junit\n";

const JUNIT: &[u8] =
    br#"<testsuites><testsuite name="s"><testcase name="saves"/><testcase name="fails"><failure/></testcase></testsuite></testsuites>"#;

/// Opens the change, lets the worker run once and returns the result's open questions with the
/// CI evidence it used.
async fn review(w: &World, head: &str) -> (Coverage, Vec<String>, Vec<(TrustClass, Validity)>) {
    w.open_pull(head, &w.base, false);
    w.pr_event("d-1", "opened", head).await;
    w.schedule(6).await;
    assert!(w.engine.work_once().await.expect("work"));
    let result = w.result(0).await;
    let run = RunId(w.run_ids().await[0].parse().expect("uuid"));
    let manifest: EvidenceManifest = w
        .app
        .storage
        .load_artifact("local", run, "evidence_manifest")
        .await
        .expect("manifest");
    let mut ci = Vec::new();
    for record in manifest
        .evidence
        .iter()
        .filter(|r| r.origin == EvidenceOrigin::ExternalInput)
    {
        let ev: Evidence = w
            .app
            .storage
            .load_artifact("local", run, &format!("evidence/{}", record.evidence_id))
            .await
            .expect("evidence");
        ci.push((ev.trust_class, ev.validity));
    }
    let gaps = result
        .unresolved_questions
        .into_iter()
        .filter(|q| q.starts_with("CI_"))
        .collect();
    (result.coverage, gaps, ci)
}

#[tokio::test(flavor = "current_thread")]
async fn a_source_head_report_is_authenticated_evidence_fetched_without_credentials() {
    let w = world(Some(POLICY)).await;
    let head = w.branch("src/b.rs");
    w.publish_ci(7, &head, "failure", Some(&head), JUNIT);
    let (_, gaps, ci) = review(&w, &head).await;
    assert!(gaps.is_empty(), "{gaps:?}");
    assert_eq!(ci, [(TrustClass::AuthenticatedCi, Validity::Valid)]);
    let log = lock(&w.fake).log.clone();
    assert!(
        log.iter().any(|l| l == "GET /blob/700 auth=false"),
        "{log:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_merge_result_binds_through_its_parents() {
    let w = world(Some(POLICY)).await;
    let head = w.branch("src/b.rs");
    git(&w.origin, &["checkout", "-q", "-B", "merge", &w.base]);
    git(
        &w.origin,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "merge",
            "-q",
            "--no-ff",
            "-m",
            "m",
            &head,
        ],
    );
    let merge = git(&w.origin, &["rev-parse", "HEAD"]);
    git(&w.origin, &["checkout", "-q", "main"]);
    w.publish_ci(7, &head, "success", Some(&merge), JUNIT);
    let (_, gaps, ci) = review(&w, &head).await;
    assert!(gaps.is_empty(), "{gaps:?}");
    assert_eq!(ci, [(TrustClass::AuthenticatedCi, Validity::Valid)]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_cargo_json_report_declared_by_policy_is_authenticated_evidence() {
    let policy = POLICY.replace("format: junit", "format: cargo_json");
    let w = world(Some(&policy)).await;
    let head = w.branch("src/b.rs");
    let clippy = br#"{"reason":"compiler-message","message":{"level":"warning","code":{"code":"clippy::unwrap_used"},"message":"used `unwrap()`","spans":[{"file_name":"src/b.rs","line_start":1,"line_end":1,"is_primary":true}]}}"#;
    w.publish_ci(7, &head, "success", Some(&head), clippy);
    let (_, gaps, ci) = review(&w, &head).await;
    assert!(gaps.is_empty(), "{gaps:?}");
    assert_eq!(ci, [(TrustClass::AuthenticatedCi, Validity::Valid)]);
}

#[tokio::test(flavor = "current_thread")]
async fn an_unproven_checkout_is_stale_and_named() {
    let w = world(Some(POLICY)).await;
    let head = w.branch("src/b.rs");
    w.publish_ci(7, &head, "success", None, JUNIT);
    let (coverage, gaps, ci) = review(&w, &head).await;
    assert_eq!(coverage, Coverage::Partial);
    assert!(
        gaps[0].starts_with("CI_BINDING_UNVERIFIED for report `tests`"),
        "{gaps:?}"
    );
    assert_eq!(ci, [(TrustClass::AuthenticatedCi, Validity::Stale)]);
}

#[tokio::test(flavor = "current_thread")]
async fn skipped_cancelled_expired_malformed_and_hostile_reports_are_gaps() {
    type Break = fn(&World, &str);
    let cases: [(&str, Break); 5] = [
        ("CI_JOB_SKIPPED", |w, h| {
            w.publish_ci(7, h, "skipped", Some(h), JUNIT);
        }),
        ("CI_JOB_CANCELLED", |w, h| {
            w.publish_ci(7, h, "cancelled", Some(h), JUNIT);
        }),
        ("CI_ARTIFACT_EXPIRED", |w, h| {
            w.publish_ci(7, h, "success", Some(h), JUNIT);
            lock(&w.fake).artifacts.get_mut(&7).expect("artifacts")[0]["expired"] = json!(true);
        }),
        ("CI_REPORT_MALFORMED", |w, h| {
            w.publish_ci(7, h, "success", Some(h), b"<testsuites><testcase");
        }),
        ("CI_ARCHIVE_REJECTED", |w, h| {
            w.publish_ci(7, h, "success", Some(h), JUNIT);
            let zip = stored_zip(&[("../junit.xml", JUNIT), ("junit.xml", JUNIT)]);
            lock(&w.fake).blobs.insert(700, zip);
        }),
    ];
    for (code, setup) in cases {
        let w = world(Some(POLICY)).await;
        let head = w.branch("src/b.rs");
        setup(&w, &head);
        let (coverage, gaps, ci) = review(&w, &head).await;
        assert_eq!(coverage, Coverage::Partial, "{code}");
        assert!(gaps.iter().any(|g| g.starts_with(code)), "{code}: {gaps:?}");
        assert!(ci.is_empty(), "{code}: no evidence from a gap");
    }
}

/// The run's derived evidence ids with their trust classes, its external inputs and the
/// manifest as stored.
async fn derived(
    w: &World,
    run: &str,
) -> (Vec<(String, TrustClass)>, Vec<String>, serde_json::Value) {
    let run = RunId(run.parse().expect("uuid"));
    let load = |role: &'static str| {
        w.app
            .storage
            .load_artifact::<serde_json::Value>("local", run, role)
    };
    let stored = load("evidence_manifest").await.expect("manifest");
    let manifest: EvidenceManifest = serde_json::from_value(stored.clone()).expect("manifest");
    let mut out = Vec::new();
    for r in manifest
        .evidence
        .iter()
        .filter(|r| r.origin == EvidenceOrigin::Derived)
    {
        let ev: Evidence = w
            .app
            .storage
            .load_artifact("local", run, &format!("evidence/{}", r.evidence_id))
            .await
            .expect("evidence");
        out.push((r.evidence_id.to_string(), ev.trust_class));
    }
    let external: ExternalInputManifest =
        serde_json::from_value(load("external_input_manifest").await.expect("inputs"))
            .expect("inputs");
    let inputs = external
        .inputs
        .iter()
        .map(|i| i.evidence_id.to_string())
        .collect();
    (out, inputs, stored)
}

#[tokio::test(flavor = "current_thread")]
async fn a_late_ci_successor_derives_its_own_evidence_and_leaves_the_parent_frozen() {
    let w = world(Some(POLICY)).await;
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    w.pr_event("d-1", "opened", &head).await;
    w.schedule(6).await;
    w.engine.work_once().await.expect("work");
    advance(181);
    w.engine.work_once().await.expect("work");
    let parent = w.run_ids().await[0].clone();
    let (parent_derived, parent_inputs, parent_manifest) = derived(&w, &parent).await;
    assert!(
        !parent_derived.is_empty(),
        "syntax facts are derived evidence"
    );
    assert!(
        parent_derived
            .iter()
            .all(|(id, trust)| *trust == TrustClass::DerivedLocal && !parent_inputs.contains(id)),
        "{parent_derived:?}"
    );

    w.publish_ci(7, &head, "success", Some(&head), JUNIT);
    w.ci("c-1", &head).await;
    w.schedule(11).await;
    assert!(w.engine.work_once().await.expect("work"));
    let runs = w.run_ids().await;
    assert_eq!(runs.len(), 2, "one successor");
    let (successor, successor_inputs, _) = derived(&w, &runs[1]).await;
    assert!(!successor.is_empty());
    assert!(
        successor
            .iter()
            .all(|(id, trust)| *trust == TrustClass::DerivedLocal && !successor_inputs.contains(id))
    );
    assert!(
        !successor_inputs.is_empty(),
        "the late report is an input of the successor only"
    );
    assert!(parent_inputs.is_empty());
    assert_eq!(
        derived(&w, &parent).await.2,
        parent_manifest,
        "the parent's frozen manifest does not change"
    );
}
