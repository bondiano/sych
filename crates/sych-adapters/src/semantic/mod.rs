//! Semantic backends over pinned language servers.
//!
//! Each side of a snapshot is materialized separately from Git objects, so base and head
//! never share workspace state. Changed constructs are found with `documentSymbol`, then
//! references, incoming and outgoing calls and implementations are mapped back to exact `CodeRef`s.

mod access;
mod cache;
mod calls;
mod cargo;
pub(crate) mod deps;
mod diagnostics;
mod gomod;
mod init;
mod locate;
mod lsp;
mod profiles;
mod symbols;
mod tsproject;

pub(crate) use profiles::toolchain;

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sych_core::{
    BackendStatus, EdgeSet, FileChange, Language, RepoPath, SchemaVersion, SemanticFacts,
    SourceSide, SymbolFacts, classify, source_language,
};

use crate::git::{LocalSnapshot, Materialized, SnapshotError};
use crate::sandbox::{self, NOT_ISOLATED};
use sych_core::config::Tooling;

use crate::tools::{PinnedTool, ToolError, host_target};
use cache::DIAGNOSTICS_FAILED;
use init::{file_uri, initialize};
use locate::Edge;
use lsp::{LspError, LspServer};
use profiles::Profile;
use symbols::{changed_symbols, type_names};

const MAX_WORKSPACE_BLOB_BYTES: u64 = 4 << 20;
const READY_TIMEOUT: Duration = Duration::from_secs(600);
const QUERY_TIMEOUT: Duration = Duration::from_secs(120);
/// All queries of one server once it answered its first document: a slow or stuck server
/// cannot take `MAX_SYMBOLS` times the query timeout, and a file reached past it is not opened.
const QUERY_BUDGET: Duration = Duration::from_secs(180);
const MAX_SYMBOLS: usize = 60;
/// Type names per construct whose definitions are looked up.
const MAX_TYPES: usize = 6;

/// Resolves a pinned tool's build for a target (`<arch>-<os>`) to a binary, downloading it
/// when needed.
pub type Resolve<'a> = dyn Fn(&'static PinnedTool, &str) -> Result<PathBuf, ToolError> + 'a;

pub struct Options<'a> {
    pub resolve: &'a Resolve<'a>,
    /// Facts across runs of the same snapshot.
    pub cache: Option<&'a Path>,
    pub tooling: &'a Tooling,
    /// Sych-owned state for installs: the pnpm store and `CARGO_HOME`.
    pub state_dir: &'a Path,
}

/// A changed file on one side: path, LSP language id and changed 1-based line ranges.
struct Target {
    path: RepoPath,
    language_id: &'static str,
    ranges: Vec<(u32, u32)>,
    generated: bool,
}

fn targets(files: &[FileChange], side: SourceSide) -> BTreeMap<Language, Vec<Target>> {
    let mut out: BTreeMap<Language, Vec<Target>> = BTreeMap::new();
    for file in files {
        let Some(file_side) = file.side(side) else {
            continue;
        };
        let Some((language, language_id)) = source_language(file_side.path.as_str()) else {
            continue;
        };
        let ranges = file.changed_lines(side);
        out.entry(language).or_default().push(Target {
            path: file_side.path.clone(),
            language_id,
            ranges,
            generated: file.generated,
        });
    }
    out
}

