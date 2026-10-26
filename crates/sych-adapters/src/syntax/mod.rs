//! Syntax facts of changed Rust, TypeScript, Go and Python files on each side (spec §5.3,
//! §10.6). Parsing runs in process on pinned blobs; nothing from the repository is executed.

mod cfg;
mod go;
mod logging;
mod python;
mod react;
mod rust;
mod tree;

use std::collections::BTreeMap;

use sych_core::broker::UNRESOLVED_IMPORT;
use sych_core::config::Effect;
use sych_core::{
    CodeRef, Language, Producer, RepoPath, SchemaVersion, SnapshotReader, SourceSide, SyntaxFact,
    SyntaxFacts, SyntaxKind, source_language,
};

use crate::git::LocalSnapshot;

/// Facts per file are capped; the rest is named in limitations.
const MAX_FACTS_PER_FILE: usize = 200;

/// A fact before it is bound to the snapshot: 1-based inclusive lines.
pub(crate) struct Raw {
    pub kind: SyntaxKind,
    pub start: u32,
    pub end: u32,
    pub symbol: Option<String>,
    pub detail: String,
    /// The name a fact is about beyond its span, e.g. the type of a `Debug`-logged value.
    pub subject: Option<String>,
    /// Condition of the nearest enclosing `if`.
    pub guard: Option<String>,
}

fn producer(language: Language) -> Producer {
    let (name, version) = match language {
        Language::Rust => ("syn", "2"),
        Language::Typescript => ("oxc", "0.139"),
        Language::Go => ("tree-sitter-go", "0.25"),
        Language::Python => ("tree-sitter-python", "0.25"),
    };
    Producer {
        name: name.into(),
        version: version.into(),
        config_hash: None,
    }
}

/// `(call name, registry symbol)` of the registered wrappers of `language`: a call is matched
/// by the last segment of the symbol, e.g. `AppendEvent` of `pkg.Store.AppendEvent`.
fn wrappers(effects: &[Effect], language: Language) -> Vec<(String, String)> {
    let name = serde_json::to_value(language).unwrap_or_default();
    effects
        .iter()
        .filter(|e| Some(e.matcher.language.as_str()) == name.as_str())
        .map(|e| {
            let symbol = &e.matcher.symbol;
            let call = symbol.rsplit(['.', '/', ':']).next().unwrap_or(symbol);
            (call.to_owned(), symbol.clone())
        })
        .collect()
}

/// One document per side and language with changed files; calls of the wrappers in the
/// effect registry are `RegisteredEffectCall`.
pub fn collect(snapshot: &LocalSnapshot, effects: &[Effect]) -> Vec<SyntaxFacts> {
    let mut docs: BTreeMap<(SourceSide, Language), SyntaxFacts> = BTreeMap::new();
    for side in [SourceSide::DiffBase, SourceSide::Head] {
        for file in &snapshot.manifest.files {
            let Some(file_side) = file.side(side) else {
                continue;
            };
            let Some((language, _)) = source_language(file_side.path.as_str()) else {
                continue;
            };
            let doc = docs.entry((side, language)).or_insert_with(|| SyntaxFacts {
                schema_version: SchemaVersion,
                source_snapshot_id: snapshot.snapshot.id.clone(),
                side,
                language,
                producer: producer(language),
                files: BTreeMap::new(),
                facts: Vec::new(),
                limitations: vec![
                    "macros are not expanded and types are not resolved: facts are shapes, and \
                     a missing fact is not proof of absence"
                        .into(),
                ],
            });
            file_facts(
                snapshot,
                side,
                &file_side.path,
                &file.changed_lines(side),
                &wrappers(effects, language),
                doc,
            );
        }
    }
    docs.into_values().collect()
}

