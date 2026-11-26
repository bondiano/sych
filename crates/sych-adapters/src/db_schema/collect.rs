//! Review-pipeline database and application schema fact collector.
//!
//! Coordinates live read-only inspection (optional), bound CI snapshot/manifest
//! artifacts, static SQL DDL reconstruction (HEAD and BASE) and built-in static
//! ORM model extraction (HEAD and BASE), producing [`DbSchemaFacts`] with
//! resolved HEAD [`CodeRef`] anchors and honest limitations.

use std::time::Duration;

use sych_core::db_schema::{
    DEFAULT_INSPECT_TIMEOUT_SECONDS, DbModelManifest, DbSchemaConfig, DbSchemaFacts,
    DbSchemaFinding, DbSchemaSnapshot, DbSchemaSource, DbSourcePosition, SchemaLintSettings,
    SqlDialect, diff_findings, rules,
};
use sych_core::{
    BackendStatus, CiBindingKind, CiFacts, CodeRef, Producer, RepoPath, SchemaVersion, SourceSide,
};

use super::extract::{SourceTree, extract_models};
use super::inspect::{InspectOptions, inspect_dsn};
use super::sql_parse::{discover_sql_targets, parse_sql_files};
use crate::git::LocalSnapshot;

const PRODUCER_NAME: &str = "sych-db-schema";

/// Options for collecting [`DbSchemaFacts`] inside a review run.
pub struct ReviewCollectOptions<'a> {
    pub settings: &'a SchemaLintSettings,
    pub config: &'a DbSchemaConfig,
    /// Bound CI facts imported for this run (checked for `db_schema_snapshot` / `db_model_manifest`).
    pub ci: &'a [CiFacts],
    /// Optional live read-only DSN (from CLI `--db-dsn` or allowed server `schema_lint.connections`).
    pub live_dsn: Option<&'a str>,
    /// Label for the live connection (e.g. `live:staging` or `live:dsn`).
    pub live_label: Option<&'a str>,
    /// Timeout for live read-only inspection.
    pub timeout: Duration,
    /// Optional explicit model manifest (local CLI `--db-model-manifest`).
    pub explicit_manifest: Option<&'a DbModelManifest>,
}

impl ReviewCollectOptions<'_> {
    pub fn default_timeout() -> Duration {
        Duration::from_secs(u64::from(DEFAULT_INSPECT_TIMEOUT_SECONDS))
    }
}

/// Collects deterministic [`DbSchemaFacts`] for a review snapshot when schema linting is enabled.
///
/// Database source priority:
/// 1. Explicit live read-only DSN (CLI `--db-dsn` or trusted server connection); fails closed on
///    connection/inspection error rather than silently falling back to a different source.
/// 2. Bound CI `db_schema_snapshot` artifact
/// 3. Static SQL DDL reconstruction from HEAD (and BASE for diffing)
///
/// Model source priority:
/// 1. Bound CI `db_model_manifest` artifact
/// 2. Explicit CLI `--db-model-manifest`
/// 3. Built-in static ORM extractors over HEAD (and BASE when comparing static SQL)
pub async fn collect_review_facts(
    snapshot: &LocalSnapshot,
    options: &ReviewCollectOptions<'_>,
) -> Vec<DbSchemaFacts> {
    let head_tree = SourceTree::from_snapshot(snapshot, SourceSide::Head);
    let head_manifest = resolve_head_manifest(snapshot, &head_tree, options);

    // 1. Live read-only DB inspection (fail closed if configured and unreachable).
    if let Some(dsn) = options.live_dsn {
        let target = options.live_label.unwrap_or("live:database").to_owned();
        let inspect_opts = InspectOptions {
            timeout: options.timeout,
            schemas: Vec::new(),
        };
        let facts = match inspect_dsn(dsn, &inspect_opts).await {
            Ok(db_snap) => evaluate_target(
                snapshot,
                &target,
                &db_snap,
                false,
                head_manifest.as_ref(),
                None,
                options.config,
            ),
            Err(err) => failed_facts(
                snapshot,
                &target,
                super::detect_dialect(dsn)
                    .ok()
                    .or(options.settings.dialect)
                    .unwrap_or(SqlDialect::Postgres),
                DbSchemaSource::LiveDatabase,
                err.to_string(),
            ),
        };
        return vec![facts];
    }

    // 2. Bound CI `db_schema_snapshot` artifact.
    if let Some((ci_fact, ci_snap)) =
        find_ci_snapshot(snapshot, options.ci, options.settings.ci_report.as_deref())
    {
        let target = options
            .settings
            .ci_report
            .as_deref()
            .map_or_else(|| "ci:db-schema".to_owned(), |id| format!("ci:{id}"));
        let authenticated = ci_fact
            .origin
            .as_ref()
            .is_some_and(|o| o.binding != CiBindingKind::Unverified);
        let facts = evaluate_target(
            snapshot,
            &target,
            ci_snap,
            authenticated,
            head_manifest.as_ref(),
            None,
            options.config,
        );
        return vec![facts];
    }

    // 3. Static SQL DDL reconstruction (HEAD vs BASE).
    collect_static_sql(snapshot, &head_tree, head_manifest.as_ref(), options)
}

