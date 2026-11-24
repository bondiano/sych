//! The architecture model of the fixtures under `fixtures/architecture/<language>/<variant>`,
//! built from the SCIP index committed beside them; and, with `--ignored`, the pinned indexers
//! themselves: they write those indexes and never run repository code.

use std::path::{Path, PathBuf};
use std::{env, fs};

use sych_adapters::architecture::{self, Built};
use sych_adapters::git::{LocalSnapshot, SnapshotRequest};
use sych_adapters::semantic::Options;
use sych_adapters::tools::{ToolCache, ToolError, default_cache_root};
use sych_core::Language;
use sych_core::architecture::{Graph, ItemId, ItemKind, Model, Wrapper};
use sych_core::config::Tooling;
use tokio::runtime;

mod common;

const LANGUAGES: [(Language, &str); 4] = [
    (Language::Rust, "rust"),
    (Language::Go, "go"),
    (Language::Typescript, "typescript"),
    (Language::Python, "python"),
];

fn fixture_dir(language: &str, variant: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/architecture")
        .join(language)
        .join(variant)
}

fn fixture(language: Language, name: &str, variant: &str) -> Built {
    let dir = fixture_dir(name, variant);
    let index = fs::read(dir.join("index.scip")).expect("a committed index: run the bless test");
    architecture::model_of(language, "", &index, &dir).expect("an index")
}

