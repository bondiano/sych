//! Retention inside `sych serve`: the pass `sych cache prune` runs by hand, on a timer. Every
//! instance runs it; the storage write lock serializes the deletes and they are idempotent,
//! while mirrors live on each instance's own disk.

use super::engine::Engine;
use super::mirrors;
use super::worker::blocking;

impl Engine {
    /// Unpinned artifacts, records past `storage.retention` and mirrors unused for a week.
    pub async fn retain_once(&self) -> anyhow::Result<()> {
        let storage = &self.app.storage;
        let now = (self.now)();
        let report = storage.prune(now, false).await?;
        let records = storage.prune_records(now, &self.retention, false).await?;
        let active = storage.active_repositories().await?;
        let artifacts = self.store.artifacts.clone();
        let gone =
            blocking(move || mirrors::prune(&artifacts, &active, now.as_second(), false)).await??;
        tracing::info!(
            artifacts = report.deleted,
            records = records.iter().map(|r| r.rows).sum::<i64>(),
            mirrors = gone.len(),
            "retention pass"
        );
        Ok(())
    }
}
