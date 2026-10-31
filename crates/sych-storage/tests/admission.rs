//! Webhook admission on the selected backend: receipt, binding and intent in one
//! transaction, recognized repeats and separate tenant bindings of one repository.

use sych_core::scm::{ChangeSignal, RepositoryRef, WebhookEvent};
use sych_core::sha256;
use sych_storage::{Admitted, Connection};

mod support;

use support::{T0, at, migrated};

fn conn(tenant: &str, id: &str) -> Connection {
    Connection {
        tenant_id: tenant.into(),
        connection_id: id.into(),
        provider_instance: "github.com".into(),
    }
}

fn event(signal: ChangeSignal) -> WebhookEvent {
    WebhookEvent {
        event: "pull_request".into(),
        action: Some("synchronize".into()),
        repository: Some(RepositoryRef {
            external_id: "42".into(),
            full_name: "o/r".into(),
        }),
        installation_id: Some(9),
        change_numbers: vec![7],
        fork: false,
        sender: Some("dev".into()),
        signal: Some(signal),
    }
}

fn push(sha: &str) -> WebhookEvent {
    event(ChangeSignal::SourcePush {
        head_sha: sha.into(),
    })
}

#[tokio::test(flavor = "current_thread")]
async fn a_repeated_delivery_is_recognized_and_changes_nothing() {
    let (_dir, s) = migrated().await;
    let c = conn("local", "gh");
    let digest = sha256(b"body");
    let first = s
        .admit(&c, "d-1", &digest, &push("a"), true, at(0))
        .await
        .expect("admit");
    let Admitted::Stored { inbox_id, .. } = first else {
        panic!("{first:?}")
    };
    let again = s
        .admit(&c, "d-1", &digest, &push("a"), true, at(1))
        .await
        .expect("admit");
    assert_eq!(again, Admitted::Duplicate { inbox_id });
    let intents = s.open_intents().await.expect("intents");
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].events, 1, "the repeat did not touch the intent");
}

#[tokio::test(flavor = "current_thread")]
async fn a_stream_of_pushes_coalesces_within_a_finite_bound() {
    let (_dir, s) = migrated().await;
    let c = conn("local", "gh");
    for i in 0..20 {
        s.admit(
            &c,
            &format!("d-{i}"),
            &sha256(b"b"),
            &push("a"),
            true,
            at(i * 4),
        )
        .await
        .expect("admit");
    }
    let intents = s.open_intents().await.expect("intents");
    assert_eq!(intents.len(), 1);
    let i = &intents[0];
    assert_eq!(i.events, 20);
    assert_eq!(i.latest_at, T0 + 30, "5 s debounce, at most six of them");
    assert_eq!(i.due_at, T0 + 30, "the stream cannot push it further");
}

#[tokio::test(flavor = "current_thread")]
async fn two_tenants_bind_one_repository_separately() {
    let (_dir, s) = migrated().await;
    for (tenant, id) in [("a", "gh-a"), ("b", "gh-b")] {
        s.admit(
            &conn(tenant, id),
            "same-id",
            &sha256(b"b"),
            &push("a"),
            true,
            at(0),
        )
        .await
        .expect("admit");
    }
    let intents = s.open_intents().await.expect("intents");
    assert_eq!(
        intents.len(),
        2,
        "same delivery id on another connection is new"
    );
    assert_ne!(intents[0].change_id, intents[1].change_id);
    assert_ne!(intents[0].partition, intents[1].partition);
}

#[tokio::test(flavor = "current_thread")]
async fn bot_comments_and_unlisted_repositories_are_receipts_only() {
    let (_dir, s) = migrated().await;
    let c = conn("local", "gh");
    let bot = s
        .admit(
            &c,
            "d-1",
            &sha256(b"b"),
            &event(ChangeSignal::BotComment),
            true,
            at(0),
        )
        .await
        .expect("admit");
    let foreign = s
        .admit(&c, "d-2", &sha256(b"b"), &push("a"), false, at(0))
        .await
        .expect("admit");
    for admitted in [bot, foreign] {
        assert!(matches!(admitted, Admitted::Stored { ref intents, .. } if intents.is_empty()));
    }
    assert!(s.open_intents().await.expect("intents").is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn capabilities_are_fixed_per_binding() {
    use sych_core::scm::ProviderCapabilities;
    let (_dir, s) = migrated().await;
    let repo = RepositoryRef {
        external_id: "42".into(),
        full_name: "o/r".into(),
    };
    let caps = ProviderCapabilities {
        change_refs: true,
        max_diff_files: 3000,
        ..ProviderCapabilities::default()
    };
    s.bind_repository(&conn("a", "gh-a"), &repo, 9, &caps, at(0))
        .await
        .expect("bind");
    let a = s.repository_capabilities(&conn("a", "gh-a"), "42").await;
    assert_eq!(a.expect("read"), Some(caps));
    let b = s.repository_capabilities(&conn("b", "gh-b"), "42").await;
    assert_eq!(b.expect("read"), None, "another tenant sees no binding");
}
