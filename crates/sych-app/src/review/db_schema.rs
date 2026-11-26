//! Database and application schema fact collection for one review run.

use std::path::Path;
use std::time::Duration;
use std::{env, fs};

use sych_adapters::db_schema::collect::{ReviewCollectOptions, collect_review_facts};
use sych_adapters::git::LocalSnapshot;
use sych_core::config::{EffectiveConfig, RunMode};
use sych_core::db_schema::{DbModelManifest, DbSchemaFacts, DbSchemaSource, SqlDialect};
use sych_core::{BackendStatus, CiFacts, Evidence, Producer, SchemaVersion, Timestamp};

use crate::args::Switch;
use crate::errors::Failure;
use crate::storage;

use super::Request;

/// Collects [`DbSchemaFacts`] and partitions their evidence into derived (static SQL)
/// and external (live DB / CI snapshot, frozen before analysis).
pub(super) fn collect_evidence(
    args: &Request,
    snap: &LocalSnapshot,
    effective: &EffectiveConfig,
    ci: &[CiFacts],
    now: Timestamp,
) -> anyhow::Result<(Vec<DbSchemaFacts>, Vec<Evidence>, Vec<Evidence>)> {
    let facts = collect(args, snap, effective, ci)?;
    let mut derived = Vec::new();
    let mut external = Vec::new();
    for fact in &facts {
        let ev = fact
            .to_evidence(&snap.snapshot, now)
            .map_err(|err| Failure::no_result(format!("cannot hash facts: {err}")))?;
        if fact.source == DbSchemaSource::StaticSql {
            derived.push(ev);
        } else {
            external.push(ev);
        }
    }
    Ok((facts, derived, external))
}

/// Collects deterministic [`DbSchemaFacts`] when schema linting is enabled for `args`.
fn collect(
    args: &Request,
    snap: &LocalSnapshot,
    effective: &EffectiveConfig,
    ci: &[CiFacts],
) -> anyhow::Result<Vec<DbSchemaFacts>> {
    let settings = &effective.repository.config.schema_lint;
    let enabled = match args.schema_lint {
        Some(Switch::Off) => false,
        Some(Switch::On) => true,
        None => {
            settings.enabled || (args.run_mode == RunMode::PersonalLocal && args.db_dsn.is_some())
        }
    };
    if !enabled {
        return Ok(Vec::new());
    }

    let explicit_manifest = if args.run_mode == RunMode::PersonalLocal {
        args.db_model_manifest
            .as_deref()
            .map(read_model_manifest)
            .transpose()?
    } else {
        None
    };

    let (live_dsn, live_label, timeout) = match resolve_live_connection(args, snap, effective) {
        Ok(conn) => conn,
        Err(failed) => return Ok(vec![*failed]),
    };

    let schema_config = effective.repository.effective_schema_lint_config();
    let opts = ReviewCollectOptions {
        settings,
        config: &schema_config,
        ci,
        live_dsn: live_dsn.as_deref(),
        live_label: live_label.as_deref(),
        timeout,
        explicit_manifest: explicit_manifest.as_ref(),
    };
    storage::block_on(collect_review_facts(snap, &opts))
}

fn read_model_manifest(path: &Path) -> anyhow::Result<DbModelManifest> {
    let text = fs::read_to_string(path).map_err(|e| {
        Failure::invalid(format!(
            "cannot read --db-model-manifest {}: {e}",
            path.display()
        ))
    })?;
    DbModelManifest::parse(&text).map_err(|e| {
        Failure::invalid(format!(
            "invalid --db-model-manifest {}: {e}",
            path.display()
        ))
        .into()
    })
}

fn resolve_live_connection(
    args: &Request,
    snap: &LocalSnapshot,
    effective: &EffectiveConfig,
) -> Result<(Option<String>, Option<String>, Duration), Box<DbSchemaFacts>> {
    if args.run_mode == RunMode::PersonalLocal
        && let Some(dsn) = &args.db_dsn
    {
        return Ok((
            Some(dsn.clone()),
            Some("live:dsn".to_owned()),
            ReviewCollectOptions::default_timeout(),
        ));
    }

    let Some(conn_name) = &effective.repository.config.schema_lint.connection else {
        return Ok((None, None, ReviewCollectOptions::default_timeout()));
    };
    let dialect = effective
        .repository
        .config
        .schema_lint
        .dialect
        .unwrap_or(SqlDialect::Postgres);
    let fail = |msg: String| Box::new(failed_schema_connection(snap, conn_name, dialect, msg));

    let Some(conn) = effective.server.schema_lint.connections.get(conn_name) else {
        return Err(fail(format!(
            "server config does not define schema_lint connection `{conn_name}`"
        )));
    };
    if args.run_mode != RunMode::PersonalLocal {
        let allowed = args
            .repository_full_name
            .as_deref()
            .is_some_and(|repo| conn.repositories.iter().any(|r| r == repo));
        if !allowed {
            return Err(fail(format!(
                "repository is not allowed to use server schema_lint connection `{conn_name}`"
            )));
        }
    }

    let dsn = env::var(conn.dsn_env.as_str())
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| {
            fail(format!(
                "environment variable `{}` for schema_lint connection `{conn_name}` is not set",
                conn.dsn_env.as_str()
            ))
        })?;
    Ok((
        Some(dsn),
        Some(format!("live:{conn_name}")),
        Duration::from_secs(u64::from(conn.timeout_seconds.max(1))),
    ))
}

fn failed_schema_connection(
    snap: &LocalSnapshot,
    conn_name: &str,
    dialect: SqlDialect,
    message: String,
) -> DbSchemaFacts {
    DbSchemaFacts {
        schema_version: SchemaVersion,
        source_snapshot_id: snap.snapshot.id.clone(),
        producer: Producer {
            name: "sych-db-schema".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            config_hash: None,
        },
        target: format!("live:{conn_name}"),
        dialect,
        source: DbSchemaSource::LiveDatabase,
        model_extractor: None,
        status: BackendStatus::Failed,
        base_checked: false,
        findings: Vec::new(),
        fixed: Vec::new(),
        unmatched_exclusions: Vec::new(),
        warnings: Vec::new(),
        skipped_rules: Vec::new(),
        limitations: vec![message],
    }
}
