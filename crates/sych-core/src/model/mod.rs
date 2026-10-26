//! Model gateway contracts: providers, structured output, budgets and the ledger (spec §16–17).
//! Transports live in adapters; this module has no HTTP or SQL.

mod budget;
mod checkpoint;
mod contract;
mod gateway;
mod router;
mod structured;

pub use budget::*;
pub use checkpoint::*;
pub use contract::*;
pub use gateway::*;
pub use router::*;
pub use structured::{parse_output, repair_messages};