fn collect_static_sql(
    snapshot: &LocalSnapshot,
    head_tree: &SourceTree<'_>,
    head_manifest: Option<&DbModelManifest>,
    options: &ReviewCollectOptions<'_>,
) -> Vec<DbSchemaFacts> {
    let head_targets = discover_sql_targets(
        head_tree,
        &options.settings.sql_paths,
        options.settings.dialect,
    );
    if head_targets.is_empty() {
        return Vec::new();
    }

    let base_tree = SourceTree::from_snapshot(snapshot, SourceSide::DiffBase);
    let base_targets = discover_sql_targets(
        &base_tree,
        &options.settings.sql_paths,
        options.settings.dialect,
    );
    let base_manifest = if options.explicit_manifest.is_none()
        && find_ci_manifest(
            snapshot,
            options.ci,
            options.settings.model_ci_report.as_deref(),
        )
        .is_none()
    {
        let m = extract_models(&base_tree, &options.config.models);
        (!m.tables.is_empty()).then_some(m)
    } else {
        head_manifest.cloned()
    };

    let mut out = Vec::new();
    for target in head_targets {
        let head_snap = parse_sql_files(target.dialect, &target.files);
        let base_findings = base_targets
            .iter()
            .find(|bt| bt.label == target.label && bt.dialect == target.dialect)
            .map(|bt| {
                let base_snap = parse_sql_files(bt.dialect, &bt.files);
                rules::evaluate(&base_snap, base_manifest.as_ref(), options.config).findings
            });
        out.push(evaluate_target(
            snapshot,
            &target.label,
            &head_snap,
            false,
            head_manifest,
            base_findings.as_deref(),
            options.config,
        ));
    }
    out
}

fn resolve_head_manifest(
    snapshot: &LocalSnapshot,
    head_tree: &SourceTree<'_>,
    options: &ReviewCollectOptions<'_>,
) -> Option<DbModelManifest> {
    if let Some(m) = find_ci_manifest(
        snapshot,
        options.ci,
        options.settings.model_ci_report.as_deref(),
    ) {
        return Some(m.clone());
    }

    if let Some(m) = options.explicit_manifest {
        return Some(m.clone());
    }

    let extracted = extract_models(head_tree, &options.config.models);
    (!extracted.tables.is_empty()).then_some(extracted)
}

fn find_ci_snapshot<'a>(
    snapshot: &LocalSnapshot,
    ci: &'a [CiFacts],
    report_hint: Option<&str>,
) -> Option<(&'a CiFacts, &'a DbSchemaSnapshot)> {
    ci.iter()
        .filter(|c| c.bound_to(&snapshot.snapshot))
        .filter(|c| report_hint.is_none_or(|h| c.report.contains(h)))
        .find_map(|c| c.db_schema_snapshot.as_deref().map(|snap| (c, snap)))
}

fn find_ci_manifest<'a>(
    snapshot: &LocalSnapshot,
    ci: &'a [CiFacts],
    report_hint: Option<&str>,
) -> Option<&'a DbModelManifest> {
    ci.iter()
        .filter(|c| c.bound_to(&snapshot.snapshot))
        .filter(|c| report_hint.is_none_or(|h| c.report.contains(h)))
        .find_map(|c| c.db_model_manifest.as_deref())
}

fn evaluate_target(
    snapshot: &LocalSnapshot,
    target: &str,
    head_snap: &DbSchemaSnapshot,
    ci_authenticated: bool,
    head_manifest: Option<&DbModelManifest>,
    base_findings: Option<&[DbSchemaFinding]>,
    config: &DbSchemaConfig,
) -> DbSchemaFacts {
    let report = rules::evaluate(head_snap, head_manifest, config);
    let anchored: Vec<(DbSchemaFinding, Option<CodeRef>)> = report
        .findings
        .into_iter()
        .map(|f| {
            let code_ref = f
                .source
                .as_ref()
                .and_then(|pos| resolve_code_ref(snapshot, pos));
            (f, code_ref)
        })
        .collect();
    let (findings, fixed) = diff_findings(base_findings, anchored);

    DbSchemaFacts {
        schema_version: SchemaVersion,
        source_snapshot_id: snapshot.snapshot.id.clone(),
        producer: Producer {
            name: PRODUCER_NAME.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            config_hash: None,
        },
        target: target.to_owned(),
        dialect: head_snap.dialect,
        source: head_snap.source,
        ci_authenticated,
        model_extractor: head_manifest.and_then(|m| m.extractor.clone()),
        status: BackendStatus::Ready,
        base_checked: base_findings.is_some(),
        findings,
        fixed,
        unmatched_exclusions: report.unmatched_exclusions,
        warnings: report.warnings,
        skipped_rules: report.skipped_rules,
        limitations: report.limitations,
    }
}

fn failed_facts(
    snapshot: &LocalSnapshot,
    target: &str,
    dialect: SqlDialect,
    source: DbSchemaSource,
    error: String,
) -> DbSchemaFacts {
    DbSchemaFacts {
        schema_version: SchemaVersion,
        source_snapshot_id: snapshot.snapshot.id.clone(),
        producer: Producer {
            name: PRODUCER_NAME.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            config_hash: None,
        },
        target: target.to_owned(),
        dialect,
        source,
        ci_authenticated: false,
        model_extractor: None,
        status: BackendStatus::Failed,
        base_checked: false,
        findings: Vec::new(),
        fixed: Vec::new(),
        unmatched_exclusions: Vec::new(),
        warnings: Vec::new(),
        skipped_rules: Vec::new(),
        limitations: vec![error],
    }
}

fn resolve_code_ref(snapshot: &LocalSnapshot, pos: &DbSourcePosition) -> Option<CodeRef> {
    let rp = pos.file.parse::<RepoPath>().ok()?;
    let line = pos.line.max(1);
    snapshot.code_ref(SourceSide::Head, &rp, line, line).ok()
}
