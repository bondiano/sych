//! Looking up saved results across revisions of a change.

use sqlx::Row;
use sych_core::{ResultId, RunId};

use crate::{Storage, StorageError};

impl Storage {
    /// The run that saved `result` for this tenant, if any.
    pub async fn run_of_result(
        &self,
        tenant_id: &str,
        result: ResultId,
    ) -> Result<Option<RunId>, StorageError> {
        let row = sqlx::query("SELECT id FROM runs WHERE result_id = $1 AND tenant_id = $2")
            .bind(result.to_string())
            .bind(tenant_id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|r| {
            let id: String = r.get(0);
            id.parse()
                .map(RunId)
                .map_err(|_| StorageError::Corrupt(format!("run id {id}")))
        })
        .transpose()
    }
}
