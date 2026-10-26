//! Webhook admission (spec §19.2, §20.2): the inbox receipt, the repository binding and the
//! scheduling intent commit in one transaction, so a 2xx always means durable.

use sqlx::Row;
use sqlx::{Sqlite, Transaction};
use sych_core::scm::{ChangeSignal, ProviderCapabilities, RepositoryRef, WebhookEvent};
use sych_core::{Sha256Digest, Timestamp, canonical_json};

use crate::{Partition, Storage, StorageError};

/// Longest an intent waits behind a stream of equal events (`debounce × MAX_DEBOUNCES`).
pub const MAX_DEBOUNCES: i64 = 6;

/// Where a delivery arrived: the connection fixes tenant and provider instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub tenant_id: String,
    pub connection_id: String,
    pub provider_instance: String,
}

impl Connection {
    /// Tenant and repository binding: two connections of one repository never share a scope.
    pub fn partition(&self, repository_external_id: &str) -> Partition {
        Partition {
            tenant_id: self.tenant_id.clone(),
            access_scope: format!(
                "{}/{}/{repository_external_id}",
                self.provider_instance, self.connection_id
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admitted {
    /// A new receipt; `intents` are the change intents it created or moved.
    Stored { inbox_id: i64, intents: Vec<i64> },
    /// The delivery id was already stored; nothing changed.
    Duplicate { inbox_id: i64 },
}

/// The intent an event asks for and its debounce, or why it asks for none.
fn intent(signal: Option<&ChangeSignal>) -> Result<(String, i64), &'static str> {
    use sych_core::scm::ScmCommand;
    match signal {
        None => Err("IGNORED"),
        Some(ChangeSignal::BotComment) => Err("BOT_COMMENT"),
        Some(ChangeSignal::Closed { .. }) => Ok(("CLOSE".into(), 0)),
        Some(s @ ChangeSignal::CiCompleted { .. }) => Ok(("CI".into(), s.debounce_seconds())),
        Some(ChangeSignal::Command { command, .. }) => {
            let name = match command {
                ScmCommand::Rerun => "COMMAND_RERUN",
                ScmCommand::Deep => "COMMAND_DEEP",
                ScmCommand::Stop => "COMMAND_STOP",
            };
            Ok((name.into(), 0))
        }
        Some(s) => Ok(("REFRESH".into(), s.debounce_seconds())),
    }
}

impl Storage {
    /// Stores the receipt and, for a bound repository, its intents; a repeat is recognized.
    /// `allowed` is whether the connection lists the repository.
    pub async fn admit(
        &self,
        conn: &Connection,
        delivery_id: &str,
        payload: &Sha256Digest,
        event: &WebhookEvent,
        allowed: bool,
        now: Timestamp,
    ) -> Result<Admitted, StorageError> {
        let now = now.as_second();
        let wanted = intent(event.signal.as_ref());
        let decision = match (&event.repository, allowed, &wanted) {
            (None, ..) => "NO_REPOSITORY",
            (_, false, _) => "REPOSITORY_NOT_ALLOWED",
            (_, true, Err(reason)) => reason,
            (_, true, Ok(_)) if event.change_numbers.is_empty() => "NO_CHANGE",
            (_, true, Ok(_)) => "SCHEDULED",
        };
        let mut tx = self.pool().begin().await?;
        let inserted = sqlx::query(
            "INSERT INTO inbox_events (provider_instance, connection_id, delivery_id, event,
                 action, payload_sha256, normalized, received_at, decision)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (provider_instance, connection_id, delivery_id) DO NOTHING
             RETURNING id",
        )
        .bind(&conn.provider_instance)
        .bind(&conn.connection_id)
        .bind(delivery_id)
        .bind(&event.event)
        .bind(&event.action)
        .bind(payload.to_string())
        .bind(String::from_utf8_lossy(&canonical_json(event)?).into_owned())
        .bind(now)
        .bind(decision)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = inserted else {
            let row = sqlx::query(
                "SELECT id FROM inbox_events
                 WHERE provider_instance = ? AND connection_id = ? AND delivery_id = ?",
            )
            .bind(&conn.provider_instance)
            .bind(&conn.connection_id)
            .bind(delivery_id)
            .fetch_one(&mut *tx)
            .await?;
            return Ok(Admitted::Duplicate {
                inbox_id: row.get(0),
            });
        };
        let inbox_id: i64 = row.get(0);
        let mut intents = Vec::new();
        if let (Some(repo), true, Ok((kind, debounce))) = (&event.repository, allowed, wanted) {
            let repository_id: i64 = sqlx::query(
                "INSERT INTO repositories (tenant_id, connection_id, provider_instance,
                     external_id, full_name, installation_id)
                 VALUES (?, ?, ?, ?, ?, ?)
                 ON CONFLICT (tenant_id, connection_id, provider_instance, external_id)
                 DO UPDATE SET full_name = excluded.full_name,
                     installation_id = coalesce(excluded.installation_id, installation_id)
                 RETURNING id",
            )
            .bind(&conn.tenant_id)
            .bind(&conn.connection_id)
            .bind(&conn.provider_instance)
            .bind(&repo.external_id)
            .bind(&repo.full_name)
            .bind(event.installation_id.and_then(|i| i64::try_from(i).ok()))
            .fetch_one(&mut *tx)
            .await?
            .get(0);
            for number in &event.change_numbers {
                let partition = conn.partition(&repo.external_id);
                let id = schedule(
                    &mut tx,
                    &partition,
                    repository_id,
                    *number,
                    &kind,
                    debounce,
                    inbox_id,
                    now,
                )
                .await?;
                intents.push(id);
            }
        }

        tx.commit().await?;
        Ok(Admitted::Stored { inbox_id, intents })
    }
}

/// Upserts the change and its coalesced intent; returns the intent id.
#[allow(clippy::too_many_arguments)] // one transaction step with its keys
async fn schedule(
    tx: &mut Transaction<'_, Sqlite>,
    partition: &Partition,
    repository_id: i64,
    number: u64,
    kind: &str,
    debounce: i64,
    inbox_id: i64,
    now: i64,
) -> Result<i64, StorageError> {
    let change_id: i64 = sqlx::query(
        "INSERT INTO changes (tenant_id, access_scope, change_key, desired_generation,
             repository_id, external_number)
         VALUES (?, ?, ?, 0, ?, ?)
         ON CONFLICT (tenant_id, access_scope, change_key)
         DO UPDATE SET repository_id = excluded.repository_id
         RETURNING id",
    )
    .bind(&partition.tenant_id)
    .bind(&partition.access_scope)
    .bind(format!("pr/{number}"))
    .bind(repository_id)
    .bind(i64::try_from(number).unwrap_or(i64::MAX))
    .fetch_one(&mut **tx)
    .await?
    .get(0);
    Ok(sqlx::query(
        "INSERT INTO change_intents (change_id, kind, due_at, latest_at, events,
             first_inbox_id, last_inbox_id, created_at)
         VALUES (?, ?, ?, ?, 1, ?, ?, ?)
         ON CONFLICT (change_id, kind) WHERE consumed_at IS NULL
         DO UPDATE SET due_at = min(excluded.due_at, latest_at),
             events = events + 1, last_inbox_id = excluded.last_inbox_id
         RETURNING id",
    )
    .bind(change_id)
    .bind(kind)
    .bind(now + debounce)
    .bind(now + debounce * MAX_DEBOUNCES)
    .bind(inbox_id)
    .bind(inbox_id)
    .bind(now)
    .fetch_one(&mut **tx)
    .await?
    .get(0))
}

