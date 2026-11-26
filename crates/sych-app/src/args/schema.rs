//! Command-line arguments for `sych schema` subcommands.

use std::path::PathBuf;

use clap::{Subcommand, ValueEnum};
use sych_core::db_schema::{DEFAULT_INSPECT_TIMEOUT_SECONDS, ModelExtractorKind, SqlDialect};

/// Standalone database and application schema linter commands.
#[derive(Subcommand)]
pub enum SchemaCommands {
    /// Check a database schema (live read-only DB, CI snapshot or static SQL) against rules and
    /// an optional application model (exit 0 clean, 1 findings/stale exclusions, 2 invalid input)
    Check(Box<SchemaCheckArguments>),
    /// Append current findings as exact exclusions with a baseline reason to the schema-lint config
    Baseline(Box<SchemaBaselineArguments>),
    /// Export a normalized database schema snapshot JSON (no row data, no credentials)
    Snapshot(SchemaSnapshotArguments),
    /// Extract a normalized application model manifest JSON from repository ORMs or a local adapter
    Extract(SchemaExtractArguments),
    /// List all built-in database and application schema rules
    Rules(SchemaRulesArguments),
    /// Show detailed rationale, fix guidance and valid exceptions for one rule
    Explain(SchemaExplainArguments),
    /// Print the JSON Schema for the application model manifest format
    ManifestSchema(SchemaManifestSchemaArguments),
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SchemaOutputFormat {
    Text,
    Json,
    Sarif,
    Markdown,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SchemaListFormat {
    Text,
    Json,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliDialect {
    Postgres,
    Mysql,
    Sqlite,
}

impl From<CliDialect> for SqlDialect {
    fn from(d: CliDialect) -> Self {
        match d {
            CliDialect::Postgres => Self::Postgres,
            CliDialect::Mysql => Self::Mysql,
            CliDialect::Sqlite => Self::Sqlite,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliExtractor {
    Auto,
    None,
    Gorm,
    Bun,
    Ent,
    Prisma,
    Drizzle,
    Sqlalchemy,
    Django,
    Diesel,
    Seaorm,
}

impl From<CliExtractor> for ModelExtractorKind {
    fn from(e: CliExtractor) -> Self {
        match e {
            CliExtractor::Auto => Self::Auto,
            CliExtractor::None => Self::None,
            CliExtractor::Gorm => Self::Gorm,
            CliExtractor::Bun => Self::Bun,
            CliExtractor::Ent => Self::Ent,
            CliExtractor::Prisma => Self::Prisma,
            CliExtractor::Drizzle => Self::Drizzle,
            CliExtractor::Sqlalchemy => Self::Sqlalchemy,
            CliExtractor::Django => Self::Django,
            CliExtractor::Diesel => Self::Diesel,
            CliExtractor::Seaorm => Self::Seaorm,
        }
    }
}

/// Shared schema & model source flags for `check` and `baseline`.
#[derive(clap::Args, Clone)]
pub struct SchemaSourceArgs {
    /// Live read-only database DSN (`postgres://`, `mysql://`, `sqlite://` or `.db`/`.sqlite` path)
    #[arg(long, conflicts_with_all = ["snapshot", "sql"])]
    pub dsn: Option<String>,
    /// Previously exported `DbSchemaSnapshot` JSON file
    #[arg(long, value_name = "FILE", conflicts_with_all = ["dsn", "sql"])]
    pub snapshot: Option<PathBuf>,
    /// SQL schema or migration file/glob (repeatable); auto-discovered under `--repo` when omitted
    #[arg(long, value_name = "PATH_OR_GLOB")]
    pub sql: Vec<String>,
    /// SQL dialect hint for static SQL reconstruction
    #[arg(long, value_enum)]
    pub dialect: Option<CliDialect>,
    /// Database schemas to inspect (repeatable; defaults to all non-system schemas)
    #[arg(long = "schema", value_name = "NAME")]
    pub schemas: Vec<String>,
    /// Read-only inspection timeout in seconds
    #[arg(long, default_value_t = DEFAULT_INSPECT_TIMEOUT_SECONDS)]
    pub timeout: u32,
    /// Repository directory for static SQL discovery, ORM extraction and default config lookup
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
    /// Explicit application model manifest JSON file
    #[arg(long, value_name = "FILE", conflicts_with = "adapter")]
    pub model_manifest: Option<PathBuf>,
    /// Built-in static ORM extractors to run (repeatable; defaults to `auto` or config)
    #[arg(long = "extractor", value_enum)]
    pub extractors: Vec<CliExtractor>,
    /// Local external model adapter name (`sych-schema-adapter-<NAME>` on PATH)
    #[arg(long, value_name = "NAME", conflicts_with = "model_manifest")]
    pub adapter: Option<String>,
    /// Extra argument passed to `--adapter` (repeatable)
    #[arg(
        long = "adapter-arg",
        value_name = "ARG",
        requires = "adapter",
        allow_hyphen_values = true
    )]
    pub adapter_args: Vec<String>,
}

#[derive(clap::Args)]
pub struct SchemaCheckArguments {
    #[command(flatten)]
    pub source: SchemaSourceArgs,
    /// Schema linter YAML config (defaults to `<repo>/.sych/schema-lint.yaml` if present)
    #[arg(long, value_name = "FILE")]
    pub config: Option<PathBuf>,
    /// Output format
    #[arg(long, value_enum, default_value = "text")]
    pub format: SchemaOutputFormat,
    /// Write output here instead of stdout
    #[arg(long, value_name = "FILE")]
    pub output: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct SchemaBaselineArguments {
    #[command(flatten)]
    pub source: SchemaSourceArgs,
    /// Schema linter YAML config file to create or update
    #[arg(long, value_name = "FILE", default_value = ".sych/schema-lint.yaml")]
    pub config: PathBuf,
    /// Print the updated YAML to stdout without modifying `--config`
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(clap::Args)]
pub struct SchemaSnapshotArguments {
    /// Live read-only database DSN (`postgres://`, `mysql://`, `sqlite://` or `.db`/`.sqlite` path)
    #[arg(long, conflicts_with = "sql")]
    pub dsn: Option<String>,
    /// SQL schema or migration file/glob (repeatable); auto-discovered under `--repo` when omitted
    #[arg(long, value_name = "PATH_OR_GLOB")]
    pub sql: Vec<String>,
    /// SQL dialect hint for static SQL reconstruction
    #[arg(long, value_enum)]
    pub dialect: Option<CliDialect>,
    /// Database schemas to inspect (repeatable)
    #[arg(long = "schema", value_name = "NAME")]
    pub schemas: Vec<String>,
    /// Read-only inspection timeout in seconds
    #[arg(long, default_value_t = DEFAULT_INSPECT_TIMEOUT_SECONDS)]
    pub timeout: u32,
    /// Repository directory for static SQL discovery
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
    /// Write snapshot JSON here instead of stdout
    #[arg(long, value_name = "FILE")]
    pub output: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct SchemaExtractArguments {
    /// Repository directory to scan for ORM models
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
    /// Built-in static ORM extractors to run (repeatable; defaults to `auto`)
    #[arg(long = "extractor", value_enum)]
    pub extractors: Vec<CliExtractor>,
    /// Local external model adapter name (`sych-schema-adapter-<NAME>` on PATH)
    #[arg(long, value_name = "NAME")]
    pub adapter: Option<String>,
    /// Extra argument passed to `--adapter` (repeatable)
    #[arg(
        long = "adapter-arg",
        value_name = "ARG",
        requires = "adapter",
        allow_hyphen_values = true
    )]
    pub adapter_args: Vec<String>,
    /// Optional schema linter YAML config carrying model naming/table overrides
    #[arg(long, value_name = "FILE")]
    pub config: Option<PathBuf>,
    /// Read-only adapter timeout in seconds
    #[arg(long, default_value_t = DEFAULT_INSPECT_TIMEOUT_SECONDS)]
    pub timeout: u32,
    /// Write manifest JSON here instead of stdout
    #[arg(long, value_name = "FILE")]
    pub output: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct SchemaRulesArguments {
    /// Filter rules applicable to this SQL dialect
    #[arg(long, value_enum)]
    pub dialect: Option<CliDialect>,
    #[arg(long, value_enum, default_value = "text")]
    pub format: SchemaListFormat,
    #[arg(long, value_name = "FILE")]
    pub output: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct SchemaExplainArguments {
    /// Rule id (for example `fk-without-index` or `DB-FK-WITHOUT-INDEX`)
    pub rule: String,
    #[arg(long, value_enum, default_value = "text")]
    pub format: SchemaListFormat,
    #[arg(long, value_name = "FILE")]
    pub output: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct SchemaManifestSchemaArguments {
    /// Write JSON Schema here instead of stdout
    #[arg(long, value_name = "FILE")]
    pub output: Option<PathBuf>,
}
