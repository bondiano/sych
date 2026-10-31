//! Checkpoints under a run's fence: kept while the lease is current; once a write
//! shows the run is no longer worked under it, nothing more is kept and no further model call
//! is reserved, because its result could never be committed.

use sych_core::config::{BillingKind, Budget, Enforcement, Role};
use sych_core::model::{
    AttemptKind, Capabilities, Checkpoint, CheckpointDeps, Ledger, Limits, Purpose, Reservation,
    Route,
};
use sych_core::{AttemptId, MicroUsd, RunId, SchemaVersion, sha256};
use sych_storage::{CHECKPOINT_ROLE, RunLedger, TENANT_LOCAL};

mod support;

use support::{T0, at, change, claim, inputs, migrated};

fn checkpoint(run: RunId, template: &str) -> Checkpoint {
    let deps = CheckpointDeps {
        template: template.into(),
        schema_name: "discovery".into(),
        role: Role::Reviewer,
        chain: vec!["cheap".into()],
        messages_sha256: sha256(b"messages"),
        schema_sha256: sha256(b"schema"),
        models_revision: "models-1".into(),
    };
    Checkpoint {
        schema_version: SchemaVersion,
        key: deps.key().expect("key"),
        deps,
        profile: "cheap".into(),
        run_id: run,
        output: serde_json::json!({"candidates": []}),
    }
}

fn reservation(run: RunId) -> Reservation {
    Reservation {
        attempt_id: AttemptId::new_v7(),
        tenant_id: TENANT_LOCAL.into(),
        run_id: run,
        purpose: Purpose::Analysis,
        kind: AttemptKind::Initial,
        template: "discovery@1".into(),
        route: Route {
            profile: "cheap".into(),
            provider: "openrouter".into(),
            adapter: "test".into(),
            model: "m".into(),
            billing: BillingKind::ApiUsage,
            enforcement: Enforcement::ApiHard,
            capabilities: Capabilities {
                structured_output: true,
                context_tokens: None,
                output_cap_enforced: true,
                usage_reported: true,
                cost_reported: true,
                hidden_calls: false,
            },
        },
        money: MicroUsd(10),
        input_tokens: 10,
        output_tokens: Some(10),
        fixed_fee: None,
        at: at(2),
    }
}

fn limits() -> Limits {
    Limits {
        run: Budget {
            max_cost_micro_usd: Some(MicroUsd(1_000)),
            ..Budget::default()
        },
        daily_money: Some(MicroUsd(1_000)),
        monthly_money: Some(MicroUsd(1_000)),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_superseded_run_keeps_no_more_checkpoints_and_reserves_no_more_calls() {
    let (_dir, s) = migrated().await;
    let id = change(&s).await;
    s.generate(id, &inputs("h1"), T0 + 300, 2, at(0))
        .await
        .expect("gen");
    let old = claim(&s, "w1", 1).await;
    let ledger = RunLedger::new(&s, &old.run, || at(2));
    let run = old.run.id;
    ledger
        .checkpoint(&checkpoint(run, "discovery@1"))
        .await
        .expect("kept under the current lease");
    assert!(matches!(
        ledger.reserve(&reservation(run), &limits()).await,
        Ok(Ok(()))
    ));

    s.generate(id, &inputs("h2"), T0 + 300, 2, at(3))
        .await
        .expect("a push supersedes the run");
    let lost = ledger.checkpoint(&checkpoint(run, "verify@1")).await;
    assert!(lost.is_err(), "{lost:?}");
    let refused = ledger.reserve(&reservation(run), &limits()).await;
    assert!(
        refused.is_err(),
        "no paid call for a result that cannot commit"
    );

    let kept: Vec<Checkpoint> = s
        .load_artifacts(&old.run.partition, run, CHECKPOINT_ROLE)
        .await
        .expect("load");
    assert_eq!(kept, [checkpoint(run, "discovery@1")]);
}