/// Every pinned tool, fetched into the user's tool cache on first use.
fn with_tools<T>(run: impl FnOnce(&Options<'_>) -> T) -> T {
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
fn snapshot(files: &[(String, Vec<u8>)]) -> (tempfile::TempDir, LocalSnapshot) {
    let repo = tempfile::tempdir().expect("repo");
    common::repo::git(repo.path(), &["init", "-q", "-b", "main"]);
    common::repo::commit(repo.path(), &[("README.md", "fixture\n")], "base");
    for (path, bytes) in files {
        let path = repo.path().join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, bytes).expect("write");
    }
    common::repo::commit(repo.path(), &[], "head");
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

fn sources(dir: &Path) -> Vec<(String, Vec<u8>)> {
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

/// Rewrites each fixture's `index.scip` with the pinned indexer. Run after changing a fixture:
/// `cargo test -p sych-adapters --test architecture -- --ignored bless`.
#[test]
#[ignore = "downloads the pinned indexers and rewrites fixtures/architecture/*/*/index.scip"]
fn bless_fixture_indexes() {
    with_tools(|options| {
        for (language, name) in LANGUAGES {
            for variant in ["smelly", "clean"] {
                let dir = fixture_dir(name, variant);
                let (_repo, snapshot) = snapshot(&sources(&dir));
                let found = architecture::collect(&snapshot, options);
                let [analysis] = found.as_slice() else {
                    panic!("{name}/{variant}: one project, {} found", found.len());
                };
                assert_eq!(analysis.language, language);
                assert!(analysis.base.is_none(), "the base has no project");
                if let Err((_, why)) = &analysis.head {
                    panic!("{name}/{variant}: {why}");
                }

                let cached: Vec<PathBuf> = fs::read_dir(options.state_dir.join("architecture"))
                    .expect("the cache")
                    .map(|e| e.expect("an entry").path())
                    .collect();
                let [index] = cached.as_slice() else {
                    panic!("one index cached: {cached:?}");
                };
                fs::rename(index, dir.join("index.scip")).expect("bless");
            }
        }
    });
}

fn named(model: &Model, name: &str) -> Vec<ItemId> {
    (0..model.items.len())
        .map(ItemId::new)
        .filter(|id| model.items[id.index()].name == name)
        .collect()
}

fn one(model: &Model, name: &str) -> ItemId {
    match named(model, name).as_slice() {
        [id] => *id,
        found => panic!("one item named {name}: {found:?}"),
    }
}

/// The names each fixture writes, by language: the queue's collection field, its getter, the
/// function of `app` that uses them, and the lines of `app`'s imports.
struct Names {
    items: &'static str,
    limit: &'static str,
    getter: &'static str,
    run: &'static str,
    app: &'static str,
    imports: [u32; 2],
}

fn names(language: Language) -> Names {
    match language {
        Language::Rust => Names {
            items: "Queue::items",
            limit: "Queue::limit",
            getter: "Queue::items",
            run: "run",
            app: "src/app.rs",
            imports: [1, 2],
        },
        Language::Go => Names {
            items: "Queue::Items",
            limit: "Queue::Limit",
            getter: "Queue::View",
            run: "Run",
            app: "app/app.go",
            imports: [4, 5],
        },
        Language::Typescript => Names {
            items: "Queue::items",
            limit: "Queue::limit",
            getter: "Queue::view",
            run: "run",
            app: "src/app.ts",
            imports: [1, 2],
        },
        Language::Python => Names {
            items: "Queue::items",
            limit: "Queue::limit",
            getter: "Queue::view",
            run: "run",
            app: "smelly/app.py",
            imports: [1, 2],
        },
    }
}

fn kind_of(model: &Model, name: &str, kind: ItemKind) -> ItemId {
    let found: Vec<ItemId> = named(model, name)
        .into_iter()
        .filter(|id| model.items[id.index()].kind == kind)
        .collect();
    match found.as_slice() {
        [id] => *id,
        _ => panic!("one {kind:?} named {name}: {found:?}"),
    }
}

#[test]
fn a_method_belongs_to_its_type_and_a_getter_is_seen_through_in_every_language() {
    for (language, name) in LANGUAGES {
        let built = fixture(language, name, "smelly");
        let model = &built.model;
        assert!(built.unparsed.is_empty(), "{name}: {:?}", built.unparsed);
        let n = names(language);
        let queue = kind_of(model, "Queue", ItemKind::Type);
        let items = kind_of(model, n.items, ItemKind::Field);
        assert_eq!(model.items[items.index()].parent, Some(queue), "{name}");
        let getter = kind_of(model, n.getter, ItemKind::Function);
        assert_eq!(
            model.wrappers.get(&getter),
            Some(&Wrapper::Accessor {
                lent: vec![items],
                copied: vec![],
                to_read: true
            }),
            "{name}"
        );
        let full = one(
            model,
            if language == Language::Go {
                "Queue::Full"
            } else {
                "Queue::full"
            },
        );
        assert!(model.items[full.index()].takes_self, "{name}");
        assert!(!model.wrappers.contains_key(&full), "{name}: Full computes");

        if language != Language::Go {
            let timers = one(model, "Timers");
            let fire = &model.items[one(model, "Timers::fire").index()];
            assert_eq!(fire.parent, Some(timers), "{name}");
            assert!(
                model.tree.display(fire.owner).ends_with("kernel::timers"),
                "{name}"
            );
        }
    }
}

#[test]
fn what_code_does_with_a_field_decides_whether_it_only_reads_it() {
    for (language, name) in LANGUAGES {
        let model = fixture(language, name, "smelly").model;
        let n = names(language);
        let run = kind_of(&model, n.run, ItemKind::Function);
        let by_run = |item: ItemId| -> Vec<bool> {
            model
                .references
                .iter()
                .filter(|r| r.holder == Some(run) && r.item == item)
                .map(|r| r.copies)
                .collect()
        };
        let items = kind_of(&model, n.items, ItemKind::Field);
        let limit = kind_of(&model, n.limit, ItemKind::Field);
        assert!(
            by_run(items).contains(&false),
            "{name}: app adds to the items"
        );
        assert_eq!(by_run(limit), [true], "{name}: app only reads the limit");
        let app = model
            .files
            .iter()
            .position(|f| f.path == n.app)
            .expect("app");
        let on_imports = model
            .references
            .iter()
            .filter(|r| r.file.index() == app && n.imports.contains(&r.line))
            .count();
        assert_eq!(on_imports, 0, "{name}: an import is no use");
        let tests: Vec<bool> = model
            .references
            .iter()
            .filter(|r| r.item == items && model.files[r.file.index()].test || r.test)
            .map(|r| r.test)
            .collect();
        assert!(
            !tests.is_empty() && tests.iter().all(|t| *t),
            "{name}: {tests:?}"
        );
        assert!(by_run(items).iter().chain(&by_run(limit)).count() > 0);
        assert!(
            model
                .references
                .iter()
                .filter(|r| r.holder == Some(run))
                .all(|r| !r.test),
            "{name}: app is no test code"
        );
    }
}

#[test]
fn things_that_know_each_other_across_modules_are_one_tangle_until_the_change_unties_them() {
    for (language, name) in LANGUAGES {
        for variant in ["smelly", "clean"] {
            let model = fixture(language, name, variant).model;
            let graph = Graph::new(&model);
            let tangles: Vec<Vec<String>> = graph
                .tangles
                .iter()
                .map(|members| {
                    let mut names: Vec<String> = members
                        .iter()
                        .map(|m| {
                            model.items[graph.things[m.index()].item.index()]
                                .name
                                .clone()
                        })
                        .collect();
                    names.sort();
                    names
                })
                .collect();
            let expected: Vec<Vec<String>> = match (language, variant) {
                // Go forbids a cycle of packages: its tangle is a ring of types in one.
                (Language::Go, "smelly") => vec![(0..8).map(|i| format!("T{i}")).collect()],
                (Language::Go, _) => vec![(0..7).map(|i| format!("T{i}")).collect()],
                (_, "smelly") => vec![vec!["Tasks".into(), "Timers".into()]],
                _ => Vec::new(),
            };
            assert_eq!(tangles, expected, "{name}/{variant}");
        }
    }
}

#[test]
fn typed_fixtures_leave_no_member_of_the_project_unresolved() {
    for (language, name) in LANGUAGES {
        let built = fixture(language, name, "smelly");
        assert!(built.members > 0, "{name}");
        assert_eq!(built.unresolved, 0, "{name}");

        assert!(built.unknowns.is_empty(), "{name}: {:?}", built.unknowns);
    }
}

fn files(list: &[(&str, &str)]) -> Vec<(String, Vec<u8>)> {
    list.iter()
        .map(|(p, t)| ((*p).to_owned(), t.as_bytes().to_vec()))
        .collect()
}

#[test]
fn a_change_touches_the_outermost_cargo_workspace_and_the_nearest_project_of_the_others() {
    let (_repo, snapshot) = snapshot(&files(&[
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/a\"]\n"),
        ("crates/a/Cargo.toml", "[package]\nname = \"a\"\n"),
        ("crates/a/src/lib.rs", "pub fn a() {}\n"),
        ("svc/go.mod", "module example.com/svc\n"),
        ("svc/x/x.go", "package x\n"),
        ("web/package.json", "{}\n"),
        ("web/app/tsconfig.json", "{}\n"),
        ("web/app/src/a.ts", "export const a = 1;\n"),
        ("web/lib/b.js", "export const b = 1;\n"),
        ("tools/gen.py", "x = 1\n"),
    ]));
    let tree = tempfile::tempdir().expect("tree");
    let head = snapshot
        .materialize(sych_core::SourceSide::Head, tree.path(), 1 << 20)
        .expect("materialized");
    let (found, more) = architecture::projects(&snapshot, (tree.path(), &head), None);
    let found: Vec<(Language, &str)> = found.iter().map(|(l, d)| (*l, d.as_str())).collect();
    assert_eq!(
        found,
        [
            (Language::Rust, ""),
            (Language::Python, ""),
            (Language::Go, "svc"),
            (Language::Typescript, "web"),
        ]
    );
    assert_eq!(more, 1, "web/app is the fifth project");
}

/// What the run must never do: each marker file appears only if repository code ran.
const MARKERS: [&str; 4] = ["build-ran", "python-ran", "npm-ran", "go-ran"];

#[test]
#[ignore = "downloads the pinned indexers"]
fn the_indexers_run_no_repository_code_and_read_only_pinned_blobs() {
    let out = tempfile::tempdir().expect("markers");
    let marker = |name: &str| out.path().join(name).display().to_string();
    let project = [
        ("Cargo.toml".to_owned(), "[package]\nname = \"p\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n".to_owned()),
        ("build.rs".to_owned(), format!("fn main() {{ std::fs::write({:?}, \"\").unwrap(); }}\n", marker(MARKERS[0]))),
        ("src/lib.rs".to_owned(), "pub mod a;\npub mod b;\n".to_owned()),
        ("src/a.rs".to_owned(), "pub fn a() -> usize { crate::b::B }\n".to_owned()),
        ("src/b.rs".to_owned(), "pub const B: usize = 1;\n".to_owned()),
        ("pyproject.toml".to_owned(), "[project]\nname = \"p\"\n".to_owned()),
        ("p/__init__.py".to_owned(), format!("open({:?}, 'w').close()\n", marker(MARKERS[1]))),
        ("p/a.py".to_owned(), "from p import b\n\ndef a():\n    return b.B\n".to_owned()),
        ("p/b.py".to_owned(), "B = 1\n".to_owned()),
        ("web/package.json".to_owned(), format!("{{\"name\": \"w\", \"version\": \"1.0.0\", \"scripts\": {{\"prepare\": \"touch {0}\", \"postinstall\": \"touch {0}\"}}}}\n", marker(MARKERS[2]))),
        ("web/a.ts".to_owned(), "export const a = 1;\n".to_owned()),
        ("svc/go.mod".to_owned(), "module example.com/svc\n\ngo 1.22\n".to_owned()),
        ("svc/x/x.go".to_owned(), format!("package x\n\nimport \"os\"\n\nfunc init() {{ os.WriteFile({:?}, nil, 0o600) }}\n\n//go:generate touch {}\nfunc X() int {{ return 1 }}\n", marker(MARKERS[3]), marker(MARKERS[3]))),
    ];
    let head: Vec<(String, Vec<u8>)> = project
        .iter()
        .map(|(p, t)| (p.clone(), t.as_bytes().to_vec()))
        .collect();
    let (repo, snapshot) = snapshot(&head);
    // A working tree the review never reads: the facts come from the commit.
    fs::write(
        repo.path().join("src/b.rs"),
        "pub const B: usize = 2;\npub fn dirty() {}\n",
    )
    .expect("dirty");
    let analyses = with_tools(|options| architecture::collect(&snapshot, options));
    let languages: Vec<Language> = analyses.iter().map(|a| a.language).collect();
    assert_eq!(
        languages,
        [
            Language::Rust,
            Language::Python,
            Language::Go,
            Language::Typescript
        ]
    );
    for analysis in &analyses {
        let built = analysis
            .head
            .as_ref()
            .unwrap_or_else(|(_, why)| panic!("{:?}: {why}", analysis.language));
        assert!(!built.model.files.is_empty(), "{:?}", analysis.language);
        assert!(analysis.base.is_none());
    }
    let rust = analyses[0].head.as_ref().expect("rust");
    assert!(
        rust.model.items.iter().all(|i| i.name != "dirty"),
        "only the committed blob is read"
    );
    for name in MARKERS {
        assert!(
            !out.path().join(name).exists(),
            "{name}: repository code ran"
        );
    }
}
