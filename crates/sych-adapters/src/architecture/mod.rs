//! The architecture model of each project a change touches, on its diff base and its head: a
//! pinned SCIP indexer resolves every name in the whole project, and the syntax of each indexed
//! file says what the code does with what it names (see `sych_core::architecture`). Both sides
//! are written from pinned Git objects, never from a working tree, and no repository code runs.

mod build;
mod facts;
mod index;
mod scip;
mod source;

pub use build::Built;
pub use facts::facts;

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use sych_core::{BackendStatus, Language, RepoPath, SourceSide, glob_match, sha256};

use crate::git::{LocalSnapshot, Materialized};
use crate::lint::easement::{self, Failure, under};
use crate::semantic::Options;
use crate::tools;

/// Projects analysed in one run; a change across more is analysed in part.
pub const MAX_PROJECTS: usize = 4;
/// Source files of one project; a larger one is not indexed in a review.
pub const MAX_FILES: usize = 5_000;
/// Index files kept in the cache; the oldest go first.
const CACHE_ENTRIES: usize = 64;

/// A materialized side: its root directory and what was written there.
pub type Side<'a> = (&'a Path, &'a Materialized);

/// One project and its model on each side.
pub struct Analysis {
    pub language: Language,
    /// The project's directory, `""` for the repository's root.
    pub dir: String,
    pub head: Result<Built, Failure>,
    /// `None` when the diff base has no such project.
    pub base: Option<Result<Built, Failure>>,
    /// What the runs cost, said once.
    pub limitations: Vec<String>,
}

/// The projects the diff touches, by the source and manifest files it changes, root first,
/// but those whose directory a glob of `skip` (labelled fixture data) matches; and how many
/// more were left out.
pub fn projects(
    snapshot: &LocalSnapshot,
    head: Side<'_>,
    base: Option<Side<'_>>,
    skip: &[String],
) -> (Vec<(Language, String)>, usize) {
    let mut found = BTreeSet::new();
    for file in &snapshot.manifest.files {
        for (side, workspace) in [(&file.new, Some(head)), (&file.old, base)] {
            let (Some(side), Some(workspace)) = (side, workspace) else {
                continue;
            };
            let path = side.path.as_str();
            if let Some(language) = easement::env::language(path)
                && let Some(dir) = root(language, path, workspace)
            {
                found.insert((dir.len(), dir, language));
            }
        }
    }
    let fixture = |dir: &str| !dir.is_empty() && skip.iter().any(|g| glob_match(g, dir));
    let mut found: Vec<(Language, String)> = found
        .into_iter()
        .filter(|(_, dir, _)| !fixture(dir))
        .map(|(_, dir, l)| (l, dir))
        .collect();
    let more = found.len().saturating_sub(MAX_PROJECTS);
    found.truncate(MAX_PROJECTS);
    (found, more)
}

/// The directory of the project `path` is in: the outermost Cargo workspace or nearest
/// `Cargo.toml`, the nearest `go.mod`, `tsconfig.json` or `package.json`, the nearest Python
/// project file or the repository's root.
fn root(language: Language, path: &str, (root, blobs): Side<'_>) -> Option<String> {
    let file = |dir: &str, name: &str| {
        if dir.is_empty() {
            name.to_owned()
        } else {
            format!("{dir}/{name}")
        }
    };
    let has = |dir: &str, name: &str| {
        file(dir, name)
            .parse::<RepoPath>()
            .is_ok_and(|p| blobs.blobs.contains_key(&p))
    };
    // A workspace's `Cargo.toml` says `[workspace]`; its members' do not.
    let workspace = |dir: &str| {
        fs::read_to_string(root.join(file(dir, "Cargo.toml")))
            .is_ok_and(|text| text.lines().any(|l| l.trim() == "[workspace]"))
    };
    let mut dirs = Vec::new();
    let mut rest = path;
    while let Some((parent, _)) = rest.rsplit_once('/') {
        dirs.push(parent);
        rest = parent;
    }
    dirs.push("");
    let markers: &[&str] = match language {
        Language::Rust => &["Cargo.toml"],
        Language::Go => &["go.mod"],
        Language::Typescript => &["tsconfig.json", "package.json"],
        Language::Python => &["pyproject.toml", "setup.py", "setup.cfg"],
    };
    let nearest = dirs.iter().find(|d| markers.iter().any(|m| has(d, m)));
    match (language, nearest) {
        (Language::Rust, Some(_)) => dirs
            .iter()
            .rev()
            .find(|d| has(d, "Cargo.toml") && workspace(d))
            .or(nearest)
            .map(|d| (*d).to_owned()),
        (Language::Python, None) => Some(String::new()),
        (_, nearest) => nearest.map(|d| (*d).to_owned()),
    }
}