/// An unconsumed change intent with the change it concerns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Intent {
    pub id: i64,
    pub change_id: i64,
    pub partition: Partition,
    pub change_key: String,
    pub kind: String,
    pub due_at: i64,
    pub latest_at: i64,
    pub events: i64,
    pub first_inbox_id: i64,
    pub last_inbox_id: i64,
}

impl Storage {
    /// Open intents, earliest due first.
    pub async fn open_intents(&self) -> Result<Vec<Intent>, StorageError> {
        let rows = sqlx::query(
            "SELECT i.id, i.change_id, c.tenant_id, c.access_scope, c.change_key, i.kind,
                 i.due_at, i.latest_at, i.events, i.first_inbox_id, i.last_inbox_id
             FROM change_intents i JOIN changes c ON c.id = i.change_id
             WHERE i.consumed_at IS NULL ORDER BY i.due_at, i.id",
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .iter()
            .map(|r| Intent {
                id: r.get(0),
                change_id: r.get(1),
                partition: Partition {
                    tenant_id: r.get(2),
                    access_scope: r.get(3),
                },
                change_key: r.get(4),
                kind: r.get(5),
                due_at: r.get(6),
                latest_at: r.get(7),
                events: r.get(8),
                first_inbox_id: r.get(9),
                last_inbox_id: r.get(10),
            })
            .collect())
    }
}

