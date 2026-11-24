//! Command-line arguments: one `*Arguments` struct per subcommand.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use sych_core::config::CheckRequest;

use crate::render::{Format, ReportFormat};

mod eval;
mod schema;
mod tour;

pub use eval::{
    EvalAutoJudgeArguments, EvalCommands, EvalCompareArguments, EvalJudgeArguments,
    EvalMeasureArguments, EvalRunArguments, EvalValidateArguments, PipelineVariant,
};
pub use schema::{
    CliDialect, CliExtractor, SchemaBaselineArguments, SchemaCheckArguments, SchemaCommands,
    SchemaExplainArguments, SchemaExtractArguments, SchemaListFormat,
    SchemaManifestSchemaArguments, SchemaOutputFormat, SchemaRulesArguments,
    SchemaSnapshotArguments, SchemaSourceArgs,
};
pub use tour::{
    AskArguments, AskKind, HandoffArguments, TourArguments, TourCommands, TourRef,
    TourRevealArguments, TourShowArguments,
};

#[derive(Parser)]
#[command(name = "sych", version, about, arg_required_else_help = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Review the commits between a target ref and a head (local, never publishes)
    Review(Box<ReviewArguments>),
    /// Print semantic facts of changed constructs from pinned language servers; downloads and
    /// caches the pinned server on first use, never runs build scripts or proc-macros
    Semantic(SemanticArguments),
    /// Create and validate configuration
    #[command(subcommand)]
    Config(ConfigCommands),
    /// Check availability and permissions; never deploys or comments, and calls a paid model
    /// only with --probe-model
    Doctor(DoctorArguments),
    /// Print a saved review result; never runs analysis
    Show(ShowArguments),
    /// Explain one obligation of a saved run with the evidence it cites; never runs analysis
    Explain(ExplainArguments),
    /// Generate a guided tour of a saved run with a model profile and store it; never reviews
    /// again. `tour show|reveal|complete|dismiss` read a stored one without a model
    Tour(TourArguments),
    /// Ask about one stop of a stored tour: a comprehension question calls the model under the
    /// session's own follow-up budget; the answer is private and never evidence
    Ask(AskArguments),
    /// Export selected findings of a saved run for a separate author workflow; never calls a
    /// model, edits sources or resolves a finding
    Handoff(HandoffArguments),
    /// Harness evaluation against fixtures
    #[command(subcommand)]
    Eval(EvalCommands),
    /// Database schema management
    #[command(subcommand)]
    Db(DbCommands),
    /// Built-in database and application schema linter (standalone, live DB / CI snapshot / SQL / ORMs)
    #[command(subcommand)]
    Schema(SchemaCommands),
    /// Sign in to a subscription with its official flow; writes only the token file
    #[command(subcommand)]
    Auth(AuthCommands),
    /// Artifact retention and GC
    #[command(subcommand)]
    Cache(CacheCommands),
    /// Run the server: SCM webhooks, scheduler, worker and publisher in one process
    Serve(ServeArguments),
    /// Run only the API half: webhooks and `/v1`, no claimed work
    Api(ServeArguments),
    /// Run only the worker half: scheduler, worker and publisher, with health and metrics
    Worker(ServeArguments),
    /// Queue the advisory publication of a committed run; the server sends it
    Publish(RunActionArguments),
    /// Cancel an active run; a committed result keeps its state (exit 130 when cancelled)
    Cancel(RunActionArguments),
    /// A new generation of a change from the provider's current state
    Rerun(RerunArguments),
    /// List publications whose outcome is unknown; with --allow-create, record the operator's
    /// audited decision to create one again despite the duplicate risk
    Reconcile(ReconcileArguments),
}

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Write a personal server config; existing files are never overwritten
    Init(ConfigInitArguments),
    /// Validate config and print the effective config with hashes and provenance
    Validate(ConfigValidateArguments),
}

