//! Local storage: where the database and artifacts live, and reading a saved run back.

use std::env;
use std::future::Future;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::de::DeserializeOwned;
use sych_core::broker::ContextPackets;
use sych_core::config::ServerConfig;
use sych_core::{EvidenceManifest, RunFacts, RunId};
use sych_storage::{Database, Storage, StorageError, StoreConfig, TENANT_LOCAL};
use tokio::runtime::Builder;
use url::Url;

use crate::config;
use crate::errors::Failure;
use crate::render::Report;

const DEFAULT_DATABASE: &str = ".sych-data/sych.db";
const DEFAULT_ARTIFACTS: &str = ".sych-data/artifacts";

/// Runs one future on a single-threaded runtime.
pub fn block_on<F: Future>(future: F) -> anyhow::Result<F::Output> {
    Builder::new_current_thread()
        .enable_all()
        .build()
        .map(|runtime| runtime.block_on(future))
        .map_err(|e| Failure::no_result(format!("cannot start runtime: {e}")).into())
}

/// `SQLite` and artifact paths are relative to `sych.yaml`; without a config they default
/// under `./.sych-data`. `PostgreSQL` URLs are passed to the database driver unchanged.
pub fn store_config(config: Option<&Path>, server: &ServerConfig) -> anyhow::Result<StoreConfig> {
    let base = config
        .and_then(Path::parent)
        .map_or_else(PathBuf::new, Path::to_path_buf);
    let database = match &server.storage.database_url_env {
        Some(name) => {
            let url = env::var(name.as_str()).map_err(|_| {
                Failure::invalid(format!(
                    "{} is not set (storage.database_url_env)",
                    name.as_str()
                ))
            })?;
            database(&url, &base)?
        }
        None => Database::Sqlite(base.join(DEFAULT_DATABASE)),
    };
    let artifacts = server
        .storage
        .artifact_store
        .as_ref()
        .map_or_else(|| base.join(DEFAULT_ARTIFACTS), |s| base.join(&s.path));
    Ok(StoreConfig {
        database,
        artifacts,
    })
}

fn database(value: &str, base: &Path) -> anyhow::Result<Database> {
    let invalid = || Failure::invalid("invalid or unsupported storage database URL");
    if value.trim().is_empty() {
        return Err(invalid().into());
    }

    if value.starts_with("postgres://") || value.starts_with("postgresql://") {
        let url = Url::parse(value).map_err(|_| invalid())?;
        if url.host_str().is_none_or(str::is_empty) || url.fragment().is_some() {
            return Err(invalid().into());
        }

        return Ok(Database::Postgres(value.to_owned()));
    }

    let path = value
        .strip_prefix("sqlite://")
        .or_else(|| value.strip_prefix("sqlite:"));
    match path {
        Some("") => Err(invalid().into()),
        Some(path) => Ok(Database::Sqlite(base.join(path))),
        None if Url::parse(value).is_ok() || value.contains("://") => Err(invalid().into()),
        None => Ok(Database::Sqlite(base.join(value))),
    }
}

/// Storage location from the server config at `config`, or engine defaults.
pub fn load_store_config(config: Option<&Path>) -> anyhow::Result<StoreConfig> {
    store_config(config, &config::server(config)?)
}

/// Whether the configured database opens at the current schema; never migrates.
pub fn database_status(config: &Path, server: &ServerConfig) -> Result<String, String> {
    let store = store_config(Some(config), server).map_err(|e| format!("{e:#}"))?;
    // The pool is dropped inside its runtime.
    match block_on(async { Storage::open(&store).await.map(drop) }) {
        Ok(Ok(())) => Ok(format!("{} is migrated", store.database)),
        Ok(Err(err)) => Err(err.to_string()),
        Err(err) => Err(format!("{err:#}")),
    }
}

