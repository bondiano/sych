//! Standalone `sych schema` subcommands: `check`, `baseline`, `snapshot`,
//! `extract`, `rules`, `explain` and `manifest-schema`.

use std::fs;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use schemars::schema_for;
use sych_adapters::db_schema::DbSchemaError;
use sych_adapters::db_schema::extract::{SourceTree, extract_models, run_external_adapter};
use sych_adapters::db_schema::inspect::{InspectOptions, inspect_dsn};
use sych_adapters::db_schema::sql_parse::{discover_sql_targets, parse_sql_files};
use sych_core::db_schema::rules::{ALL_RULES, evaluate, lookup, run_raw};
use sych_core::db_schema::{
    DEFAULT_SCHEMA_LINT_FILE, DbModelManifest, DbSchemaConfig, DbSchemaReport, DbSchemaSnapshot,
    ModelExtractionSettings, ModelExtractorKind, RuleSpec, SqlDialect, add_baseline,
};

use crate::args::{
    CliExtractor, SchemaBaselineArguments, SchemaCheckArguments, SchemaExplainArguments,
    SchemaExtractArguments, SchemaListFormat, SchemaManifestSchemaArguments, SchemaOutputFormat,
    SchemaRulesArguments, SchemaSnapshotArguments, SchemaSourceArgs,
};
use crate::errors::{EXIT_SCHEMA_FINDINGS, Failure};
use crate::output::{emit, emit_json};
use crate::schema_lint::{
    render_explain_text, render_markdown, render_rules_text, render_sarif, render_text,
};
use crate::storage::block_on;

/// `sych schema check`
pub fn check(args: &SchemaCheckArguments) -> anyhow::Result<ExitCode> {
    let config = load_config(args.config.as_deref(), &args.source.repo)?;
    let snapshot = load_snapshot(&args.source)?;
    let manifest = load_manifest(&args.source, &config)?;
    let report = evaluate(&snapshot, manifest.as_ref(), &config);

    let out = args.output.as_deref();
    match args.format {
        SchemaOutputFormat::Text => emit(&render_text(&report), out)?,
        SchemaOutputFormat::Json => emit_json(&report, out)?,
        SchemaOutputFormat::Sarif => emit_json(&render_sarif(&report), out)?,
        SchemaOutputFormat::Markdown => emit(&render_markdown(&report), out)?,
    }

    if report.passed() {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::from(EXIT_SCHEMA_FINDINGS))
    }
}

/// `sych schema baseline`
pub fn baseline(args: &SchemaBaselineArguments) -> anyhow::Result<ExitCode> {
    let existing = if args.config.exists() {
        fs::read_to_string(&args.config)
            .map_err(|e| Failure::invalid(format!("cannot read {}: {e}", args.config.display())))?
    } else {
        String::new()
    };
    let origin = args.config.display().to_string();
    let config = if existing.trim().is_empty() {
        DbSchemaConfig::default()
    } else {
        DbSchemaConfig::parse_yaml(&origin, &existing).map_err(|errs| Failure::config(&errs))?
    };

    let snapshot = load_snapshot(&args.source)?;
    let manifest = load_manifest(&args.source, &config)?;
    let raw_findings = run_raw(&snapshot, manifest.as_ref(), &config);
    let (updated, added) =
        add_baseline(&origin, &existing, raw_findings).map_err(|errs| Failure::config(&errs))?;

    if args.dry_run {
        emit(&updated, None)?;
    } else {
        if let Some(parent) = args.config.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|e| {
                Failure::no_result(format!("cannot create {}: {e}", parent.display()))
            })?;
        }

        fs::write(&args.config, &updated).map_err(|e| {
            Failure::no_result(format!("cannot write {}: {e}", args.config.display()))
        })?;
        eprintln!(
            "baseline updated ({}): {added} exclusion(s) added",
            args.config.display()
        );
    }

    Ok(ExitCode::SUCCESS)
}

/// `sych schema snapshot`
pub fn snapshot(args: &SchemaSnapshotArguments) -> anyhow::Result<ExitCode> {
    let snap = if let Some(dsn) = &args.dsn {
        let opts = InspectOptions {
            timeout: Duration::from_secs(u64::from(args.timeout.max(1))),
            schemas: args.schemas.clone(),
        };
        block_on(inspect_dsn(dsn, &opts))?.map_err(map_adapter_error)?
    } else {
        load_static_sql_snapshot(&args.repo, &args.sql, args.dialect.map(SqlDialect::from))?
    };
    emit_json(&snap, args.output.as_deref())?;
    Ok(ExitCode::SUCCESS)
}

