//! Semantic facts from pinned language servers and diagnostics of pinned linters, downloaded
//! and cached on first use.

use std::path::Path;

use anyhow::Context;

use sych_adapters::git::LocalSnapshot;
use sych_adapters::lint;
use sych_adapters::semantic::{self, Options};
use sych_adapters::tools::{ToolCache, ToolError, default_cache_root};
use sych_core::config::Tooling;
use sych_core::{LintFacts, SemanticFacts};

use crate::errors::Failure;
use crate::storage::block_on;

/// Facts for every side and language of `snap`. Dependencies are installed only when
/// `tooling` says so, in its sandbox.
pub fn collect_facts(
    snap: &LocalSnapshot,
    tooling: &Tooling,
) -> anyhow::Result<Vec<SemanticFacts>> {
    let root = default_cache_root()
        .ok_or_else(|| Failure::invalid("no tool cache directory (set SYCH_TOOLS_DIR or HOME)"))?;
    let state_dir = root
        .parent()
        .map_or_else(|| root.join("state"), Path::to_path_buf);
    let semantic_cache = state_dir.join("semantic");
    let cache = ToolCache::new(root);
    let resolve = |tool, target: &str| {
        block_on(cache.ensure(tool, target))
            .map_err(|_| ToolError::Download("cannot start runtime".into()))?
    };
    let options = Options {
        resolve: &resolve,
        cache: Some(&semantic_cache),
        tooling,
        state_dir: &state_dir,
    };
    semantic::collect(snap, &options).context("semantic facts")
}

/// Pinned linters on the head's changed files; a linter that cannot be fetched is reported in
/// its facts, never fatal.
pub fn collect_lint(snap: &LocalSnapshot) -> anyhow::Result<Vec<LintFacts>> {
    let root = default_cache_root()
        .ok_or_else(|| Failure::invalid("no tool cache directory (set SYCH_TOOLS_DIR or HOME)"))?;
    let cache = ToolCache::new(root);
    let resolve = |tool, target: &str| {
        block_on(cache.ensure(tool, target))
            .map_err(|_| ToolError::Download("cannot start runtime".into()))?
    };
    Ok(lint::collect(snap, &resolve))
}