/// Facts for every side and language with changed source; failures are reported inside.
/// `cache` holds facts across runs of the same snapshot.
/// `Err(Unavailable)` when dependency install fails under the `fail` policy.
#[tracing::instrument(level = "info", name = "semantic", skip_all, fields(snapshot = %snapshot.snapshot.id.0))]
pub fn collect(
    snapshot: &LocalSnapshot,
    options: &Options<'_>,
) -> Result<Vec<SemanticFacts>, SnapshotError> {
    let mut facts = Vec::new();
    for side in [SourceSide::DiffBase, SourceSide::Head] {
        let by_language = targets(&snapshot.manifest.files, side);
        if by_language.is_empty() {
            continue;
        }

        let dir = tempfile::tempdir()
            .map_err(|e| SnapshotError::Git(format!("workspace directory: {e}")))?;
        // Use the same canonical root as servers reporting locations through getcwd().
        let root = fs::canonicalize(dir.path())
            .map_err(|e| SnapshotError::Git(format!("workspace directory: {e}")))?;
        let materialized = snapshot.materialize(side, &root, MAX_WORKSPACE_BLOB_BYTES)?;
        let workspace = Workspace {
            root: &root,
            side,
            snapshot: snapshot.snapshot.id.clone(),
            partition: &snapshot.snapshot.repository,
            materialized,
        };
        for (language, files) in by_language {
            let analyzed = workspace.analyze(language, &files, options);
            facts.push(analyzed.map_err(|err| SnapshotError::Unavailable {
                reason_code: "TOOLING_UNAVAILABLE",
                detail: err.to_string(),
            })?);
        }
    }
    Ok(facts)
}

struct Workspace<'a> {
    root: &'a Path,
    side: SourceSide,
    snapshot: sych_core::SourceSnapshotId,
    partition: &'a str,
    materialized: Materialized,
}

