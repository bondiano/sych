//! One file per subcommand. Each `run` takes its arguments, runs the services it needs and
//! returns its exit code; errors are reported by [`crate::command::run_command`].

pub mod auth_chatgpt;
pub mod cache_prune;
pub mod config_init;
pub mod config_validate;
pub mod db_migrate;
pub mod doctor;
pub mod eval_measure;
pub mod eval_run;
pub mod eval_validate;
pub mod explain;
pub mod reconcile;
pub mod review;
pub mod run_action;
pub mod semantic;
pub mod serve;
pub mod show;
