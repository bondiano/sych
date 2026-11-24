//! The structure of the code itself, computed without a model: which modules, types and
//! functions know which, built from resolved references, never from names found in text.
//! Where an item is declared and what owns it differ: a method belongs to its type wherever
//! its `impl` block is, and a wrapper that adds nothing is seen through.
//!
//! The model, the graph and the checks are ported from deslop-please
//! (<https://github.com/deterministic-machines/deslop-please>, commit `f678d10`), MIT,
//! Copyright (c) 2026 Deterministic Machines, Inc.; the notice is in `UPSTREAM-LICENSE`.
//! Upstream reads Rust only; here a language adapter builds the same model for Rust, Go,
//! TypeScript/JavaScript and Python.

mod baseline;
mod checks;
mod facts;
mod findings;
mod graph;
mod model;
mod tree;

pub use baseline::{Change, Comparison, Found, compare};
pub use checks::{Check, Diagnostic, HELD_IN_MIND, Limits, Location, Picture, Step, check, draw};
pub use facts::{ArchitectureFacts, ArchitectureFinding, CheckCoverage, FixedFinding};
pub use findings::{PICTURE, findings, gaps, rule_id};
pub use graph::{Graph, Thing, ThingId, Use, circles};
pub use model::{File, FileId, Item, ItemId, ItemKind, Model, Reference, Wrapper};
pub use tree::{Node, NodeId, NodeKind, Tree};