/// `sych schema extract`
pub fn extract(args: &SchemaExtractArguments) -> anyhow::Result<ExitCode> {
    let config = load_config(args.config.as_deref(), &args.repo)?;
    let manifest = if let Some(adapter) = &args.adapter {
        let limit = Duration::from_secs(u64::from(args.timeout.max(1)));
        block_on(run_external_adapter(adapter, &args.adapter_args, limit))?
            .map_err(map_adapter_error)?
    } else {
        let mut model_settings = config.models;
        if !args.extractors.is_empty() {
            model_settings.extractors = args
                .extractors
                .iter()
                .copied()
                .map(ModelExtractorKind::from)
                .collect();
        }

        let tree = SourceTree::from_dir(&args.repo);
        extract_models(&tree, &model_settings)
    };
    emit_json(&manifest, args.output.as_deref())?;
    Ok(ExitCode::SUCCESS)
}

/// `sych schema rules`
pub fn rules(args: &SchemaRulesArguments) -> anyhow::Result<ExitCode> {
    let dialect = args.dialect.map(SqlDialect::from);
    let list: Vec<_> = ALL_RULES
        .iter()
        .filter(|r| dialect.is_none_or(|d| r.applies_to(d)))
        .map(RuleSpec::info)
        .collect();
    match args.format {
        SchemaListFormat::Text => emit(&render_rules_text(&list), args.output.as_deref())?,
        SchemaListFormat::Json => emit_json(&list, args.output.as_deref())?,
    }
    Ok(ExitCode::SUCCESS)
}

/// `sych schema explain <RULE>`
pub fn explain(args: &SchemaExplainArguments) -> anyhow::Result<ExitCode> {
    let spec = lookup(&args.rule)
        .ok_or_else(|| Failure::invalid(format!("unknown schema rule `{}`", args.rule)))?;
    match args.format {
        SchemaListFormat::Text => emit(&render_explain_text(spec), args.output.as_deref())?,
        SchemaListFormat::Json => emit_json(&spec.info(), args.output.as_deref())?,
    }
    Ok(ExitCode::SUCCESS)
}

/// `sych schema manifest-schema`
pub fn manifest_schema(args: &SchemaManifestSchemaArguments) -> anyhow::Result<ExitCode> {
    let schema = schema_for!(DbModelManifest);
    emit_json(&schema, args.output.as_deref())?;
    Ok(ExitCode::SUCCESS)
}

fn load_config(explicit: Option<&Path>, repo: &Path) -> anyhow::Result<DbSchemaConfig> {
    let path = if let Some(p) = explicit {
        Some(p.to_path_buf())
    } else {
        let default = repo.join(DEFAULT_SCHEMA_LINT_FILE);
        default.is_file().then_some(default)
    };
    let Some(path) = path else {
        return Ok(DbSchemaConfig::default());
    };
    let text = fs::read_to_string(&path)
        .map_err(|e| Failure::invalid(format!("cannot read {}: {e}", path.display())))?;
    DbSchemaConfig::parse_yaml(&path.display().to_string(), &text)
        .map_err(|errs| Failure::config(&errs).into())
}

fn load_snapshot(src: &SchemaSourceArgs) -> anyhow::Result<DbSchemaSnapshot> {
    if let Some(dsn) = &src.dsn {
        let opts = InspectOptions {
            timeout: Duration::from_secs(u64::from(src.timeout.max(1))),
            schemas: src.schemas.clone(),
        };
        return Ok(block_on(inspect_dsn(dsn, &opts))?.map_err(map_adapter_error)?);
    }

    if let Some(path) = &src.snapshot {
        let text = fs::read_to_string(path)
            .map_err(|e| Failure::invalid(format!("cannot read {}: {e}", path.display())))?;
        let snap: DbSchemaSnapshot = serde_json::from_str(&text)
            .map_err(|e| Failure::invalid(format!("invalid snapshot {}: {e}", path.display())))?;
        return Ok(snap);
    }

    load_static_sql_snapshot(&src.repo, &src.sql, src.dialect.map(SqlDialect::from))
}