impl Workspace<'_> {
    #[tracing::instrument(level = "info", skip_all, fields(side = ?self.side, language = ?language, files = files.len()))]
    fn analyze(
        &self,
        language: Language,
        files: &[Target],
        options: &Options<'_>,
    ) -> Result<SemanticFacts, deps::DepsError> {
        let mut facts = SemanticFacts {
            schema_version: SchemaVersion,
            source_snapshot_id: self.snapshot.clone(),
            side: self.side,
            language,
            status: BackendStatus::Ready,
            producer: None,
            position_encoding: None,
            build_configuration: BTreeMap::new(),
            query_scope: files.iter().map(|f| f.path.to_string()).collect(),
            symbols: Vec::new(),
            diagnostics: Vec::new(),
            limitations: Vec::new(),
        };
        let cached = profiles::profile(language, self.root)
            .ok()
            .zip(options.cache)
            .and_then(|(profile, dir)| {
                let key = cache::key(
                    &self.snapshot,
                    self.side,
                    language,
                    &profile,
                    options.tooling,
                )?;
                Some((dir, key))
            });
        if let Some((dir, key)) = &cached
            && let Some(hit) = cache::load(dir, key, self.partition)
        {
            tracing::debug!("semantic cache hit");
            return Ok(hit);
        }

        let resolve = options.resolve;
        let started = profiles::profile(language, self.root).and_then(|profile| {
            let binary = resolve(profile.tool, &host_target())?;
            Ok((profile, binary))
        });
        let (mut profile, binary) = match started {
            Ok(started) => started,
            Err(err) => {
                facts.status = match err {
                    ToolError::Unsupported(_) => BackendStatus::Unsupported,
                    _ => BackendStatus::Failed,
                };
                facts.limitations.push(err.to_string());
                return Ok(facts);
            }
        };
        let prepared = deps::prepare(
            language,
            self.root,
            options.tooling,
            resolve,
            options.state_dir,
        )?;
        profile.apply(&prepared);
        if language == Language::Go {
            match deps::go_env(resolve, options.state_dir) {
                Ok(env) => profile.env.extend(env),
                Err(err) => {
                    facts.status = BackendStatus::Failed;
                    facts.limitations.push(err.to_string());
                    return Ok(facts);
                }
            }
        }

        if language == Language::Rust && !self.materialized.tool_configs.is_empty() {
            profile.limitations.push(format!(
                "tool configuration not materialized (it can name commands): {}",
                self.materialized.tool_configs.join(", ")
            ));
        }

        if self.materialized.over_budget > 0 {
            profile.limitations.push(format!(
                "workspace budget spent: {} files not materialized",
                self.materialized.over_budget
            ));
        }

        facts
            .build_configuration
            .clone_from(&profile.build_configuration);
        facts.limitations.clone_from(&profile.limitations);
        if !sandbox::isolated() {
            facts.limitations.push(NOT_ISOLATED.into());
        }

        if let Err(err) = self.run(&profile, &binary, files, &mut facts) {
            facts.status = BackendStatus::Failed;
            facts.limitations.push(err.to_string());
        }

        // A degraded install is retried next time instead of being served from the cache.
        if let Some((dir, key)) = &cached
            && prepared.limitation.is_none()
        {
            cache::store(dir, key, self.partition, &facts);
        }

        tracing::info!(status = ?facts.status, symbols = facts.symbols.len(), diagnostics = facts.diagnostics.len(), limitations = facts.limitations.len(), "semantic facts collected");
        Ok(facts)
    }

    fn run(
        &self,
        profile: &Profile,
        binary: &Path,
        files: &[Target],
        facts: &mut SemanticFacts,
    ) -> Result<(), LspError> {
        let mut server = LspServer::spawn(binary, profile.args, self.root, &profile.env)?;
        let capabilities = initialize(&mut server, self.root, profile, facts)?;
        let mut texts = HashMap::new();
        let mut until = None;
        for file in files {
            if until.is_some_and(|until| Instant::now() >= until) {
                facts
                    .limitations
                    .push(format!("{}: not queried (time budget)", file.path));
                continue;
            }

            self.query_file(
                &mut server,
                &capabilities,
                file,
                facts,
                &mut texts,
                &mut until,
            )?;
        }
        server.shutdown();
        Ok(())
    }

    fn query_file(
        &self,
        server: &mut LspServer,
        capabilities: &Value,
        file: &Target,
        facts: &mut SemanticFacts,
        texts: &mut HashMap<PathBuf, String>,
        until: &mut Option<Instant>,
    ) -> Result<(), LspError> {
        let has = |name: &str| !capabilities[name].is_null() && capabilities[name] != false;
        let uri = file_uri(&self.root.join(file.path.as_str()));
        let Ok(text) = fs::read_to_string(self.root.join(file.path.as_str())) else {
            facts
                .limitations
                .push(format!("{}: not materialized or not UTF-8", file.path));
            return Ok(());
        };
        server.notify(
            "textDocument/didOpen",
            &json!({"textDocument": {
                "uri": uri, "languageId": file.language_id, "version": 1, "text": text
            }}),
        )?;
        let symbols = server.request(
            "textDocument/documentSymbol",
            &json!({"textDocument": {"uri": uri}}),
            READY_TIMEOUT,
        )?;
        let until = *until.get_or_insert_with(|| Instant::now() + QUERY_BUDGET);
        for symbol in changed_symbols(&symbols, &file.ranges) {
            let (queried, limits) = (&facts.symbols, &mut facts.limitations);
            if skipped(queried, limits, &file.path, &symbol.name, until) {
                continue;
            }

            let Edge::Ref(definition) = self.locate(&uri, &symbol.range, texts) else {
                continue;
            };
            let position = json!({"textDocument": {"uri": uri}, "position": symbol.position});
            let offered = (has("referencesProvider"), has("documentHighlightProvider"));
            let (references, readers, writers) =
                self.references(server, &position, offered, &symbol.kind, texts);
            let mut query = |capability: &str, method: &str, params: &Value| {
                if !has(capability) {
                    return EdgeSet::unsupported();
                }

                match server.request(method, params, QUERY_TIMEOUT) {
                    Ok(result) => self.edges(&result, texts),
                    Err(err) => EdgeSet::failed(err.to_string()),
                }
            };
            let implementations = query(
                "implementationProvider",
                "textDocument/implementation",
                &position,
            );
            let (callers, calling_constructs, outgoing) = if !symbol.callable {
                let none = EdgeSet::not_applicable;
                (none(), Vec::new(), none())
            } else if has("callHierarchyProvider") {
                self.call_hierarchy(server, &position, texts)
            } else {
                let none = EdgeSet::unsupported;
                (none(), Vec::new(), none())
            };
            let type_definitions = if has("definitionProvider") {
                let utf8 = facts.position_encoding.as_deref() == Some("utf-8");
                let names = type_names(&text, &symbol.range, &symbol.name, utf8, MAX_TYPES);
                self.definitions(server, &uri, names, texts)
            } else {
                EdgeSet::unsupported()
            };
            facts.symbols.push(SymbolFacts {
                name: symbol.name,
                kind: symbol.kind,
                container: symbol.container,
                path_class: classify(definition.path.as_str(), file.generated),
                definition,
                references,
                readers,
                writers,
                callers,
                calling_constructs,
                callees: outgoing,
                type_definitions,
                implementations,
            });
        }
        if has("diagnosticProvider") {
            match self.diagnostics(server, &file.path, &uri, texts) {
                Ok((found, limits)) => {
                    facts.diagnostics.extend(found);
                    facts.limitations.extend(limits);
                }
                Err(err) => facts
                    .limitations
                    .push(format!("{}: {DIAGNOSTICS_FAILED}: {err}", file.path)),
            }
        } else {
            let missing = "server does not offer pull diagnostics".to_owned();
            if !facts.limitations.contains(&missing) {
                facts.limitations.push(missing);
            }
        }

        Ok(())
    }
}

