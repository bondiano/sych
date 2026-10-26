//! A late CI report on a real model route (roadmap M2.2): the parent pins a checkpoint for
//! every accepted answer as it arrives, and its successor answers the tasks whose input the
//! report did not change from them, without a model call, and links the parent's result.
//! A changed input asking again is covered on the engine (`checkpoints.rs`).

use serde_json::Value;
use sych_core::model::Checkpoint;
use sych_storage::CHECKPOINT_ROLE;

mod fake_github;
mod fake_model;
mod world;

use world::{advance, world_with};

const JUNIT: &[u8] = br#"<testsuites><testsuite name="s"><testcase name="t" file="src/b.rs"/></testsuite></testsuites>"#;

const CI_POLICY: &str = "schema_version: 1\nci:\n  wait_seconds: 180\n  expected_reports:\n    \
    - id: tests\n      applies_to: [\"src/**\"]\n      format: junit\n";

#[tokio::test(flavor = "current_thread")]
async fn a_successor_reuses_its_parents_checkpoints_through_the_server() {
    let model = fake_model::start();
    let w = world_with(Some(CI_POLICY), Some(&model.endpoint)).await;
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    w.pr_event("d-1", "opened", &head).await;
    w.schedule(6).await;
    w.engine.work_once().await.expect("work");
    advance(181);
    w.engine.work_once().await.expect("work");
    assert_eq!(w.states().await, ["COMPLETED"]);
    let parent_calls = model.calls();
    // No rules: one discovery call over the changed unit.
    assert_eq!(parent_calls, ["discovery"]);
    let parent = w.result(0).await;
    let partition = w.app.storage.change(1).await.expect("change").partition;
    let kept: Vec<Checkpoint> = w
        .app
        .storage
        .load_artifacts(&partition, parent.run_id, CHECKPOINT_ROLE)
        .await
        .expect("checkpoints");
    assert_eq!(
        kept.len(),
        parent_calls.len(),
        "one checkpoint per accepted answer"
    );

    w.publish_ci(555, &head, "success", Some(&head), JUNIT);
    w.ci("c-1", &head).await;
    w.schedule(11).await;
    assert!(w.engine.work_once().await.expect("work"));
    assert_eq!(w.states().await, ["COMPLETED", "COMPLETED"]);
    let successor = w.result(1).await;
    assert_eq!(successor.parent_result_id, Some(parent.result_id));
    let analysis: Value = w
        .app
        .storage
        .load_artifact("local", successor.run_id, "analysis")
        .await
        .expect("analysis");
    assert!(
        analysis["plan"]["revision"]
            .as_str()
            .is_some_and(|r| !r.is_empty())
    );
    assert_eq!(
        analysis["routes"][0]["model"],
        "deepseek/deepseek-v4.1-flash"
    );
    assert_eq!(analysis["routes"][0]["reasoning_effort"], "low");
    assert_eq!(
        analysis["routes"][0]["upstreams"],
        serde_json::json!(["deepseek"])
    );
    let passes = analysis["passes"].as_array().expect("passes");
    // The report is not shown to discovery, so its input is unchanged: no new call.
    assert_eq!(model.calls(), parent_calls);
    assert_eq!(passes.len(), 1);
    assert_eq!(passes[0]["reused_from"], parent.run_id.to_string());
    assert_eq!(passes[0]["attempt_ids"], serde_json::json!([]));
    assert_eq!(analysis["hops"][0]["outcome"], "REUSED", "{analysis}");
    assert!(passes.iter().all(|p| p.get("checkpoint_error").is_none()));
    let carried: Vec<Checkpoint> = w
        .app
        .storage
        .load_artifacts(&partition, successor.run_id, CHECKPOINT_ROLE)
        .await
        .expect("checkpoints");
    assert_eq!(
        carried, kept,
        "kept again for the successor's own successors"
    );
}