/// Each project the diff touches, analysed on head and, where it is there too, on the diff
/// base; a project in labelled fixture data, which `skip` matches, is not. Tool failures are
/// reported inside.
#[tracing::instrument(level = "info", name = "architecture", skip_all)]
pub fn collect(snapshot: &LocalSnapshot, options: &Options<'_>, skip: &[String]) -> Vec<Analysis> {
    let touches = snapshot
        .manifest
        .files
        .iter()
        .flat_map(|f| f.new.iter().chain(&f.old))
        .any(|s| easement::env::language(s.path.as_str()).is_some());
    if !touches {
        return Vec::new();
    }

    let (head_dir, head) = easement::materialize(snapshot, SourceSide::Head);
    let (base_dir, base) = easement::materialize(snapshot, SourceSide::DiffBase);
    let Ok((head_root, head_blobs)) = &head else {
        let why = head.err().map(|e| e.to_string()).unwrap_or_default();
        tracing::warn!(%why, "architecture skipped: head not materialized");
        return Vec::new();
    };
    let base_side = base.as_ref().ok().map(|(root, m)| (root.as_path(), m));
    let (found, more) = projects(snapshot, (head_root, head_blobs), base_side, skip);
    let scratch = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(e) => {
            tracing::warn!(%e, "architecture skipped: no temporary directory");
            return Vec::new();
        }
    };
    let out = found
        .into_iter()
        .enumerate()
        .map(|(i, (language, dir))| {
            let scratch = scratch.path().join(format!("project-{i}"));
            let mut limitations = Vec::new();
            if more > 0 && i == 0 {
                limitations.push(format!(
                    "{more} more projects the change touches are not analysed: one review analyses {MAX_PROJECTS}"
                ));
            }

            let head = model(
                language,
                &dir,
                (head_root, head_blobs),
                &scratch.join("head"),
                options,
                &mut limitations,
            );
            // What the base run costs is already said for head.
            let base = match &base {
                Ok((root, blobs)) if has_project(language, blobs, &dir) => Some(model(
                    language,
                    &dir,
                    (root, blobs),
                    &scratch.join("base"),
                    options,
                    &mut Vec::new(),
                )),
                Ok(_) => None,
                Err(e) => Some(Err((BackendStatus::Failed, format!("diff base: {e}")))),
            };
            Analysis {
                language,
                dir,
                head,
                base,
                limitations,
            }
        })
        .collect();
    drop((head_dir, base_dir));
    out
}

fn has_project(language: Language, blobs: &Materialized, dir: &str) -> bool {
    blobs
        .blobs
        .keys()
        .any(|p| under(p.as_str(), dir) && easement::env::language(p.as_str()) == Some(language))
}

