//! Opt-in contracts against a disposable, real `PostgreSQL` cluster; never a production database.

#[path = "postgres/consumers.rs"]
mod consumers;
#[path = "postgres/import.rs"]
mod import;
#[path = "support/import.rs"]
mod import_support;
#[path = "postgres/races.rs"]
mod races;
mod support;

use std::fs;
use std::path::{Path, PathBuf};

use import_support::{PgFixture, inspect, seed_rows};
use sych_core::config::{BillingKind, Budget, Enforcement};
use sych_core::model::{AttemptKind, Capabilities, Limits, Purpose, Reservation, Route};
use sych_core::{
    AttemptId, MicroUsd, PolicySnapshotId, ReviewResult, Sha256Digest, SourceSnapshotId, Timestamp,
    decode, sha256,
};
use sych_storage::{
    ArtifactKind, ArtifactStore, Database, Partition, RunHandle, Storage, StoreConfig,
};
use tempfile::TempDir;

use support::at;

const T0: i64 = 1_800_000_000;

struct Source {
    _dir: TempDir,
    config: StoreConfig,
    run: RunHandle,
    result: ReviewResult,
    digest: Sha256Digest,
}

impl Source {
    async fn seeded() -> Self {
        let dir = tempfile::tempdir().expect("source directory");
        let config = StoreConfig {
            database: Database::Sqlite(dir.path().join("sych.db")),
            artifacts: dir.path().join("artifacts"),
        };
        Storage::migrate(&config)
            .await
            .expect("migrate source SQLite");
        let storage = Storage::open(&config).await.expect("open source SQLite");
        let now = Timestamp::from_second(T0).expect("timestamp");
        let run = storage
            .begin_run(
                &Partition {
                    tenant_id: "import-tenant".into(),
                    access_scope: "repo:o/r".into(),
                },
                "o/r#7",
                &SourceSnapshotId(sha256(b"snapshot")),
                &PolicySnapshotId(sha256(b"policy")),
                now,
            )
            .await
            .expect("begin source run");
        let text = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/core/v1/review-result.json"),
        )
        .expect("review fixture");
        let mut result: ReviewResult = decode(&text).expect("decode review fixture");
        result.run_id = run.id;
        result.review_revision = run.generation;
        storage
            .put_artifact(
                &run,
                "source_snapshot",
                ArtifactKind::SourceSnapshot,
                &"snapshot",
                now,
            )
            .await
            .expect("real snapshot artifact");
        let digest = storage
            .put_artifact(
                &run,
                "review_result",
                ArtifactKind::ReviewResult,
                &result,
                now,
            )
            .await
            .expect("real result artifact");
        storage
            .complete_run(&run, &result, &digest, now)
            .await
            .expect("complete source run");
        drop(storage);

        let pool = inspect(&config).await;
        seed_rows(&pool, &run, &result, T0).await;
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&pool)
            .await
            .expect("checkpoint source snapshot");
        pool.close().await;
        Self {
            _dir: dir,
            config,
            run,
            result,
            digest,
        }
    }

    fn path(&self) -> &Path {
        match &self.config.database {
            Database::Sqlite(path) => path,
            Database::Postgres(_) => unreachable!("SQLite source"),
        }
    }

    fn result_path(&self) -> PathBuf {
        self.config
            .artifacts
            .join(ArtifactStore::key(&self.run.partition, &self.digest))
    }

    fn target(&self, pg: &PgFixture) -> StoreConfig {
        StoreConfig {
            database: pg.config.database.clone(),
            artifacts: self.config.artifacts.clone(),
        }
    }
}

fn reservation(run: &RunHandle, money: u64) -> Reservation {
    Reservation {
        attempt_id: AttemptId::new_v7(),
        tenant_id: run.partition.tenant_id.clone(),
        run_id: run.id,
        purpose: Purpose::Analysis,
        kind: AttemptKind::Initial,
        template: "discovery@1".into(),
        route: Route {
            profile: "cheap".into(),
            provider: "openrouter".into(),
            adapter: "test".into(),
            model: "m".into(),
            billing: BillingKind::ApiUsage,
            enforcement: Enforcement::ApiHard,
            capabilities: Capabilities {
                structured_output: true,
                context_tokens: None,
                output_cap_enforced: true,
                usage_reported: true,
                cost_reported: true,
                hidden_calls: false,
            },
        },
        money: MicroUsd(money),
        input_tokens: 10,
        output_tokens: Some(10),
        fixed_fee: None,
        at: at(1),
    }
}

fn limits(run: u64, month: Option<u64>) -> Limits {
    Limits {
        run: Budget {
            max_cost_micro_usd: Some(MicroUsd(run)),
            ..Budget::default()
        },
        daily_money: None,
        monthly_money: month.map(MicroUsd),
    }
}