async fn load<T: DeserializeOwned>(
    (storage, tenant): (&Storage, &str),
    run: RunId,
    role: &str,
) -> Result<T, StorageError> {
    storage.load_artifact(tenant, run, role).await
}

/// Source data has a shorter retention than the result; once pruned it reads as `None`.
async fn load_source<T: DeserializeOwned>(
    storage: (&Storage, &str),
    run: RunId,
    role: &str,
) -> Result<Option<T>, StorageError> {
    match load(storage, run, role).await {
        Err(StorageError::ArtifactUnavailable(_)) => Ok(None),
        loaded => loaded.map(Some),
    }
}

async fn load_report(store: StoreConfig, run: RunId) -> Result<Report, StorageError> {
    report_of(&Storage::open(&store).await?, TENANT_LOCAL, run).await
}

/// Reads every artifact of a completed run of `tenant`; no analysis and no model call.
pub async fn report_of(
    storage: &Storage,
    tenant: &str,
    run: RunId,
) -> Result<Report, StorageError> {
    let review_result = storage.load_result(tenant, run).await?;
    let storage = (storage, tenant);
    let evidence_manifest: EvidenceManifest = load(storage, run, "evidence_manifest").await?;
    let mut evidence = Vec::with_capacity(evidence_manifest.evidence.len());
    for record in &evidence_manifest.evidence {
        evidence.push(load(storage, run, &format!("evidence/{}", record.evidence_id)).await?);
    }

    let facts: RunFacts = load(storage, run, "facts").await?;
    let packets: ContextPackets = load(storage, run, "context_packets").await?;
    Ok(Report {
        snapshot: load_source(storage, run, "source_snapshot").await?,
        diff_manifest: load_source(storage, run, "diff_manifest").await?,
        policy: load(storage, run, "policy_snapshot").await?,
        policy_review: load(storage, run, "policy_review").await?,
        external_input_manifest: load(storage, run, "external_input_manifest").await?,
        evidence_manifest,
        evidence,
        semantic_facts: facts.semantic,
        syntax_facts: facts.syntax,
        ci_facts: facts.ci,
        effect_ledger: facts.ledger,
        lint_facts: facts.lint,
        context_packets: packets.packets,
        review_result,
    })
}

/// The saved run `run` from the store configured at `config`.
pub fn saved_report(config: Option<&Path>, run: &str) -> anyhow::Result<Report> {
    let run = run
        .parse()
        .map(RunId)
        .map_err(|_| Failure::invalid(format!("`{run}` is not a run id")))?;
    let store = load_store_config(config)?;
    block_on(load_report(store, run))?.with_context(|| format!("cannot read saved run {run}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_selection_preserves_local_paths_and_redacts_remote_credentials() {
        let base = Path::new("/config");
        for value in ["history.db", "sqlite:history.db", "sqlite://history.db"] {
            assert!(matches!(
                database(value, base).expect("SQLite"),
                Database::Sqlite(path) if path == base.join("history.db")
            ));
        }
        assert!(matches!(
            database("sqlite:///data/history.db", base).expect("absolute"),
            Database::Sqlite(path) if path == Path::new("/data/history.db")
        ));
        for scheme in ["postgres", "postgresql"] {
            let value = format!("{scheme}://sych:private-password@db/sych?sslmode=require");
            let selected = database(&value, base).expect("PostgreSQL");
            assert!(matches!(&selected, Database::Postgres(url) if url == &value));
            assert!(!format!("{selected:?} {selected}").contains("private-password"));
        }
        for value in [
            "",
            "  ",
            "sqlite:",
            "sqlite://",
            "postgres://",
            "postgresql://user:private-password@/db",
            "postgres://user:private-password@host:bad/db",
            "postgres://user:private-password@db/sych#fragment",
            "mysql://user:private-password@db/sych",
            "postgres:history",
        ] {
            let error = database(value, base).expect_err("invalid URL").to_string();
            assert!(!error.contains("private-password"), "{error}");
        }
    }
}