fn load_static_sql_snapshot(
    repo: &Path,
    sql_globs: &[String],
    dialect_hint: Option<SqlDialect>,
) -> anyhow::Result<DbSchemaSnapshot> {
    let tree = SourceTree::from_dir(repo);
    let mut targets = discover_sql_targets(&tree, sql_globs, dialect_hint);
    if targets.is_empty() {
        return Err(Failure::invalid(format!(
            "no SQL schema or migration files found under {}; pass --dsn, --snapshot or --sql",
            repo.display()
        ))
        .into());
    }

    if let Some(d) = dialect_hint {
        let matching: Vec<_> = targets.iter().filter(|t| t.dialect == d).cloned().collect();
        if !matching.is_empty() {
            targets = matching;
        }
    }

    if targets.len() > 1 {
        let names: Vec<String> = targets
            .iter()
            .map(|t| format!("{} ({})", t.label, t.dialect))
            .collect();
        return Err(Failure::invalid(format!(
            "multiple SQL schema targets found ({}); pass --dialect or --sql to select one",
            names.join(", ")
        ))
        .into());
    }

    let target = targets.remove(0);
    Ok(parse_sql_files(target.dialect, &target.files))
}

fn load_manifest(
    src: &SchemaSourceArgs,
    config: &DbSchemaConfig,
) -> anyhow::Result<Option<DbModelManifest>> {
    if let Some(path) = &src.model_manifest {
        let text = fs::read_to_string(path)
            .map_err(|e| Failure::invalid(format!("cannot read {}: {e}", path.display())))?;
        let manifest = DbModelManifest::parse(&text).map_err(|e| {
            Failure::invalid(format!("invalid model manifest {}: {e}", path.display()))
        })?;
        return Ok(Some(manifest));
    }

    if let Some(adapter) = &src.adapter {
        let limit = Duration::from_secs(u64::from(src.timeout.max(1)));
        let manifest = block_on(run_external_adapter(adapter, &src.adapter_args, limit))?
            .map_err(map_adapter_error)?;
        return Ok(Some(manifest));
    }

    let model_settings = effective_model_settings(&src.extractors, &config.models);
    if model_settings
        .extractors
        .contains(&ModelExtractorKind::None)
    {
        return Ok(None);
    }

    // Only auto-extract when `--sql` / `--repo` is used or `--extractor` was requested explicitly;
    // when checking a live DSN or standalone snapshot without `--extractor`, still run auto-extract
    // if `--repo` has models, returning `None` if zero tables were found.
    let tree = SourceTree::from_dir(&src.repo);
    let extracted = extract_models(&tree, &model_settings);
    Ok((!extracted.tables.is_empty()).then_some(extracted))
}

fn effective_model_settings(
    cli: &[CliExtractor],
    base: &ModelExtractionSettings,
) -> ModelExtractionSettings {
    let mut settings = base.clone();
    if !cli.is_empty() {
        settings.extractors = cli.iter().copied().map(ModelExtractorKind::from).collect();
    }

    settings
}

fn map_adapter_error(err: DbSchemaError) -> Failure {
    match err {
        DbSchemaError::InvalidDsn { .. } | DbSchemaError::SqlParse(_) => Failure::invalid(err),
        other => Failure::no_result(other),
    }
}

/// Validates and evaluates a [`DbSchemaSnapshot`] for the stateless HTTP API.
pub fn evaluate_stateless(
    snapshot: &DbSchemaSnapshot,
    manifest: Option<&DbModelManifest>,
    config: Option<&DbSchemaConfig>,
) -> Result<DbSchemaReport, String> {
    if let Some(m) = manifest {
        m.validate().map_err(|e| e.to_string())?;
    }

    let cfg = match config {
        Some(c) => {
            // Re-validate via YAML round-trip so every rule id, option and exclusion is checked.
            let yaml = serde_yaml_ng::to_string(c).map_err(|e| e.to_string())?;
            DbSchemaConfig::parse_yaml("request.config", &yaml).map_err(|errs| {
                errs.into_iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            })?
        }
        None => DbSchemaConfig::default(),
    };
    Ok(evaluate(snapshot, manifest, &cfg))
}
