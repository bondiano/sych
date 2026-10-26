//! The `sych` command line: arguments, one command per subcommand and the services they
//! share. The binary in `bin/sych.rs` only parses arguments and dispatches.

pub mod analysis;
pub mod args;
pub mod backend;
pub mod command;
pub mod commands;
pub mod errors;

pub mod ci;
pub mod config;
pub mod eval;
pub mod fake;
pub mod output;
pub mod probe;
pub mod quality;
pub mod render;
pub mod review;
pub mod semantic;
pub mod server;
pub mod snapshot;
pub mod storage;

pub use command::run_command;

/// The commit this reviewer was built from, or `unknown`.
pub const BUILD_COMMIT: &str = env!("SYCH_BUILD_COMMIT");
