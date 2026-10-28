use std::io::{self, IsTerminal};
use std::process::ExitCode;

use clap::Parser;
use sych_app::args::{
    AuthCommands, CacheCommands, Cli, Commands, ConfigCommands, DbCommands, EvalCommands,
};
use sych_app::commands::{
    ask, auth_chatgpt, cache_prune, config_init, config_validate, db_migrate, doctor, eval_measure,
    eval_run, eval_validate, explain, reconcile, review, run_action, semantic, serve, show, tour,
};
use sych_app::run_command;
use sych_app::server::Role;
use tracing_subscriber::EnvFilter;

/// Sych crates log at `info`, dependencies only from `warn`; `SYCH_LOG` overrides it with
/// the usual filter syntax, e.g. `SYCH_LOG=debug` or `SYCH_LOG=sych_adapters=trace`.
const DEFAULT_LOG: &str = "warn,sych_app=info,sych_adapters=info,sych_storage=info";

fn main() -> ExitCode {
    let filter =
        EnvFilter::try_from_env("SYCH_LOG").unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(io::stderr)
        .with_ansi(io::stderr().is_terminal())
        .with_target(false)
        .without_time()
        .init();

    match Cli::parse().command {
        Commands::Review(args) => run_command("review", || review::run(&args)),
        Commands::Semantic(args) => run_command("semantic", || semantic::run(&args)),
        Commands::Show(args) => run_command("show", || show::run(&args)),
        Commands::Explain(args) => run_command("explain", || explain::run(&args)),
        Commands::Tour(args) => run_command("tour", || tour::run(&args)),
        Commands::Ask(args) => run_command("ask", || ask::run(&args)),
        Commands::Doctor(args) => run_command("doctor", || doctor::run(&args)),
        Commands::Config(ConfigCommands::Init(args)) => {
            run_command("config init", || config_init::run(&args))
        }
        Commands::Config(ConfigCommands::Validate(args)) => {
            run_command("config validate", || config_validate::run(&args))
        }
        Commands::Eval(EvalCommands::Run(args)) => run_command("eval run", || eval_run::run(&args)),
        Commands::Eval(EvalCommands::Validate(args)) => {
            run_command("eval validate", || eval_validate::run(&args))
        }
        Commands::Eval(EvalCommands::Measure(args)) => {
            run_command("eval measure", || eval_measure::run(&args))
        }
        Commands::Db(DbCommands::Migrate(args)) => {
            run_command("db migrate", || db_migrate::run(&args))
        }
        Commands::Auth(AuthCommands::Chatgpt(args)) => {
            run_command("auth chatgpt", || auth_chatgpt::run(&args))
        }
        Commands::Cache(CacheCommands::Prune(args)) => {
            run_command("cache prune", || cache_prune::run(&args))
        }
        Commands::Serve(args) => run_command("serve", || serve::run(&args, Role::All)),
        Commands::Api(args) => run_command("api", || serve::run(&args, Role::Api)),
        Commands::Worker(args) => run_command("worker", || serve::run(&args, Role::Worker)),
        Commands::Reconcile(args) => run_command("reconcile", || reconcile::run(&args)),
        Commands::Publish(args) => run_command("publish", || run_action::publish(&args)),
        Commands::Cancel(args) => run_command("cancel", || run_action::cancel(&args)),
        Commands::Rerun(args) => run_command("rerun", || run_action::rerun(&args)),
    }
}
