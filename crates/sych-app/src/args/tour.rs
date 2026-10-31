//! Arguments of the guided tour, its follow-up questions and the fix handoff.

use std::path::PathBuf;

use super::{ModelSelection, Output};

#[derive(clap::Args)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
pub struct TourArguments {
    #[command(subcommand)]
    pub action: Option<TourCommands>,
    #[arg(long, required = true)]
    pub run: Option<String>,
    /// Server config with the model profiles and the store
    #[arg(long, required = true)]
    pub config: Option<PathBuf>,
    /// The repository the run reviewed; its snapshot is captured again and must match
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
    /// `roles` or one model profile; the tour has its own budget (`budgets.tour`)
    #[arg(long, value_parser = ModelSelection::parse, default_value = "roles")]
    pub models: ModelSelection,
    #[command(flatten)]
    pub out: Output,
}

/// Reading a stored tour: no model call, and progress stays in the reader's own session.
#[derive(clap::Subcommand)]
pub enum TourCommands {
    /// Print a stored tour (the newest of the run unless --tour) and start the session
    Show(TourShowArguments),
    /// Print the answer of a step's question and record it in the session
    Reveal(TourRevealArguments),
    /// Mark the session completed; this is not an approval and is not published
    Complete(TourRef),
    /// Mark the session dismissed; the tour and the review stay as they are
    Dismiss(TourRef),
}

#[derive(clap::Args)]
pub struct TourRef {
    #[arg(long)]
    pub run: String,
    /// One tour of the run; the newest when omitted
    #[arg(long)]
    pub tour: Option<String>,
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[command(flatten)]
    pub out: Output,
}

#[derive(clap::Args)]
pub struct TourShowArguments {
    #[command(flatten)]
    pub tour: TourRef,
    /// The repository the run reviewed: code is read from its pinned blobs
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
    /// Only the overview and the stops' titles
    #[arg(long)]
    pub brief: bool,
}

#[derive(clap::Args)]
pub struct TourRevealArguments {
    #[command(flatten)]
    pub tour: TourRef,
    #[arg(long)]
    pub step: String,
}

#[derive(clap::Args)]
pub struct AskArguments {
    #[command(flatten)]
    pub tour: TourRef,
    #[arg(long)]
    pub step: String,
    /// The question; for `fact-request`, your own statement of the missing fact
    #[arg(long)]
    pub question: String,
    #[arg(long, value_enum, default_value = "comprehension")]
    pub kind: AskKind,
    /// The repository the run reviewed: code is read from its pinned blobs
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
    /// `roles` or one model profile; follow-up has its own budget (`budgets.followup_session`)
    #[arg(long, value_parser = ModelSelection::parse, default_value = "roles")]
    pub models: ModelSelection,
}

/// Only a comprehension question calls the model.
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum AskKind {
    Comprehension,
    /// Records your statement with author and time; it is not evidence
    FactRequest,
    /// A scoped waiver; refused locally, it is the repository owner's on the server
    RiskDecision,
}

#[derive(clap::Args)]
pub struct HandoffArguments {
    #[arg(long)]
    pub run: String,
    /// A finding id of the run's result; repeat for several
    #[arg(long = "finding", required = true)]
    pub findings: Vec<String>,
    /// Refuse unless the run reviewed this source snapshot (`sha256:…`)
    #[arg(long)]
    pub snapshot: Option<String>,
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[command(flatten)]
    pub out: Output,
}
