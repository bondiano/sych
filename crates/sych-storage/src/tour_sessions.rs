//! Stored tours of a run and the personal sessions through them (spec §21.8, roadmap M5.2). A
//! session is one actor's row; nothing here touches the tour artifact or the result.

use sqlx::Row;
use sych_core::analysis::followup::Exchange;
use sych_core::tour::{SessionState, TourSession};
use sych_core::{RunId, Timestamp, TourId};

use crate::{Storage, StorageError};

/// Role a tour is pinned to its run under.
pub fn tour_role(tour: TourId) -> String {
    format!("tour:{tour}")
}

fn state_str(s: SessionState) -> &'static str {
    match s {
        SessionState::InProgress => "IN_PROGRESS",
        SessionState::Completed => "COMPLETED",
        SessionState::Dismissed => "DISMISSED",
    }
}

impl Storage {
    /// The newest tour of a run; ids are `UUIDv7`, so their order is the creation order.
    pub async fn latest_tour(
        &self,
        tenant_id: &str,
        run: RunId,
    ) -> Result<Option<TourId>, StorageError> {
        let row = sqlx::query(
            "SELECT substr(role, 6) FROM run_artifacts
             WHERE run_id = $1 AND tenant_id = $2 AND role LIKE 'tour:%' ORDER BY role DESC LIMIT 1",
        )
        .bind(run.to_string())
        .bind(tenant_id)
        .fetch_optional(self.pool())
        .await?;
        row.map(|r| {
            let id: String = r.get(0);
            id.parse()
                .map(TourId)
                .map_err(|_| StorageError::Corrupt(format!("tour id `{id}`")))
        })
        .transpose()
    }

    /// The run pins this tour; a tour of another run or tenant is not its.
    pub async fn has_tour(
        &self,
        tenant_id: &str,
        run: RunId,
        tour: TourId,
    ) -> Result<bool, StorageError> {
        Ok(sqlx::query(
            "SELECT 1 FROM run_artifacts WHERE run_id = $1 AND tenant_id = $2 AND role = $3",
        )
        .bind(run.to_string())
        .bind(tenant_id)
        .bind(tour_role(tour))
        .fetch_optional(self.pool())
        .await?
        .is_some())
    }

    pub async fn tour_session(
        &self,
        tenant_id: &str,
        tour: TourId,
        actor: &str,
    ) -> Result<Option<TourSession>, StorageError> {
        let row = sqlx::query(
            "SELECT state, revealed FROM tour_sessions
             WHERE tenant_id = $1 AND tour_id = $2 AND actor = $3",
        )
        .bind(tenant_id)
        .bind(tour.to_string())
        .bind(actor)
        .fetch_optional(self.pool())
        .await?;
        row.map(|r| {
            let (state, revealed): (String, String) = (r.get(0), r.get(1));
            let corrupt = |e: serde_json::Error| StorageError::Corrupt(e.to_string());
            Ok(TourSession {
                tour_id: tour,
                actor: actor.to_owned(),
                state: serde_json::from_value(serde_json::Value::String(state)).map_err(corrupt)?,
                revealed: serde_json::from_str(&revealed).map_err(corrupt)?,
            })
        })
        .transpose()
    }

    pub async fn save_tour_session(
        &self,
        tenant_id: &str,
        run: RunId,
        session: &TourSession,
        now: Timestamp,
    ) -> Result<(), StorageError> {
        let revealed = serde_json::to_string(&session.revealed)
            .map_err(|e| StorageError::Corrupt(e.to_string()))?;
        let mut tx = self.write().await?;
        sqlx::query(
            "INSERT INTO tour_sessions (tenant_id, tour_id, actor, run_id, state, revealed, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (tenant_id, tour_id, actor)
             DO UPDATE SET state = excluded.state, revealed = excluded.revealed,
                 updated_at = excluded.updated_at",
        )
        .bind(tenant_id)
        .bind(session.tour_id.to_string())
        .bind(&session.actor)
        .bind(run.to_string())
        .bind(state_str(session.state))
        .bind(revealed)
        .bind(now.as_second())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// One exchange of a session; only its actor reads it back.
    pub async fn add_exchange(
        &self,
        tenant_id: &str,
        run: RunId,
        exchange: &Exchange,
    ) -> Result<(), StorageError> {
        let mut tx = self.write().await?;
        sqlx::query(
            "INSERT INTO tour_exchanges (id, tenant_id, tour_id, actor, run_id, body, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&exchange.id)
        .bind(tenant_id)
        .bind(exchange.tour_id.to_string())
        .bind(&exchange.actor)
        .bind(run.to_string())
        .bind(serde_json::to_string(exchange)?)
        .bind(exchange.at.as_second())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The actor's own exchanges on a tour, oldest first; never another actor's.
    pub async fn exchanges(
        &self,
        tenant_id: &str,
        tour: TourId,
        actor: &str,
    ) -> Result<Vec<Exchange>, StorageError> {
        let rows = sqlx::query(
            "SELECT body FROM tour_exchanges WHERE tenant_id = $1 AND tour_id = $2 AND actor = $3
             ORDER BY created_at, id",
        )
        .bind(tenant_id)
        .bind(tour.to_string())
        .bind(actor)
        .fetch_all(self.pool())
        .await?;
        rows.iter()
            .map(|r| Ok(serde_json::from_str(&r.get::<String, _>(0))?))
            .collect()
    }
}