fn file_facts(
    snapshot: &LocalSnapshot,
    side: SourceSide,
    path: &RepoPath,
    changed: &[(u32, u32)],
    wrappers: &[(String, String)],
    doc: &mut SyntaxFacts,
) {
    let text = match snapshot.blob(side, path) {
        Ok((_, text)) => text,
        Err(err) => {
            doc.files
                .insert(path.to_string(), format!("unreadable: {err}"));
            return;
        }
    };
    let parsed = match doc.language {
        Language::Rust => rust::facts(&text, &crate_features(snapshot, side, path)),
        Language::Typescript => react::facts(&text, path.as_str(), wrappers),
        Language::Go => go::facts(&text, &package_types(snapshot, side, path), wrappers),
        Language::Python => python::facts(&text, wrappers),
    };
    let raws = match parsed {
        Ok(raws) => {
            doc.files.insert(path.to_string(), "ok".into());
            raws
        }
        Err(err) => {
            tracing::warn!(%path, side = ?side, %err, "syntax facts skipped: file does not parse");
            doc.files.insert(path.to_string(), err);
            return;
        }
    };
    if raws.len() > MAX_FACTS_PER_FILE {
        doc.limitations.push(format!(
            "{path}: {MAX_FACTS_PER_FILE} of {} facts kept",
            raws.len()
        ));
    }

    for raw in raws.into_iter().take(MAX_FACTS_PER_FILE) {
        let Ok(code_ref) = snapshot.code_ref(side, path, raw.start, raw.end) else {
            doc.limitations.push(format!(
                "{path}:{}-{}: span outside the blob",
                raw.start, raw.end
            ));
            continue;
        };
        let in_change = changed.iter().any(|&(s, e)| s <= raw.end && raw.start <= e);
        let mut subject = raw.subject;
        let related = match (raw.kind, &subject) {
            (SyntaxKind::CustomHookCall, _) => definitions(snapshot, side, &raw.detail),
            (SyntaxKind::DebugFormatted, Some(ty)) => type_definitions(snapshot, side, ty),
            (SyntaxKind::RegisteredEffectCall, _) if doc.language != Language::Rust => {
                wrapper_definitions(snapshot, side, doc.language, &raw.detail)
            }
            (SyntaxKind::Import, _) if doc.language == Language::Python => {
                let found = python_module(snapshot, side, path, &raw.detail);
                if found.is_none() && !python_stdlib(&raw.detail) {
                    subject = Some(UNRESOLVED_IMPORT.into());
                }

                found.unwrap_or_default()
            }
            _ => Vec::new(),
        };
        doc.facts.push(SyntaxFact {
            kind: raw.kind,
            code_ref,
            symbol: raw.symbol,
            detail: raw.detail,
            in_change,
            related,
            subject,
            guard: raw.guard,
        });
    }
}

/// Struct fields and interfaces of the Go package in `path`'s directory on `side`: every
/// `.go` file directly in it, found through its `package` clause.
fn package_types(snapshot: &LocalSnapshot, side: SourceSide, path: &RepoPath) -> go::PackageTypes {
    let dir = path.as_str().rsplit_once('/').map_or("", |(dir, _)| dir);
    let scope: Vec<RepoPath> = dir.parse().into_iter().collect();
    let hits = snapshot
        .search(side, "package ", &scope, MAX_PACKAGE_FILES)
        .unwrap_or_default();
    let mut files: Vec<&RepoPath> = hits
        .iter()
        .map(|h| &h.path)
        .filter(|p| {
            let rest = p.as_str().strip_prefix(dir).unwrap_or(p.as_str());
            p.as_str()
                .rsplit_once('.')
                .is_some_and(|(_, ext)| ext == "go")
                && !rest.trim_start_matches('/').contains('/')
        })
        .collect();
    files.dedup();
    let texts: Vec<_> = files
        .into_iter()
        .filter_map(|p| snapshot.blob(side, p).ok().map(|(_, text)| text))
        .collect();
    go::PackageTypes::new(texts.iter().map(AsRef::as_ref))
}

/// Package clauses read per Go package; a larger package's other files add no types.
const MAX_PACKAGE_FILES: usize = 64;

fn python_stdlib(module: &str) -> bool {
    let top = module.split(['.', ' ']).next().unwrap_or(module);
    include_str!("python/stdlib.txt").lines().any(|m| m == top)
}

