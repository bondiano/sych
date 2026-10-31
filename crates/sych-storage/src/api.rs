//! Storage behind the HTTP API: idempotency keys scoped by tenant, actor and
//! endpoint; read models of runs and publications; and the numbers `/metrics` reports.

use sqlx::Row;
use sqlx::any::AnyRow;
use sych_core::Timestamp;
use sych_core::scm::RepositoryRef;

use crate::audit::Audit;
use crate::{Storage, StorageError};

/// Default lifetime of an idempotency key.
pub const IDEMPOTENCY_TTL_SECONDS: i64 = 86_400;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Idempotency {
    /// First use: handle the request, then call [`Storage::idempotency_finish`].
    New(i64),
    /// The same request again: answer what was answered.
    Replay { status: u16, body: String },
    /// The key was used with another body.
    Conflict,
    /// The first request with this key has not finished.
    InProgress,
}

/// One run as the API shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunView {
    pub run_id: String,
    pub change_id: i64,
    pub connection_id: Option<String>,
    pub full_name: Option<String>,
    pub generation: i64,
    pub state: String,
    pub stage: String,
    pub coverage: Option<String>,
    pub model_attempts: i64,
    pub cost_known_micro_usd: i64,
    pub unknown_cost_attempts: i64,
    pub failure: Option<String>,
    pub created_at: i64,
    pub finished_at: Option<i64>,
    pub parent_run_id: Option<String>,
    pub unknown_obligations: Option<i64>,
}

/// A row of [`run_view!`].
pub(crate) fn run_view(r: &AnyRow) -> RunView {
    RunView {
        run_id: r.get(0),
        change_id: r.get(1),
        connection_id: r.get(2),
        full_name: r.get(3),
        generation: r.get(4),
        state: r.get(5),
        stage: r.get(6),
        coverage: r.get(7),
        model_attempts: r.get(8),
        cost_known_micro_usd: r.get(9),
        unknown_cost_attempts: r.get(10),
        failure: r.get(11),
        created_at: r.get(12),
        finished_at: r.get(13),
        parent_run_id: r.get(14),
        unknown_obligations: r.get(15),
    }
}

/// Point-in-time numbers for `/metrics`; bounded label sets only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metrics {
    pub runs_by_state: Vec<(String, i64)>,
    pub runs_by_coverage: Vec<(String, i64)>,
    pub unknown_obligations: i64,
    pub oldest_queued_age: i64,
    pub jobs_by_state: Vec<(String, i64)>,
    pub attempts_by_state: Vec<(String, i64)>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_known_micro_usd: i64,
    pub reserved_micro_usd: i64,
    pub superseded_cost_micro_usd: i64,
    pub publications_by_state: Vec<(String, i64)>,
    pub oldest_pending_publication_age: i64,
    pub oldest_uncertain_publication_age: i64,
    pub publication_repairs: i64,
    pub feedback_by_judgement: Vec<(String, i64)>,
    pub artifact_bytes: i64,
    pub artifacts: i64,
}