#[derive(Subcommand)]
pub enum AuthCommands {
    /// Sign in with `ChatGPT` (loopback browser flow) for a `chatgpt_subscription` provider; makes
    /// no model request
    Chatgpt(AuthChatgptArguments),
}

#[derive(clap::Args)]
pub struct AuthChatgptArguments {
    /// Server config
    #[arg(long)]
    pub config: PathBuf,
    /// The `chatgpt_subscription` provider whose `connection_ref_env` names the token file
    #[arg(long)]
    pub provider: String,
    /// Loopback port of the redirect; keep it the same on later sign-ins
    #[arg(long, default_value_t = sych_adapters::siwc::auth::REDIRECT_PORT)]
    pub port: u16,
}

#[derive(Subcommand)]
pub enum DbCommands {
    /// Apply pending migrations; local review migrates SQLite automatically
    Migrate(StoreArguments),
    /// Import a stopped `SQLite` snapshot and its artifacts into an empty, migrated `PostgreSQL`
    /// database
    ImportSqlite(DbImportArguments),
}

#[derive(Subcommand)]
pub enum CacheCommands {
    /// Delete orphaned and expired artifacts; active runs and retention are protected
    Prune(CachePruneArguments),
}

/// The pinned comparison every analysis reads; only Git objects are read.
#[derive(clap::Args, Clone)]
pub struct Comparison {
    /// Repository to read; the work tree is untouched
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
    /// Target ref; diff base is merge-base(target tip, head). Not with `review --change`.
    #[arg(long, default_value = "", hide_default_value = true)]
    pub base: String,
    /// Reviewed head
    #[arg(long, default_value = "HEAD")]
    pub head: String,
    /// Explicit two-dot comparison base instead of merge-base
    #[arg(long)]
    pub diff_base: Option<String>,
    /// The head is a fork's, from a server run: its code never runs.
    #[arg(skip)]
    pub fork: bool,
}

#[derive(clap::Args)]
pub struct Output {
    #[arg(long, value_enum, default_value = "json")]
    pub format: Format,
    /// Write the rendered result here instead of stdout; parent directories are created
    #[arg(long)]
    pub output: Option<PathBuf>,
}