/// The model of the project on one side, from the cache when its inputs are the same.
fn model(
    language: Language,
    dir: &str,
    (root, blobs): Side<'_>,
    scratch: &Path,
    options: &Options<'_>,
    limitations: &mut Vec<String>,
) -> Result<Built, Failure> {
    let sources = blobs
        .blobs
        .keys()
        .filter(|p| under(p.as_str(), dir) && easement::env::language(p.as_str()) == Some(language))
        .count();
    if sources > MAX_FILES {
        return Err((
            BackendStatus::Unsupported,
            format!("{sources} {language:?} files, more than the {MAX_FILES} one review indexes"),
        ));
    }

    let started = Instant::now();
    let key = cache_key(language, dir, blobs, options);
    let cached = key.as_ref().and_then(|k| fs::read(k).ok());
    let hit = cached.is_some();
    let bytes = if let Some(bytes) = cached {
        bytes
    } else {
        let bytes = index::index(language, root, blobs, dir, scratch, options, limitations)?;
        if let Some(key) = &key {
            store(key, &bytes);
        }

        bytes
    };
    let sources = sources_of(language, dir, blobs.blobs.keys().map(RepoPath::as_str));
    let built = model_of(language, dir, &bytes, &root.join(dir), &sources)
        .map_err(|e| (BackendStatus::Failed, format!("index: {e}")))?;
    tracing::info!(
        ?language,
        project = dir,
        cached = hit,
        index_bytes = bytes.len(),
        files = built.model.files.len(),
        items = built.model.items.len(),
        references = built.model.references.len(),
        elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "architecture model built"
    );
    Ok(built)
}

/// The model of the `language` project in `dir` from its SCIP `index`, reading the files
/// under `project`, the project's directory on disk; `sources` are its source files.
pub fn model_of(
    language: Language,
    dir: &str,
    index: &[u8],
    project: &Path,
    sources: &[String],
) -> Result<Built, String> {
    let index = scip::read(index).map_err(|e| e.to_string())?;
    Ok(build::build(
        language,
        dir,
        &index,
        &|path: &str| fs::read_to_string(project.join(path)).ok(),
        sources,
    ))
}

/// The project-relative paths of `language`'s source files among `paths`, under `dir`.
pub fn sources_of<'p>(
    language: Language,
    dir: &str,
    paths: impl Iterator<Item = &'p str>,
) -> Vec<String> {
    paths
        .filter(|p| under(p, dir))
        .filter(|p| sych_core::source_language(p).is_some_and(|(l, _)| l == language))
        .map(|p| {
            if dir.is_empty() {
                p.to_owned()
            } else {
                p[dir.len() + 1..].to_owned()
            }
        })
        .collect()
}

/// Where the index of these inputs is kept: the indexer and toolchain, whether dependencies
/// are installed, and every blob of the project. `None` without a state directory.
fn cache_key(
    language: Language,
    dir: &str,
    blobs: &Materialized,
    options: &Options<'_>,
) -> Option<PathBuf> {
    let tool = indexer(language);
    let mut input = format!(
        "architecture-index v1\n{language:?}\n{dir}\n{tool} {}\n{}\n",
        tools::sole_pinned(tool).map_or("unpinned", |t| t.version.as_str()),
        options.tooling.dependency_install,
    );
    for (path, oid) in &blobs.blobs {
        if under(path.as_str(), dir) || !path.as_str().contains('/') {
            let _ = writeln!(input, "{} {}", oid.hex(), path.as_str());
        }
    }
    let directory = options.state_dir.join("architecture");
    fs::create_dir_all(&directory).ok()?;
    Some(directory.join(format!("{}.scip", sha256(input.as_bytes()).hex())))
}

/// The pinned indexer of `language`.
fn indexer(language: Language) -> &'static str {
    match language {
        Language::Rust => "rust-analyzer",
        Language::Go => "scip-go",
        Language::Typescript => "scip-typescript",
        Language::Python => "scip-python",
    }
}

/// Keeps `bytes` under `key`, and only the newest entries.
fn store(key: &Path, bytes: &[u8]) {
    let partial = key.with_extension("partial");
    if fs::write(&partial, bytes)
        .and_then(|()| fs::rename(&partial, key))
        .is_err()
    {
        let _ = fs::remove_file(&partial);
        return;
    }

    let Some(directory) = key.parent() else {
        return;
    };
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut kept: Vec<(SystemTime, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    kept.sort();
    let excess = kept.len().saturating_sub(CACHE_ENTRIES);
    for (_, path) in kept.into_iter().take(excess) {
        let _ = fs::remove_file(path);
    }
}