impl Storage {
    /// Reserves `key` for a request whose body hashes to `body_sha256`.
    pub async fn idempotency_begin(
        &self,
        scope: (&str, &str, &str),
        key: &str,
        body_sha256: &str,
        now: Timestamp,
    ) -> Result<Idempotency, StorageError> {
        let (tenant, actor, endpoint) = scope;
        let at = now.as_second();
        let mut tx = self.write().await?;
        sqlx::query(
            "DELETE FROM idempotency_keys WHERE tenant_id = $1 AND actor = $2 AND endpoint = $3
                 AND key = $4 AND expires_at <= $5",
        )
        .bind(tenant)
        .bind(actor)
        .bind(endpoint)
        .bind(key)
        .bind(at)
        .execute(&mut *tx)
        .await?;
        let existing = sqlx::query(
            "SELECT body_sha256, response_status, response_body FROM idempotency_keys
             WHERE tenant_id = $1 AND actor = $2 AND endpoint = $3 AND key = $4",
        )
        .bind(tenant)
        .bind(actor)
        .bind(endpoint)
        .bind(key)
        .fetch_optional(&mut *tx)
        .await?;
        let outcome = match existing {
            Some(r) if r.get::<String, _>(0) != body_sha256 => Idempotency::Conflict,
            Some(r) => match (r.get::<Option<i64>, _>(1), r.get::<Option<String>, _>(2)) {
                (Some(status), Some(body)) => Idempotency::Replay {
                    status: u16::try_from(status).unwrap_or(500),
                    body,
                },
                _ => Idempotency::InProgress,
            },
            None => Idempotency::New(
                sqlx::query_scalar(
                    "INSERT INTO idempotency_keys (tenant_id, actor, endpoint, key, body_sha256,
                         created_at, expires_at) VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
                )
                .bind(tenant)
                .bind(actor)
                .bind(endpoint)
                .bind(key)
                .bind(body_sha256)
                .bind(at)
                .bind(at + IDEMPOTENCY_TTL_SECONDS)
                .fetch_one(&mut *tx)
                .await?,
            ),
        };
        tx.commit().await?;
        Ok(outcome)
    }

    /// Stores the answer to replay for the key.
    pub async fn idempotency_finish(
        &self,
        id: i64,
        status: u16,
        body: &str,
    ) -> Result<(), StorageError> {
        let mut tx = self.write().await?;
        sqlx::query(
            "UPDATE idempotency_keys SET response_status = $1, response_body = $2 WHERE id = $3",
        )
        .bind(i64::from(status))
        .bind(body)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// A run of the tenant, with the repository binding its access is checked against.
    pub async fn run_view(
        &self,
        tenant_id: &str,
        run_id: &str,
    ) -> Result<Option<RunView>, StorageError> {
        let r = sqlx::query(concat!(run_view!(), "WHERE u.id = $1 AND u.tenant_id = $2"))
            .bind(run_id)
            .bind(tenant_id)
            .fetch_optional(self.pool())
            .await?;
        Ok(r.as_ref().map(run_view))
    }

    /// Cancels an active run with its job; a terminal run keeps its state. Returns the state
    /// after the call.
    pub async fn cancel_run_by_id(
        &self,
        tenant_id: &str,
        run_id: &str,
        actor: &str,
        now: Timestamp,
    ) -> Result<Option<String>, StorageError> {
        let at = now.as_second();
        let mut tx = self.write().await?;
        let done = sqlx::query(concat!(
            "UPDATE runs SET state = 'CANCELLED', failure = $1, finished_at = $2
             WHERE id = $3 AND tenant_id = $4 AND state IN ",
            active!()
        ))
        .bind(format!("cancelled by {actor}"))
        .bind(at)
        .bind(run_id)
        .bind(tenant_id)
        .execute(&mut *tx)
        .await?;
        if done.rows_affected() == 1 {
            sqlx::query(
                "UPDATE jobs SET state = 'CANCELLED', lease_epoch = lease_epoch + 1, finished_at = $1
                 WHERE run_id = $2 AND state IN ('QUEUED', 'CLAIMED')",
            )
            .bind(at)
            .bind(run_id)
            .execute(&mut *tx)
            .await?;
            let repository_id: Option<i64> = sqlx::query_scalar(
                "SELECT c.repository_id FROM runs u JOIN changes c ON c.id = u.change_id
                 WHERE u.id = $1",
            )
            .bind(run_id)
            .fetch_one(&mut *tx)
            .await?;
            Audit {
                tenant_id,
                actor,
                action: "RUN_CANCELLED",
                target: format!("run:{run_id}"),
                repository_id,
                metadata: serde_json::json!({}),
                at,
            }
            .record(&mut tx)
            .await?;
        }

        let state = sqlx::query_scalar("SELECT state FROM runs WHERE id = $1 AND tenant_id = $2")
            .bind(run_id)
            .bind(tenant_id)
            .fetch_optional(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(state)
    }

    /// `(tenant, change, state, relevance, delivered remote id, last error)` of an operation.
    pub async fn publication_view(
        &self,
        op_id: i64,
    ) -> Result<Option<(String, i64, String, String, Option<String>, Option<String>)>, StorageError>
    {
        let r = sqlx::query(
            "SELECT c.tenant_id, p.change_id, o.state, p.relevance, o.remote_id, o.last_error
             FROM publication_operations o JOIN publication_objects p ON p.id = o.object_id
             JOIN changes c ON c.id = p.change_id WHERE o.id = $1",
        )
        .bind(op_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(r.map(|r| (r.get(0), r.get(1), r.get(2), r.get(3), r.get(4), r.get(5))))
    }

    /// The change a finding belongs to, within the tenant.
    pub async fn finding_change(
        &self,
        tenant_id: &str,
        finding: i64,
    ) -> Result<Option<i64>, StorageError> {
        Ok(sqlx::query_scalar(
            "SELECT f.change_id FROM findings f JOIN changes c ON c.id = f.change_id
             WHERE f.id = $1 AND c.tenant_id = $2",
        )
        .bind(finding)
        .bind(tenant_id)
        .fetch_optional(self.pool())
        .await?)
    }

    pub async fn metrics(&self, now: Timestamp) -> Result<Metrics, StorageError> {
        let at = now.as_second();
        let pairs = |sql: &'static str| async move {
            let rows = sqlx::query(sql).fetch_all(self.pool()).await?;
            Ok::<_, StorageError>(
                rows.iter()
                    .map(|r| (r.get(0), r.get(1)))
                    .collect::<Vec<_>>(),
            )
        };
        let one = |sql: &'static str| async move {
            Ok::<i64, StorageError>(
                sqlx::query_scalar(sql)
                    .bind(at)
                    .fetch_one(self.pool())
                    .await?,
            )
        };
        Ok(Metrics {
            runs_by_state: pairs("SELECT state, count(*) FROM runs GROUP BY state ORDER BY state").await?,
            runs_by_coverage: pairs(
                "SELECT coverage, count(*) FROM runs WHERE coverage IS NOT NULL GROUP BY coverage ORDER BY coverage",
            )
            .await?,
            unknown_obligations: one(
                "SELECT CAST(coalesce(sum(unknown_obligations), 0) AS BIGINT) + 0 * CAST($1 AS BIGINT) FROM runs",
            ).await?,
            oldest_queued_age: one(
                "SELECT coalesce(CAST($1 AS BIGINT) - min(next_attempt_at), 0) FROM jobs WHERE state = 'QUEUED'",
            )
            .await?,
            jobs_by_state: pairs("SELECT state, count(*) FROM jobs GROUP BY state ORDER BY state").await?,
            attempts_by_state: pairs(
                "SELECT state, count(*) FROM model_attempts GROUP BY state ORDER BY state",
            )
            .await?,
            input_tokens: one(
                "SELECT CAST(coalesce(sum(input_tokens), 0) AS BIGINT) + 0 * CAST($1 AS BIGINT) FROM model_attempts",
            ).await?,
            output_tokens: one(
                "SELECT CAST(coalesce(sum(output_tokens), 0) AS BIGINT) + 0 * CAST($1 AS BIGINT) FROM model_attempts",
            ).await?,
            cost_known_micro_usd: one(
                "SELECT CAST(coalesce(sum(charge_micro_usd), 0) AS BIGINT) + 0 * CAST($1 AS BIGINT)
                 FROM model_attempts WHERE state = 'SETTLED'",
            )
            .await?,
            reserved_micro_usd: one(
                "SELECT CAST(coalesce(sum(reserved_micro_usd), 0) AS BIGINT) + 0 * CAST($1 AS BIGINT) FROM model_attempts
                 WHERE state IN ('RESERVED', 'UNKNOWN')",
            )
            .await?,
            superseded_cost_micro_usd: one(
                "SELECT CAST(coalesce(sum(a.charge_micro_usd), 0) AS BIGINT) + 0 * CAST($1 AS BIGINT) FROM model_attempts a
                 JOIN runs u ON u.id = a.run_id WHERE u.state = 'SUPERSEDED'",
            )
            .await?,
            publications_by_state: pairs(
                "SELECT state, count(*) FROM publication_operations GROUP BY state ORDER BY state",
            )
            .await?,
            oldest_pending_publication_age: one(
                "SELECT coalesce(CAST($1 AS BIGINT) - min(created_at), 0) FROM publication_operations WHERE state = 'PENDING'",
            )
            .await?,
            oldest_uncertain_publication_age: one(
                "SELECT coalesce(CAST($1 AS BIGINT) - min(created_at), 0) FROM publication_operations WHERE state = 'UNCERTAIN'",
            )
            .await?,
            publication_repairs: one(
                "SELECT count(*) + 0 * CAST($1 AS BIGINT) FROM change_events WHERE kind = 'PUBLICATION_REPAIRED'",
            )
            .await?,
            feedback_by_judgement: pairs(
                "SELECT judgement, count(*) FROM feedback GROUP BY judgement ORDER BY judgement",
            )
            .await?,
            artifact_bytes: one(
                "SELECT CAST(coalesce(sum(size), 0) AS BIGINT) + 0 * CAST($1 AS BIGINT) FROM artifacts",
            ).await?,
            artifacts: one("SELECT count(*) + 0 * CAST($1 AS BIGINT) FROM artifacts").await?,
        })
    }
}

