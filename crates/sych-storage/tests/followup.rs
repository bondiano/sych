//! Follow-up exchanges: a reader reads back only their own questions on a tour.

mod support;

use sych_core::analysis::followup::{Exchange, Reply};
use sych_core::tour::QuestionKind;
use sych_core::{RunId, SchemaVersion, TourId};
use sych_storage::TENANT_LOCAL;

fn exchange(tour: TourId, actor: &str, offset: i64) -> Exchange {
    Exchange {
        schema_version: SchemaVersion,
        id: uuid::Uuid::now_v7().to_string(),
        tour_id: tour,
        step_id: "stop-1".into(),
        actor: actor.into(),
        kind: QuestionKind::FactRequest,
        question: "the retry is idempotent".into(),
        at: support::at(offset),
        reply: Reply::HumanStatement {
            text: "the retry is idempotent".into(),
        },
    }
}

#[tokio::test]
async fn a_reader_reads_back_only_their_own_exchanges() {
    let (_dir, storage) = support::migrated().await;
    let (run, tour) = (RunId::new_v7(), TourId::new_v7());
    let mine = [exchange(tour, "ann", 2), exchange(tour, "ann", 1)];
    for e in mine.iter().chain([&exchange(tour, "bob", 0)]) {
        storage
            .add_exchange(TENANT_LOCAL, run, e)
            .await
            .expect("add");
    }
    let read = storage
        .exchanges(TENANT_LOCAL, tour, "ann")
        .await
        .expect("read");
    assert_eq!(
        read,
        [mine[1].clone(), mine[0].clone()],
        "own, oldest first"
    );
    assert!(
        storage
            .exchanges("other", tour, "ann")
            .await
            .expect("read")
            .is_empty(),
        "another tenant sees nothing"
    );
}