/// Whether a symbol is left unqueried, with the reason as a limitation: the symbol cap or the
/// server's time budget.
fn skipped<T>(
    queried: &[T],
    limitations: &mut Vec<String>,
    path: &RepoPath,
    name: &str,
    until: Instant,
) -> bool {
    let reason = if queried.len() == MAX_SYMBOLS {
        "symbol cap"
    } else if Instant::now() >= until {
        "time budget"
    } else {
        return false;
    };
    limitations.push(format!("{path}: {name} not queried ({reason})"));
    true
}

#[cfg(test)]
mod tests {
    use std::slice;

    use super::*;

    #[test]
    fn a_symbol_past_the_cap_or_the_time_budget_is_skipped_and_says_why() {
        let path: RepoPath = "src/a.rs".parse().expect("path");
        let later = Instant::now() + Duration::from_secs(60);
        let mut limitations = Vec::new();
        let (none, all) = ([(); 0], [(); MAX_SYMBOLS]);
        assert!(!skipped(&none, &mut limitations, &path, "f", later));
        assert!(skipped(&all, &mut limitations, &path, "g", later));
        assert!(skipped(&none, &mut limitations, &path, "h", Instant::now()));
        assert_eq!(
            limitations,
            [
                "src/a.rs: g not queried (symbol cap)",
                "src/a.rs: h not queried (time budget)"
            ]
        );
    }

    #[test]
    fn zero_count_hunk_selects_the_insertion_point() {
        let file: FileChange = serde_json::from_value(json!({
            "status": "MODIFIED",
            "old": {"path": "a.rs", "mode": "100644", "oid": format!("sha1:{}", "a".repeat(40)),
                    "content": "TEXT", "line_count": 9},
            "new": {"path": "a.rs", "mode": "100644", "oid": format!("sha1:{}", "b".repeat(40)),
                    "content": "TEXT", "line_count": 8},
            "similarity": null, "generated": false,
            "hunks": [{"old_start": 4, "old_lines": 1, "new_start": 3, "new_lines": 0}],
            "limitations": []
        }))
        .expect("file change");
        let head = targets(slice::from_ref(&file), SourceSide::Head);
        assert_eq!(head[&Language::Rust][0].ranges, [(3, 3)]);
        let base = targets(&[file], SourceSide::DiffBase);
        assert_eq!(base[&Language::Rust][0].ranges, [(4, 4)]);
    }
}