impl Storage {
    /// The change of `number` in a bound repository, created when an API request names it
    /// before any webhook did.
    pub async fn ensure_change(
        &self,
        conn: &crate::Connection,
        repo: &RepositoryRef,
        number: u64,
    ) -> Result<i64, StorageError> {
        let mut tx = self.write().await?;
        let repository: i64 = sqlx::query_scalar(
            "INSERT INTO repositories (tenant_id, connection_id, provider_instance, external_id, full_name)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (tenant_id, connection_id, provider_instance, external_id)
             DO UPDATE SET full_name = excluded.full_name RETURNING id",
        )
        .bind(&conn.tenant_id)
        .bind(&conn.connection_id)
        .bind(&conn.provider_instance)
        .bind(&repo.external_id)
        .bind(&repo.full_name)
        .fetch_one(&mut *tx)
        .await?;
        let partition = conn.partition(&repo.external_id);
        let change = sqlx::query_scalar(
            "INSERT INTO changes (tenant_id, access_scope, change_key, desired_generation,
                 repository_id, external_number) VALUES ($1, $2, $3, 0, $4, $5)
             ON CONFLICT (tenant_id, access_scope, change_key)
             DO UPDATE SET repository_id = excluded.repository_id RETURNING id",
        )
        .bind(&partition.tenant_id)
        .bind(&partition.access_scope)
        .bind(format!("pr/{number}"))
        .bind(repository)
        .bind(i64::try_from(number).unwrap_or(i64::MAX))
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(change)
    }
}
