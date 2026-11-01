//! Read models behind the dashboard: lists newest first with an id cursor, a change and a
//! finding with their history, a run's attempts and period totals. Every query is limited to
//! the caller's tenant and readable repositories; changes without a repository never show.

use sqlx::Row;
use sqlx::any::AnyRow;
use sych_core::api::{
    Attempt, ChangeEvent, ChangeSummary, FeedbackEntry, FindingEvent, FindingSummary, LatestRun,
    Page, PublicationEntry, Repository, WaiverEntry,
};

use crate::api::run_view;
use crate::audit::Audit;
use crate::{RunView, Storage, StorageError};

/// What a caller may read: its tenant and, unless every repository there, their ids.
#[derive(Debug, Clone, Copy)]
pub struct Visibility<'a> {
    pub tenant_id: &'a str,
    pub repositories: Option<&'a [i64]>,
}

impl Visibility<'_> {
    /// The bound form [`visible!`] matches against.
    pub(crate) fn set(&self) -> Option<String> {
        self.repositories.map(|ids| {
            ids.iter()
                .fold(String::from(","), |s, id| format!("{s}{id},"))
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct ChangeFilter {
    pub repository_id: Option<i64>,
    pub state: Option<String>,
    pub before: Option<i64>,
    pub limit: i64,
}

#[derive(Debug, Clone, Default)]
pub struct FindingFilter {
    pub repository_id: Option<i64>,
    pub change_id: Option<i64>,
    pub state: Option<String>,
    pub category: Option<String>,
    pub severity: Option<String>,
    pub rule_id: Option<String>,
    pub before: Option<i64>,
    pub limit: i64,
}

/// `limit + 1` rows in, one page out: the extra row only says another page exists.
pub(crate) fn page<T>(mut items: Vec<T>, limit: i64, id: impl Fn(&T) -> i64) -> Page<T> {
    let more = usize::try_from(limit).is_ok_and(|l| items.len() > l);
    if more {
        items.pop();
    }

    let next_before = items.last().filter(|_| more).map(id);
    Page { items, next_before }
}

pub(crate) fn repository(r: &AnyRow, at: usize) -> Repository {
    Repository {
        id: r.get(at),
        connection_id: r.get(at + 1),
        full_name: r.get(at + 2),
    }
}

macro_rules! change_summary {
    () => {
        "SELECT c.id, r.id, r.connection_id, r.full_name, c.external_number, c.scm_state,
             c.head_sha, c.synced_at, u.id, u.generation, u.state, u.stage, u.coverage,
             u.unknown_obligations, u.cost_known_micro_usd, u.unknown_cost_attempts, u.created_at,
             u.finished_at,
             (SELECT count(*) FROM findings f WHERE f.change_id = c.id AND f.state = 'OPEN')
         FROM changes c JOIN repositories r ON r.id = c.repository_id
         LEFT JOIN runs u ON u.change_id = c.id
             AND u.generation = (SELECT max(g.generation) FROM runs g WHERE g.change_id = c.id)
         WHERE c.tenant_id = $1 AND "
    };
}

fn change_summary(r: &AnyRow) -> ChangeSummary {
    let run: Option<String> = r.get(8);
    ChangeSummary {
        id: r.get(0),
        repository: repository(r, 1),
        number: r.get(4),
        scm_state: r.get(5),
        head_sha: r.get(6),
        synced_at: r.get(7),
        latest_run: run.map(|run_id| LatestRun {
            run_id,
            generation: r.get(9),
            state: r.get(10),
            stage: r.get(11),
            coverage: r.get(12),
            unknown_obligations: r.get(13),
            cost_known_micro_usd: r.get(14),
            unknown_cost_attempts: r.get(15),
            created_at: r.get(16),
            finished_at: r.get(17),
        }),
        open_findings: r.get(18),
    }
}

macro_rules! finding_summary {
    () => {
        "SELECT f.id, f.change_id, r.id, r.connection_id, r.full_name, c.external_number,
             f.finding_id, f.category, f.severity, f.title, f.rule_id, f.path, f.state,
             f.first_run_id, f.last_run_id, f.updated_at
         FROM findings f JOIN changes c ON c.id = f.change_id
         JOIN repositories r ON r.id = c.repository_id
         WHERE c.tenant_id = $1 AND "
    };
}

fn finding_summary(r: &AnyRow) -> FindingSummary {
    FindingSummary {
        id: r.get(0),
        change_id: r.get(1),
        repository: repository(r, 2),
        number: r.get(5),
        finding_id: r.get(6),
        category: r.get(7),
        severity: r.get(8),
        title: r.get(9),
        rule_id: r.get(10),
        path: r.get(11),
        state: r.get(12),
        first_run_id: r.get(13),
        last_run_id: r.get(14),
        updated_at: r.get(15),
    }
}

macro_rules! waiver {
    () => {
        "SELECT w.id, r.id, r.connection_id, r.full_name, w.rule_id, w.paths, w.reason, w.owner,
             w.author, w.expires_at, w.until, w.created_at, w.revoked_at
         FROM waivers w JOIN repositories r ON r.id = w.repository_id
         WHERE w.tenant_id = $1 AND "
    };
}

fn waiver(r: &AnyRow) -> Result<WaiverEntry, StorageError> {
    Ok(WaiverEntry {
        id: r.get(0),
        repository: repository(r, 1),
        rule_id: r.get(4),
        paths: serde_json::from_str(&r.get::<String, _>(5))?,
        reason: r.get(6),
        owner: r.get(7),
        author: r.get(8),
        expires_at: r.get(9),
        until: r.get(10),
        created_at: r.get(11),
        revoked_at: r.get(12),
    })
}

impl Storage {
    /// Every repository of the tenant; the caller narrows them to its own scope.
    pub async fn repositories(&self, tenant_id: &str) -> Result<Vec<Repository>, StorageError> {
        let rows = sqlx::query(
            "SELECT id, connection_id, full_name FROM repositories WHERE tenant_id = $1
             ORDER BY connection_id, full_name",
        )
        .bind(tenant_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows.iter().map(|r| repository(r, 0)).collect())
    }

    pub async fn changes(
        &self,
        v: Visibility<'_>,
        f: &ChangeFilter,
    ) -> Result<Page<ChangeSummary>, StorageError> {
        let rows = sqlx::query(concat!(
            change_summary!(),
            visible!("r.id", "$2"),
            " AND (CAST($3 AS BIGINT) IS NULL OR r.id = $3)
              AND (CAST($4 AS TEXT) IS NULL OR c.scm_state = $4)
              AND (CAST($5 AS BIGINT) IS NULL OR c.id < $5)
             ORDER BY c.id DESC LIMIT $6"
        ))
        .bind(v.tenant_id)
        .bind(v.set())
        .bind(f.repository_id)
        .bind(&f.state)
        .bind(f.before)
        .bind(f.limit + 1)
        .fetch_all(self.pool())
        .await?;
        Ok(page(
            rows.iter().map(change_summary).collect(),
            f.limit,
            |c| c.id,
        ))
    }

    pub async fn change_summary(
        &self,
        v: Visibility<'_>,
        id: i64,
    ) -> Result<Option<ChangeSummary>, StorageError> {
        let row = sqlx::query(concat!(
            change_summary!(),
            visible!("r.id", "$2"),
            " AND c.id = $3"
        ))
        .bind(v.tenant_id)
        .bind(v.set())
        .bind(id)
        .fetch_optional(self.pool())
        .await?;
        Ok(row.as_ref().map(change_summary))
    }

    /// Every generation's run of a change, oldest first.
    pub async fn runs_of(&self, change_id: i64) -> Result<Vec<RunView>, StorageError> {
        let rows = sqlx::query(concat!(
            run_view!(),
            "WHERE u.change_id = $1 ORDER BY u.generation"
        ))
        .bind(change_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows.iter().map(run_view).collect())
    }

    /// The change's event log, oldest first.
    pub async fn change_log(&self, change_id: i64) -> Result<Vec<ChangeEvent>, StorageError> {
        let rows = sqlx::query(
            "SELECT kind, detail, run_id, at FROM change_events WHERE change_id = $1 ORDER BY id",
        )
        .bind(change_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .iter()
            .map(|r| ChangeEvent {
                kind: r.get(0),
                detail: r.get(1),
                run_id: r.get(2),
                at: r.get(3),
            })
            .collect())
    }

    /// Every delivery attempt of the change's publications, oldest first.
    pub async fn publications_of(
        &self,
        change_id: i64,
    ) -> Result<Vec<PublicationEntry>, StorageError> {
        let rows = sqlx::query(
            "SELECT o.id, p.kind, o.state, p.relevance, o.generation, o.remote_id, o.last_error,
                 o.attempts, o.created_at, o.finished_at
             FROM publication_operations o JOIN publication_objects p ON p.id = o.object_id
             WHERE p.change_id = $1 ORDER BY o.id",
        )
        .bind(change_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .iter()
            .map(|r| PublicationEntry {
                id: r.get(0),
                kind: r.get(1),
                state: r.get(2),
                relevance: r.get(3),
                generation: r.get(4),
                remote_id: r.get(5),
                last_error: r.get(6),
                attempts: r.get(7),
                created_at: r.get(8),
                finished_at: r.get(9),
            })
            .collect())
    }

    pub async fn findings(
        &self,
        v: Visibility<'_>,
        f: &FindingFilter,
    ) -> Result<Page<FindingSummary>, StorageError> {
        let rows = sqlx::query(concat!(
            finding_summary!(),
            visible!("r.id", "$2"),
            " AND (CAST($3 AS BIGINT) IS NULL OR r.id = $3)
              AND (CAST($4 AS BIGINT) IS NULL OR f.change_id = $4)
              AND (CAST($5 AS TEXT) IS NULL OR f.state = $5)
              AND (CAST($6 AS TEXT) IS NULL OR f.category = $6)
              AND (CAST($7 AS TEXT) IS NULL OR f.severity = $7)
              AND (CAST($8 AS TEXT) IS NULL OR f.rule_id = $8)
              AND (CAST($9 AS BIGINT) IS NULL OR f.id < $9)
             ORDER BY f.id DESC LIMIT $10"
        ))
        .bind(v.tenant_id)
        .bind(v.set())
        .bind(f.repository_id)
        .bind(f.change_id)
        .bind(&f.state)
        .bind(&f.category)
        .bind(&f.severity)
        .bind(&f.rule_id)
        .bind(f.before)
        .bind(f.limit + 1)
        .fetch_all(self.pool())
        .await?;
        Ok(page(
            rows.iter().map(finding_summary).collect(),
            f.limit,
            |f| f.id,
        ))
    }

    pub async fn finding_summary(
        &self,
        v: Visibility<'_>,
        id: i64,
    ) -> Result<Option<FindingSummary>, StorageError> {
        let row = sqlx::query(concat!(
            finding_summary!(),
            visible!("r.id", "$2"),
            " AND f.id = $3"
        ))
        .bind(v.tenant_id)
        .bind(v.set())
        .bind(id)
        .fetch_optional(self.pool())
        .await?;
        Ok(row.as_ref().map(finding_summary))
    }

    /// A finding's lifecycle transitions and the feedback on it, both oldest first.
    pub async fn finding_log(
        &self,
        finding: i64,
    ) -> Result<(Vec<FindingEvent>, Vec<FeedbackEntry>), StorageError> {
        let events = sqlx::query(
            "SELECT run_id, from_state, to_state, reason, actor, at FROM finding_events
             WHERE finding_id = $1 ORDER BY id",
        )
        .bind(finding)
        .fetch_all(self.pool())
        .await?;
        let feedback = sqlx::query(
            "SELECT actor, judgement, scope, rationale, at FROM feedback
             WHERE finding_id = $1 ORDER BY id",
        )
        .bind(finding)
        .fetch_all(self.pool())
        .await?;
        Ok((
            events
                .iter()
                .map(|r| FindingEvent {
                    run_id: r.get(0),
                    from_state: r.get(1),
                    to_state: r.get(2),
                    reason: r.get(3),
                    actor: r.get(4),
                    at: r.get(5),
                })
                .collect(),
            feedback
                .iter()
                .map(|r| FeedbackEntry {
                    actor: r.get(0),
                    judgement: r.get(1),
                    scope: r.get(2),
                    rationale: r.get(3),
                    at: r.get(4),
                })
                .collect(),
        ))
    }

    /// The run's model attempts in the order they were reserved.
    pub async fn attempts(
        &self,
        tenant_id: &str,
        run_id: &str,
    ) -> Result<Vec<Attempt>, StorageError> {
        let rows = sqlx::query(
            "SELECT id, purpose, kind, profile, provider, requested_model, resolved_model,
                 billing_kind, state, reserved_micro_usd, charge_micro_usd, input_tokens,
                 output_tokens, reasoning_tokens, created_at, settled_at
             FROM model_attempts WHERE tenant_id = $1 AND run_id = $2 ORDER BY created_at, id",
        )
        .bind(tenant_id)
        .bind(run_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .iter()
            .map(|r| Attempt {
                id: r.get(0),
                purpose: r.get(1),
                kind: r.get(2),
                profile: r.get(3),
                provider: r.get(4),
                requested_model: r.get(5),
                resolved_model: r.get(6),
                billing_kind: r.get(7),
                state: r.get(8),
                reserved_micro_usd: r.get(9),
                charge_micro_usd: r.get(10),
                input_tokens: r.get(11),
                output_tokens: r.get(12),
                reasoning_tokens: r.get(13),
                created_at: r.get(14),
                settled_at: r.get(15),
            })
            .collect())
    }

    /// Waivers by state at `now`: `active`, `expired`, `revoked`, or every one for `None`.
    pub async fn waivers(
        &self,
        v: Visibility<'_>,
        repository_id: Option<i64>,
        state: Option<&str>,
        now: i64,
    ) -> Result<Vec<WaiverEntry>, StorageError> {
        let rows = sqlx::query(concat!(
            waiver!(),
            visible!("r.id", "$2"),
            " AND (CAST($3 AS BIGINT) IS NULL OR r.id = $3)
              AND (CAST($4 AS TEXT) IS NULL
                  OR ($4 = 'revoked' AND w.revoked_at IS NOT NULL)
                  OR ($4 = 'active' AND w.revoked_at IS NULL
                      AND (w.expires_at IS NULL OR w.expires_at > $5))
                  OR ($4 = 'expired' AND w.revoked_at IS NULL AND w.expires_at <= $5))
             ORDER BY w.id DESC"
        ))
        .bind(v.tenant_id)
        .bind(v.set())
        .bind(repository_id)
        .bind(state)
        .bind(now)
        .fetch_all(self.pool())
        .await?;
        rows.iter().map(waiver).collect()
    }

    pub async fn waiver(
        &self,
        v: Visibility<'_>,
        id: i64,
    ) -> Result<Option<WaiverEntry>, StorageError> {
        let row = sqlx::query(concat!(waiver!(), visible!("r.id", "$2"), " AND w.id = $3"))
            .bind(v.tenant_id)
            .bind(v.set())
            .bind(id)
            .fetch_optional(self.pool())
            .await?;
        row.as_ref().map(waiver).transpose()
    }

    /// Revokes a readable waiver; one revoked before keeps its time. `None` if not readable.
    pub async fn revoke_waiver(
        &self,
        v: Visibility<'_>,
        id: i64,
        actor: &str,
        at: i64,
    ) -> Result<Option<WaiverEntry>, StorageError> {
        let mut tx = self.write().await?;
        let revoked: Option<i64> = sqlx::query_scalar(concat!(
            "UPDATE waivers SET revoked_at = $3
             WHERE id = $4 AND tenant_id = $1 AND revoked_at IS NULL AND ",
            visible!("repository_id", "$2"),
            " RETURNING repository_id"
        ))
        .bind(v.tenant_id)
        .bind(v.set())
        .bind(at)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(repository_id) = revoked {
            Audit {
                tenant_id: v.tenant_id,
                actor,
                action: "WAIVER_REVOKED",
                target: format!("waiver:{id}"),
                repository_id: Some(repository_id),
                metadata: serde_json::json!({}),
                at,
            }
            .record(&mut tx)
            .await?;
        }

        let row = sqlx::query(concat!(waiver!(), visible!("r.id", "$2"), " AND w.id = $3"))
            .bind(v.tenant_id)
            .bind(v.set())
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
        tx.commit().await?;
        row.as_ref().map(waiver).transpose()
    }
}