impl Storage {
    /// Whether the database answers; for readiness.
    pub async fn ping(&self) -> Result<(), StorageError> {
        sqlx::query("SELECT 1").execute(self.pool()).await?;
        Ok(())
    }
}

impl Storage {
    /// Binds a repository and fixes its capabilities, when connected or updated (§19.1).
    pub async fn bind_repository(
        &self,
        conn: &Connection,
        repo: &RepositoryRef,
        installation_id: u64,
        capabilities: &ProviderCapabilities,
        now: Timestamp,
    ) -> Result<i64, StorageError> {
        let caps = String::from_utf8_lossy(&canonical_json(capabilities)?).into_owned();
        let row = sqlx::query(
            "INSERT INTO repositories (tenant_id, connection_id, provider_instance, external_id,
                 full_name, installation_id, capabilities, capabilities_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (tenant_id, connection_id, provider_instance, external_id)
             DO UPDATE SET full_name = excluded.full_name,
                 installation_id = excluded.installation_id,
                 capabilities = excluded.capabilities, capabilities_at = excluded.capabilities_at
             RETURNING id",
        )
        .bind(&conn.tenant_id)
        .bind(&conn.connection_id)
        .bind(&conn.provider_instance)
        .bind(&repo.external_id)
        .bind(&repo.full_name)
        .bind(i64::try_from(installation_id).unwrap_or(i64::MAX))
        .bind(caps)
        .bind(now.as_second())
        .fetch_one(self.pool())
        .await?;
        Ok(row.get(0))
    }

    /// The capabilities fixed for a binding, if it was connected.
    pub async fn repository_capabilities(
        &self,
        conn: &Connection,
        external_id: &str,
    ) -> Result<Option<ProviderCapabilities>, StorageError> {
        let row = sqlx::query(
            "SELECT capabilities FROM repositories WHERE tenant_id = ? AND connection_id = ?
                 AND provider_instance = ? AND external_id = ?",
        )
        .bind(&conn.tenant_id)
        .bind(&conn.connection_id)
        .bind(&conn.provider_instance)
        .bind(external_id)
        .fetch_optional(self.pool())
        .await?;
        let text: Option<String> = row.and_then(|r| r.get(0));
        Ok(text.map(|t| serde_json::from_str(&t)).transpose()?)
    }
}

impl Storage {
    /// The normalized event of a receipt.
    pub async fn inbox_event(&self, inbox_id: i64) -> Result<WebhookEvent, StorageError> {
        let text: String = sqlx::query("SELECT normalized FROM inbox_events WHERE id = ?")
            .bind(inbox_id)
            .fetch_one(self.pool())
            .await?
            .get(0);
        Ok(serde_json::from_str(&text)?)
    }

    /// Moves an intent whose handling must be retried; the bound moves with it.
    pub async fn postpone_intent(&self, intent_id: i64, due_at: i64) -> Result<(), StorageError> {
        sqlx::query(
            "UPDATE change_intents SET due_at = ?, latest_at = max(latest_at, ?)
             WHERE id = ? AND consumed_at IS NULL",
        )
        .bind(due_at)
        .bind(due_at)
        .bind(intent_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }
}
