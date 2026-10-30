//! Command-line arguments: one `*Arguments` struct per subcommand.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use sych_core::config::{CheckRequest, Execution, RuleException, RunMode};

use crate::render::Format;

mod tour;

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
pub enum EvalCommands {
    /// Run a fixture suite and compare significant states; fake models make no external calls
    Run(EvalRunArguments),
    /// Check a labelled quality suite: labels are complete and every evidence location and
    /// expected syntax shape resolves; never calls a model
    Validate(EvalValidateArguments),
    /// Review a labelled quality suite with a real model profile and score it against the
    /// labels; makes paid or quota-counted calls within the configured budgets
    Measure(EvalMeasureArguments),
    /// Put saved `eval measure` reports of the same cases and config side by side (baselines
    /// A/B/C, repeats) with the cases whose outcome differs; never calls a model
    Compare(EvalCompareArguments),
    /// Rescore a saved `eval measure` report by the owner's judgments of which label each
    /// correctness finding states; shows location and meaning scores; never calls a model
    Judge(EvalJudgeArguments),
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
    /// Apply pending migrations; other commands never change the schema
    Migrate(StoreArguments),
    /// Import a stopped `SQLite` snapshot and its artifacts into an empty, migrated `PostgreSQL` database
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
}

#[derive(clap::Args)]
pub struct Output {
    #[arg(long, value_enum, default_value = "json")]
    pub format: Format,
    /// Write the rendered result here instead of stdout; parent directories are created
    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct ReviewArguments {
    #[command(flatten)]
    pub comparison: Comparison,
    /// Review a provider change at its current state instead of local refs; never publishes
    #[arg(
        long,
        value_name = "github:CONNECTION:OWNER/REPO:NUMBER",
        conflicts_with = "base"
    )]
    pub change: Option<String>,
    /// Server config; engine defaults when omitted. Repository policy always comes from the
    /// target tip, never from the reviewed head
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// `fake` makes no external calls; `roles` routes each role through `models.roles` and its
    /// fallback chains; otherwise one pinned model profile of `--config` (with only its own
    /// fallback chain). Real profiles make paid or quota-counted calls within the budgets
    #[arg(long, value_parser = ModelSelection::parse)]
    pub models: ModelSelection,
    /// Scripted evidence and assessments for the fake adapter; without it obligations stay
    /// honestly unassessed
    #[arg(long)]
    pub fake_responses: Option<PathBuf>,
    /// Apply the policy of this revision instead of the target tip's: an explicit operator
    /// run with a new trusted policy, recorded as the policy source of the result
    #[arg(long, value_name = "REF")]
    pub policy_ref: Option<String>,
    /// Semantic facts from pinned language servers and findings of pinned linters (ruff,
    /// oxlint, sqruff, mago, jarl, clj-kondo, actionlint, zizmor) become derived evidence and context packets; tools are
    /// downloaded on first use
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
    /// Execution profiles of one eval case instead of the server's.
    #[arg(skip)]
    pub execution: Option<Execution>,
    /// `strict` exits 10 on a significant violation, else 11 on incomplete coverage
    #[arg(long, value_enum, default_value = "operational")]
    pub exit_policy: ExitPolicy,
    #[command(flatten)]
    pub out: Output,
    /// The extra high-risk search (spec §7.3); only `eval measure` turns it off, for baseline B
    #[arg(skip = true)]
    pub risk_audit: bool,
    /// Candidate verification; only `eval measure` turns it off, for baseline A
    #[arg(skip = true)]
    pub verify: bool,
    /// Reports a server run fetched from authenticated CI, already normalized.
    #[arg(skip)]
    pub ci_imported: Vec<sych_core::CiFacts>,
    /// Expected reports that are not evidence, as `CiGap` text; each keeps coverage partial.
    #[arg(skip)]
    pub ci_gaps: Vec<String>,
    /// Scoped waivers recorded on the server, as `(rule id, exception)`; they lower the risk
    /// decision and never rewrite a status.
    #[arg(skip)]
    pub waivers: Vec<(String, RuleException)>,
    /// Where the review runs: a server run never uses a provider allowed only locally (such as
    /// a personal subscription sign-in).
    #[arg(skip = RunMode::PersonalLocal)]
    pub run_mode: RunMode,
    /// A `deep` manual server run: roles start at `models.deep_roles`.
    #[arg(skip)]
    pub deep: bool,
    /// The parent of a successor server run, whose checkpoints may be reused.
    #[arg(skip)]
    pub parent: Option<sych_core::RunId>,
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
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[command(flatten)]
    pub out: Output,
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
pub struct EvalRunArguments {
    #[arg(long)]
    pub suite: PathBuf,
    /// Only `fake`: the harness suite is deterministic and offline
    #[arg(long, value_parser = ModelSelection::parse)]
    pub models: ModelSelection,
    /// Report path; stdout when omitted
    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct EvalMeasureArguments {
    /// Directory of `<id>.yaml` cases of one split
    #[arg(long)]
    pub suite: PathBuf,
    /// Repository for `repository_fixture` cases, as NAME=PATH; repeatable
    #[arg(long, value_name = "NAME=PATH")]
    pub repo: Vec<String>,
    /// Server config with the model profile, budgets and store
    #[arg(long)]
    pub config: PathBuf,
    /// Model profile of `--config`
    #[arg(long, value_parser = ModelSelection::parse)]
    pub models: ModelSelection,
    /// Only these case ids (repeatable); all cases when omitted
    #[arg(long = "case", value_name = "ID")]
    pub cases: Vec<String>,
    /// Reviews per case, to see the variation of a nondeterministic model
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=10))]
    pub runs: u32,
    /// `c` is the default pipeline; `b` skips the high-risk search; `a` is one reviewer over
    /// the diff: no semantic facts, no high-risk search, no verifier (spec §27.5)
    #[arg(long, value_enum, default_value = "c")]
    pub variant: PipelineVariant,
    #[arg(long, value_enum, default_value = "on")]
    pub semantic: Switch,
    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PipelineVariant {
    A,
    B,
    C,
}

#[derive(clap::Args)]
pub struct EvalCompareArguments {
    /// `eval measure` reports, in the order they are shown
    #[arg(required = true, num_args = 2..)]
    pub reports: Vec<PathBuf>,
    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct EvalJudgeArguments {
    /// `eval measure` report
    pub report: PathBuf,
    /// YAML list of `{case, run, finding, label}` and `{case, run, unknown_kept}` judgments
    #[arg(long)]
    pub judgments: PathBuf,
    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct EvalValidateArguments {
    /// Directory of `<id>.yaml` cases of one split
    #[arg(long)]
    pub suite: PathBuf,
    /// Repository for `repository_fixture` cases, as NAME=PATH; repeatable
    #[arg(long, value_name = "NAME=PATH")]
    pub repo: Vec<String>,
    #[arg(long)]
    pub output: Option<PathBuf>,
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
    /// Who decides; recorded in the audit log
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
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "" => Err("expected `fake`, `roles` or a model profile name".into()),
            "fake" => Ok(Self::Fake),
            "roles" => Ok(Self::Roles),
            name => Ok(Self::Profile(name.to_owned())),
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
