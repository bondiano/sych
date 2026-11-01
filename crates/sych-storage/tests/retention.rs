//! Retention of records: operational rows go after 30 days, the event log and settled usage
//! after `storage.retention` (90 days by default), a dry run only counts, and unresolved charges
//! or publication outcomes stay.

use sqlx::AnyPool;
use sych_core::config::Retention;
use sych_storage::{RecordPrune, Storage};

mod support;

use support::{at, change, claim, finish, inputs, migrated};

const DAY: i64 = 86_400;

async fn count(pool: &AnyPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql)
        .fetch_one(pool)
        .await
        .expect("count selected database")
}

fn rows(plan: &[RecordPrune], table: &str) -> i64 {
    plan.iter().find(|r| r.table == table).map_or(0, |r| r.rows)
}

/// A committed run with its change events, one settled and one unknown model charge at `T0`.
async fn seeded(s: &Storage, pool: &AnyPool) {
    let id = change(s).await;
    let intent = s.open_intents().await.expect("intents")[0].id;
    s.consume_intent(intent, at(1)).await.expect("consume");
    s.generate(id, &inputs("h"), 1_800_001_000, 3, at(2))
        .await
        .expect("gen");
    let c = claim(s, "w", 3).await;
    finish(s, &c, 4).await.expect("commit");
    for state in ["SETTLED", "UNKNOWN"] {
        sqlx::query(
            "INSERT INTO model_attempts (id, tenant_id, run_id, purpose, kind, template, profile,
                 provider, requested_model, billing_kind, enforcement, day, month,
                 reserved_micro_usd, input_tokens_bound, state, created_at)
             VALUES ($1, 'local', $2, 'ANALYSIS', 'INITIAL', 't', 'p', 'x', 'm', 'api_usage',
                 'api_hard', 'd', 'm', 1, 1, $3, $4)",
        )
        .bind(format!("attempt-{state}"))
        .bind(c.run.id.to_string())
        .bind(state)
        .bind(1_800_000_000_i64)
        .execute(pool)
        .await
        .expect("attempt");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn old_records_go_unresolved_ones_stay_and_a_dry_run_deletes_nothing() {
    let (dir, s) = migrated().await;
    let pool = support::database::inspect(dir.config()).await;
    seeded(&s, &pool).await;
    let retention = Retention::default();

    let month = s
        .prune_records(at(31 * DAY), &retention, true)
        .await
        .expect("plan");
    let planned = |table: &str| rows(&month, table);
    assert_eq!(
        (
            planned("change_intents"),
            planned("inbox_events"),
            planned("jobs")
        ),
        (1, 1, 1)
    );
    assert_eq!(planned("model_attempts"), 0, "usage is kept 90 days");
    assert_eq!(
        count(&pool, "SELECT count(*) FROM jobs").await,
        1,
        "dry run deletes nothing"
    );

    s.prune_records(at(91 * DAY), &retention, false)
        .await
        .expect("prune");
    assert_eq!(count(&pool, "SELECT count(*) FROM inbox_events").await, 0);
    assert_eq!(count(&pool, "SELECT count(*) FROM change_events").await, 0);
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM model_attempts WHERE state = 'UNKNOWN'"
        )
        .await,
        1,
        "an unknown charge is never pruned by age"
    );
    assert_eq!(count(&pool, "SELECT count(*) FROM model_attempts").await, 1);
    assert_eq!(
        count(&pool, "SELECT count(*) FROM runs").await,
        1,
        "runs stay"
    );
    pool.close().await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_event_log_and_settled_usage_follow_the_configured_days() {
    let (dir, s) = migrated().await;
    let pool = support::database::inspect(dir.config()).await;
    seeded(&s, &pool).await;
    let retention = Retention {
        model_attempts_days: 10,
        change_events_days: 20,
    };

    let early = s
        .prune_records(at(9 * DAY), &retention, true)
        .await
        .expect("plan");
    assert_eq!(rows(&early, "model_attempts"), 0, "younger than 10 days");
    let usage = s
        .prune_records(at(11 * DAY), &retention, true)
        .await
        .expect("plan");
    assert_eq!(rows(&usage, "model_attempts"), 1, "only the settled charge");
    assert_eq!(rows(&usage, "change_events"), 0, "younger than 20 days");
    let events = s
        .prune_records(at(21 * DAY), &retention, true)
        .await
        .expect("plan");
    assert!(rows(&events, "change_events") > 0);
    pool.close().await;
}

#[tokio::test(flavor = "current_thread")]
async fn an_unfinished_publication_pins_its_result_past_retention() {
    use sych_core::schedule::RunInputs;
    let (_dir, s) = migrated().await;
    let id = change(&s).await;
    let publish = RunInputs {
        publish: true,
        ..inputs("h")
    };
    s.generate(id, &publish, 1_800_001_000, 3, at(0))
        .await
        .expect("gen");
    let c = claim(&s, "w", 1).await;
    finish(&s, &c, 2).await.expect("commit");
    let plan = s.prune(at(40 * DAY), true).await.expect("plan");
    assert!(
        plan.candidates.is_empty(),
        "the pending summary needs its result: {plan:?}"
    );
}
