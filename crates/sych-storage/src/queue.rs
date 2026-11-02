//! The operations view: jobs with the run each one serves, and publications that wait for the
//! reconciler or an operator. Both are limited to the caller's tenant and readable repositories.

use sqlx::Row;
use sych_core::api::{JobEntry, Page, UnresolvedPublication};

use crate::views::{page, repository};
use crate::{Storage, StorageError, Visibility};

#[derive(Debug, Clone, Default)]
pub struct JobFilter {
    pub repository_id: Option<i64>,
    pub state: Option<String>,
    pub before: Option<i64>,
    pub limit: i64,
}

impl Storage {
    /// Jobs newest first by id; finished ones only while retention keeps them.
    pub async fn jobs(
        &self,
        v: Visibility<'_>,
        f: &JobFilter,
    ) -> Result<Page<JobEntry>, StorageError> {
        let rows = sqlx::query(concat!(
            "SELECT j.id, j.kind, j.state, j.attempts, j.max_attempts, j.next_attempt_at,
                 j.lease_until, j.last_error, j.created_at, j.finished_at, u.id, u.state,
                 u.failure, c.id, c.external_number, r.id, r.connection_id, r.full_name
             FROM jobs j JOIN runs u ON u.id = j.run_id JOIN changes c ON c.id = u.change_id
             JOIN repositories r ON r.id = c.repository_id
             WHERE c.tenant_id = $1 AND ",
            visible!("r.id", "$2"),
            " AND (CAST($3 AS BIGINT) IS NULL OR r.id = $3)
              AND (CAST($4 AS TEXT) IS NULL OR j.state = $4)
              AND (CAST($5 AS BIGINT) IS NULL OR j.id < $5)
             ORDER BY j.id DESC LIMIT $6"
        ))
        .bind(v.tenant_id)
        .bind(v.set())
        .bind(f.repository_id)
        .bind(&f.state)
        .bind(f.before)
        .bind(f.limit + 1)
        .fetch_all(self.pool())
        .await?;
        let jobs = rows
            .iter()
            .map(|r| JobEntry {
                id: r.get(0),
                kind: r.get(1),
                state: r.get(2),
                attempts: r.get(3),
                max_attempts: r.get(4),
                next_attempt_at: r.get(5),
                lease_until: r.get(6),
                last_error: r.get(7),
                created_at: r.get(8),
                finished_at: r.get(9),
                run_id: r.get(10),
                run_state: r.get(11),
                run_failure: r.get(12),
                change_id: r.get(13),
                number: r.get(14),
                repository: repository(r, 15),
            })
            .collect();
        Ok(page(jobs, f.limit, |j| j.id))
    }

    /// `PUBLISHING` and `UNCERTAIN` operations, oldest first: the longest waiting lead.
    pub async fn unresolved(
        &self,
        v: Visibility<'_>,
        repository_id: Option<i64>,
    ) -> Result<Vec<UnresolvedPublication>, StorageError> {
        let rows = sqlx::query(concat!(
            "SELECT o.id, c.id, c.external_number, r.id, r.connection_id, r.full_name, p.slot,
                 o.state, o.attempts, o.created_at, o.reconcile_deadline, o.last_error
             FROM publication_operations o JOIN publication_objects p ON p.id = o.object_id
             JOIN changes c ON c.id = p.change_id JOIN repositories r ON r.id = c.repository_id
             WHERE o.state IN ('PUBLISHING', 'UNCERTAIN') AND c.tenant_id = $1 AND ",
            visible!("r.id", "$2"),
            " AND (CAST($3 AS BIGINT) IS NULL OR r.id = $3)
             ORDER BY o.created_at, o.id"
        ))
        .bind(v.tenant_id)
        .bind(v.set())
        .bind(repository_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .iter()
            .map(|r| UnresolvedPublication {
                id: r.get(0),
                change_id: r.get(1),
                number: r.get(2),
                repository: repository(r, 3),
                slot: r.get(6),
                state: r.get(7),
                attempts: r.get(8),
                created_at: r.get(9),
                reconcile_deadline: r.get(10),
                last_error: r.get(11),
            })
            .collect())
    }
}