/// [`Output`] of a saved review, which can also be SARIF.
#[derive(clap::Args)]
pub struct ReportOutput {
    /// Output format (JSON by default, with a short summary for review on a terminal)
    #[arg(long, value_enum)]
    pub format: Option<ReportFormat>,
    /// Write the rendered result here instead of stdout; parent directories are created
    #[arg(long)]
    pub output: Option<PathBuf>,
    /// Write the combined patch of applicable suggested changes
    #[arg(long, value_name = "PATH")]
    pub fixes_patch: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct ReviewArguments {
    /// Ref, A..B (two-dot), or A...B (merge-base); defaults to HEAD against the default branch
    #[arg(value_name = "REF", conflicts_with_all = ["base", "head", "diff_base", "change"])]
    pub target: Option<String>,
    #[command(flatten)]
    pub comparison: Comparison,
    /// Review the staged index on top of HEAD
    #[arg(long, conflicts_with_all = ["worktree", "change", "head"])]
    pub staged: bool,
    /// Review staged, unstaged and untracked files on top of HEAD (excluding ignored files)
    #[arg(long, conflicts_with_all = ["change", "head"])]
    pub worktree: bool,
    /// Review a provider change at its current state instead of local refs; never publishes
    #[arg(
        long,
        value_name = "github:CONNECTION:OWNER/REPO:NUMBER",
        conflicts_with = "base"
    )]
    pub change: Option<String>,
    /// Trusted config; for local reviews, defaults to the user config if present.
    /// Repository policy comes from the target tip unless --policy-ref is set
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// `fake` makes no external calls; `roles` routes each role through `models.roles` and its
    /// fallback chains; otherwise one pinned model profile of `--config` (with only its own
    /// fallback chain). Defaults to `default_models` in the config or `fake`.
    /// Real profiles make paid or quota-counted calls within the budgets
    #[arg(long, value_parser = ModelSelection::parse)]
    pub models: Option<ModelSelection>,
    /// Scripted evidence and assessments for the fake adapter; without it obligations stay
    /// honestly unassessed
    #[arg(long)]
    pub fake_responses: Option<PathBuf>,
    /// Apply the policy of this revision instead of the target tip's: an explicit operator
    /// run with a new trusted policy, recorded as the policy source of the result
    #[arg(long, value_name = "REF")]
    pub policy_ref: Option<String>,
    /// Semantic facts from pinned language servers and findings of pinned linters (ruff, oxlint,
    /// sqruff, mago, jarl, clj-kondo, actionlint, zizmor, easement) become derived evidence and
    /// context packets; tools are downloaded on first use
    #[arg(long, value_enum, default_value = "on")]
    pub semantic: Switch,
    /// CI report to import: `cargo … --message-format=json` output or `JUnit` XML (repeatable).
    /// Imported reports are author claims, not authenticated CI
    #[arg(long, value_name = "FILE")]
    pub ci_facts: Vec<PathBuf>,
    /// Job, tested commit, toolchain, target and features of the `--ci-facts` reports; without
    /// it they are unbound and their locations are not mapped
    #[arg(long, value_name = "FILE")]
    pub ci_manifest: Option<PathBuf>,
    /// Run only this execution profile of `--config`, with its parameters, instead of every
    /// allowed one (repeatable); each value must be one of the profile's choices
    #[arg(long = "check", value_name = "PROFILE[:PARAM=VALUE,...]", value_parser = CheckRequest::parse)]
    pub checks: Vec<CheckRequest>,
    /// `strict` exits 10 on a significant violation, else 11 on incomplete coverage
    #[arg(long, value_enum, default_value = "operational")]
    pub exit_policy: ExitPolicy,
    #[command(flatten)]
    pub out: ReportOutput,
    /// The author's description of the change, instead of its commit messages
    #[arg(long, value_name = "PATH")]
    pub intent_file: Option<PathBuf>,
    /// Override `.sych/config.yaml` `schema_lint.enabled` for this local review
    #[arg(long, value_enum)]
    pub schema_lint: Option<Switch>,
    /// Live read-only database DSN for schema linting during this local review
    #[arg(long, value_name = "DSN")]
    pub db_dsn: Option<String>,
    /// Explicit application model manifest JSON for schema linting during this local review
    #[arg(long, value_name = "FILE")]
    pub db_model_manifest: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct SemanticArguments {
    #[command(flatten)]
    pub comparison: Comparison,
    /// Write JSON here instead of stdout
    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct DoctorArguments {
    /// Server config
    #[arg(long)]
    pub config: PathBuf,
    /// Repository whose working-tree `.sych/` is validated
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
    /// Also send one small paid structured request through this model profile
    #[arg(long, value_name = "PROFILE")]
    pub probe_model: Option<String>,
    /// Only write, read back, update, find and delete one marked comment on `--pr` through
    /// this GitHub connection's first repository; prints a JSON report
    #[arg(long, value_name = "CONNECTION", requires = "pr")]
    pub scm_smoke: Option<String>,
    /// Pull request of the SCM smoke
    #[arg(long, requires = "scm_smoke")]
    pub pr: Option<u64>,
}

#[derive(clap::Args)]
pub struct ShowArguments {
    #[arg(long)]
    pub run: String,
    /// Repository containing the run's pinned source
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[command(flatten)]
    pub out: ReportOutput,
}

#[derive(clap::Args)]
pub struct ExplainArguments {
    #[arg(long)]
    pub run: String,
    /// Obligation id or rule id
    #[arg(long)]
    pub obligation: String,
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[command(flatten)]
    pub out: Output,
}

#[derive(clap::Args)]
pub struct ServeArguments {
    /// Server config; its `scm` connections, storage and http bind are used
    #[arg(long)]
    pub config: PathBuf,
    /// Model profile name, `roles` for the configured role plan, or `fake`
    #[arg(long, value_parser = ModelSelection::parse, default_value = "roles")]
    pub models: ModelSelection,
    /// Semantic facts from pinned language servers
    #[arg(long, value_enum, default_value = "on")]
    pub semantic: Switch,
}

#[derive(clap::Args)]
pub struct RunActionArguments {
    #[arg(long)]
    pub config: PathBuf,
    #[arg(long)]
    pub run: String,
    /// Configured API client this action is taken as
    #[arg(long = "as", value_name = "CLIENT")]
    pub actor: String,
}

#[derive(clap::Args)]
pub struct RerunArguments {
    #[arg(long)]
    pub config: PathBuf,
    /// Numeric change id
    #[arg(long)]
    pub change: i64,
    #[arg(long, value_parser = ["economical", "deep"], default_value = "economical")]
    pub profile: String,
    /// Configured API client this action is taken as
    #[arg(long = "as", value_name = "CLIENT")]
    pub actor: String,
}

#[derive(clap::Args)]
pub struct ReconcileArguments {
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// Only this change (its numeric id)
    #[arg(long)]
    pub change: Option<i64>,
    /// Publication operation an operator allows to be created again
    #[arg(long, requires_all = ["actor", "reason"])]
    pub allow_create: Option<i64>,
    /// Who decides: a configured API client with the writer role, recorded in the audit log
    #[arg(long)]
    pub actor: Option<String>,
    /// Why a duplicate is acceptable; recorded in the audit log
    #[arg(long)]
    pub reason: Option<String>,
}

/// Commands that only need the storage location.
#[derive(clap::Args)]
pub struct StoreArguments {
    #[arg(long)]
    pub config: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct DbImportArguments {
    /// Consistent `SQLite` snapshot; stop all writers and GC before taking it
    #[arg(long)]
    pub source: PathBuf,
    /// `PostgreSQL` target and the matching artifact directory
    #[arg(long)]
    pub config: PathBuf,
}

#[derive(clap::Args)]
pub struct CachePruneArguments {
    /// List what would be deleted without deleting
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub config: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct ConfigInitArguments {
    /// Personal mode (the only mode available so far)
    #[arg(long, required = true)]
    pub personal: bool,
    #[arg(long, default_value = "sych.yaml")]
    pub output: PathBuf,
}

#[derive(clap::Args)]
pub struct ConfigValidateArguments {
    /// Server config; engine defaults when omitted
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// Repository whose working-tree `.sych/` is validated
    #[arg(long)]
    pub repo: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelSelection {
    Fake,
    /// Each role starts at its `models.roles` profile and follows its fallback chain.
    Roles,
    Profile(String),
}

impl ModelSelection {
    pub(super) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "" => Err("expected `fake`, `roles` or a model profile name".into()),
            "fake" => Ok(Self::Fake),
            "roles" => Ok(Self::Roles),
            name => Ok(Self::Profile(name.to_owned())),
        }
    }

    /// Resolve an omitted CLI selector from the validated trusted config.
    pub fn for_review(explicit: Option<&Self>, configured: Option<&str>) -> Self {
        match (explicit, configured) {
            (Some(models), _) => models.clone(),
            (None, Some("roles")) => Self::Roles,
            (None, Some(name)) if name != "fake" => Self::Profile(name.to_owned()),
            (None, configured) => {
                if configured.is_none() {
                    eprintln!(
                        "No model was selected or called. Set default_models in your user config or pass --config and --models to enable models."
                    );
                }

                Self::Fake
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Switch {
    On,
    Off,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum ExitPolicy {
    /// Advisory findings never fail the CLI
    Operational,
    Strict,
}
