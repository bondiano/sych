//! The server applies `storage.retention` itself: past the window the change event log goes,
//! the run and its findings stay.

mod fake_github;
mod http;
mod world;

use world::database::inspect;
use world::{advance, commit_result, world};

const DAY: i64 = 86_400;

async fn count(pool: &sqlx::AnyPool, table: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(pool)
        .await
        .expect("count")
}

#[tokio::test(flavor = "current_thread")]
async fn a_retention_pass_keeps_recent_history_and_drops_what_is_past_the_window() {
    let w = world(None).await;
    commit_result(&w, 1).await;
    w.engine.lineage_once().await.expect("lineage");
    let pool = inspect(w.dir.config()).await;
    let events = count(&pool, "change_events").await;
    assert!(events > 0, "a review logs change events");

    w.engine.retain_once().await.expect("retention");
    assert_eq!(
        count(&pool, "change_events").await,
        events,
        "recent history stays"
    );

    advance(91 * DAY);
    w.engine.retain_once().await.expect("retention");
    assert_eq!(count(&pool, "change_events").await, 0, "past 90 days");
    assert_eq!(count(&pool, "runs").await, 1, "runs are kept");
    assert!(count(&pool, "findings").await > 0, "findings are kept");
}