/// The file of a Python module on `side`: `a/b.py` or `a/b/__init__.py`, from the root, a
/// `src/` layout or, for a relative import, the importing file's package; a namespace package
/// directory resolves without a file. `None` when the snapshot has none of them.
fn python_module(
    snapshot: &LocalSnapshot,
    side: SourceSide,
    importer: &RepoPath,
    module: &str,
) -> Option<Vec<CodeRef>> {
    let module = module.trim_end_matches(" (*)");
    let dots = module.len() - module.trim_start_matches('.').len();
    let rel = module.trim_start_matches('.').replace('.', "/");
    let bases: Vec<String> = if dots > 0 {
        let mut dir = importer.as_str().rsplit_once('/').map_or("", |(d, _)| d);
        for _ in 1..dots {
            dir = dir.rsplit_once('/').map_or("", |(d, _)| d);
        }
        vec![dir.to_owned()]
    } else {
        vec![String::new(), "src".into()]
    };
    let stems: Vec<String> = bases
        .iter()
        .map(|base| {
            [base.as_str(), rel.as_str()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect();
    let file = stems
        .iter()
        .flat_map(|stem| [format!("{stem}.py"), format!("{stem}/__init__.py")])
        .filter_map(|candidate| candidate.parse::<RepoPath>().ok())
        .find_map(|candidate| snapshot.code_ref(side, &candidate, 1, 1).ok());
    if let Some(file) = file {
        return Some(vec![file]);
    }

    stems
        .iter()
        .filter_map(|stem| stem.parse::<RepoPath>().ok())
        .any(|dir| snapshot.is_dir(side, &dir))
        .then(Vec::new)
}

/// Where a registered wrapper is defined on `side`, so its current implementation is read,
/// not its registry entry.
fn wrapper_definitions(
    snapshot: &LocalSnapshot,
    side: SourceSide,
    language: Language,
    symbol: &str,
) -> Vec<CodeRef> {
    let name = symbol.rsplit(['.', '/', ':']).next().unwrap_or(symbol);
    let forms = match language {
        Language::Go => vec![format!(") {name}("), format!("func {name}(")],
        Language::Python => vec![format!("def {name}(")],
        _ => return definitions(snapshot, side, name),
    };
    let mut found = Vec::new();
    for form in forms {
        for hit in snapshot.search(side, &form, &[], 3).unwrap_or_default() {
            if !found.contains(&hit) {
                found.push(hit);
            }
        }
    }
    found
}

/// Default features of the crate owning `path`: its nearest `Cargo.toml` on `side`.
fn crate_features(snapshot: &LocalSnapshot, side: SourceSide, path: &RepoPath) -> Vec<String> {
    let mut dir = path.as_str();
    while let Some((parent, _)) = dir.rsplit_once('/') {
        dir = parent;
        if let Some(features) = manifest_features(snapshot, side, &format!("{dir}/Cargo.toml")) {
            return features;
        }
    }
    manifest_features(snapshot, side, "Cargo.toml").unwrap_or_default()
}

fn manifest_features(
    snapshot: &LocalSnapshot,
    side: SourceSide,
    path: &str,
) -> Option<Vec<String>> {
    let path: RepoPath = path.parse().ok()?;
    let (_, text) = snapshot.blob(side, &path).ok()?;
    Some(cfg::default_features(&text))
}

/// Where type `name` is defined on `side`; none when it comes from a dependency.
fn type_definitions(snapshot: &LocalSnapshot, side: SourceSide, name: &str) -> Vec<CodeRef> {
    let mut found = Vec::new();
    for form in ["struct", "enum", "type"]
        .iter()
        .flat_map(|k| [" {", "<", "(", ";", " ="].map(|end| format!("{k} {name}{end}")))
    {
        for hit in snapshot.search(side, &form, &[], 3).unwrap_or_default() {
            if !found.contains(&hit) {
                found.push(hit);
            }
        }
    }
    found
}

/// Where `hook` is defined on `side`, by literal search for its declaration forms; the
/// wrapper is then reviewed as the current implementation, not by its name.
fn definitions(snapshot: &LocalSnapshot, side: SourceSide, hook: &str) -> Vec<CodeRef> {
    let mut found = Vec::new();
    for form in [
        format!("function {hook}("),
        format!("const {hook} ="),
        format!("function {hook}<"),
    ] {
        for hit in snapshot.search(side, &form, &[], 3).unwrap_or_default() {
            if !found.contains(&hit) {
                found.push(hit);
            }
        }
    }
    found
}
