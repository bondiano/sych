//! The architecture fixtures, the pinned indexers and small queries of a model.

use std::path::{Path, PathBuf};
use std::{env, fs};

use sych_adapters::architecture::{self, Built};
use sych_adapters::git::{LocalSnapshot, SnapshotRequest};
use sych_adapters::semantic::Options;
use sych_adapters::tools::{ToolCache, ToolError, default_cache_root};
use sych_core::Language;
use sych_core::architecture::{ItemId, ItemKind, Model};
use sych_core::config::Tooling;
use tokio::runtime;

pub const LANGUAGES: [(Language, &str); 4] = [
    (Language::Rust, "rust"),
    (Language::Go, "go"),
    (Language::Typescript, "typescript"),
    (Language::Python, "python"),
];

pub fn fixture_dir(language: &str, variant: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/architecture")
        .join(language)
        .join(variant)
}

pub fn fixture(language: Language, name: &str, variant: &str) -> Built {
    let dir = fixture_dir(name, variant);
    let index = fs::read(dir.join("index.scip")).expect("a committed index: run the bless test");
    built(language, &index, &dir)
}

/// The model of the project at `dir` from `index`, with every source file under it.
pub fn built(language: Language, index: &[u8], dir: &Path) -> Built {
    let paths: Vec<String> = sources(dir).into_iter().map(|(p, _)| p).collect();
    let sources = architecture::sources_of(language, "", paths.iter().map(String::as_str));
    architecture::model_of(language, "", index, dir, &sources).expect("an index")
}

/// Every pinned tool, fetched into the user's tool cache on first use.
pub fn with_tools<T>(run: impl FnOnce(&Options<'_>) -> T) -> T {
    let root = default_cache_root().expect("a tool cache directory");
    let cache = ToolCache::new(root);
    let runtime = runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let resolve = |tool, target: &str| {
        runtime
            .block_on(cache.ensure(tool, target))
            .map_err(|e| ToolError::Download(e.to_string()))
    };
    let state = tempfile::tempdir().expect("state");
    run(&Options {
        resolve: &resolve,
        cache: None,
        tooling: &Tooling::default(),
        state_dir: state.path(),
    })
}

/// A repository whose first commit has only a README and whose second adds `files`.
pub fn snapshot(files: &[(String, Vec<u8>)]) -> (tempfile::TempDir, LocalSnapshot) {
    let repo = tempfile::tempdir().expect("repo");
    super::repo::git(repo.path(), &["init", "-q", "-b", "main"]);
    super::repo::commit(repo.path(), &[("README.md", "fixture\n")], "base");
    for (path, bytes) in files {
        let path = repo.path().join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, bytes).expect("write");
    }
    super::repo::commit(repo.path(), &[], "head");
    let snapshot = LocalSnapshot::capture(&SnapshotRequest {
        repo: repo.path(),
        base: "HEAD~1",
        head: "HEAD",
        diff_base: None,
        max_blob_bytes: 1 << 20,
    })
    .expect("snapshot");
    (repo, snapshot)
}

pub fn sources(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        for entry in fs::read_dir(&at).expect("a directory") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|n| n != "index.scip") {
                let relative = path
                    .strip_prefix(dir)
                    .expect("inside")
                    .display()
                    .to_string();
                out.push((relative, fs::read(&path).expect("read")));
            }
        }
    }
    out
}

pub fn named(model: &Model, name: &str) -> Vec<ItemId> {
    (0..model.items.len())
        .map(ItemId::new)
        .filter(|id| model.items[id.index()].name == name)
        .collect()
}

pub fn one(model: &Model, name: &str) -> ItemId {
    match named(model, name).as_slice() {
        [id] => *id,
        found => panic!("one item named {name}: {found:?}"),
    }
}

pub fn kind_of(model: &Model, name: &str, kind: ItemKind) -> ItemId {
    let found: Vec<ItemId> = named(model, name)
        .into_iter()
        .filter(|id| model.items[id.index()].kind == kind)
        .collect();
    match found.as_slice() {
        [id] => *id,
        _ => panic!("one {kind:?} named {name}: {found:?}"),
    }
}

pub fn files(list: &[(&str, &str)]) -> Vec<(String, Vec<u8>)> {
    list.iter()
        .map(|(p, t)| ((*p).to_owned(), t.as_bytes().to_vec()))
        .collect()
}
