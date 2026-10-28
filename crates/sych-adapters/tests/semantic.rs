//! Semantic backend over a scripted language server (`fixtures/fake_lsp.py`) on real Git
//! snapshots: separate base/head workspaces, rename and Unicode anchors, deleted constructs,
//! unsupported capabilities and empty answers.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use serde_json::json;
use sych_adapters::git::{LocalSnapshot, SnapshotRequest};
use sych_adapters::semantic::{self, Options};
use sych_adapters::tools::PinnedTool;
use sych_core::config::Tooling;
use sych_core::{BackendStatus, Completeness, QueryStatus, RepoPath, SemanticFacts, SourceSide};
use tempfile::TempDir;

mod common;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .env_clear()
        .env("PATH", env::var_os("PATH").unwrap_or_default())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write(dir: &Path, path: &str, content: &str) {
    let path = dir.join(path);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, content).expect("write");
}

fn commit(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
}

const PRICE_BASE: &str = "// цена — base\nexport function price(x: number) {\n  const k = 2;\n  \
    const y = x * k;\n  return y;\n}\n\nexport function legacy() {\n  return 0;\n}\n";
const PRICE_HEAD: &str = "// цена — base\nexport function price(x: number): Money {\n  const k = 3;\n  \
    const y = round(x * k);\n  return y;\n}\n";
const USE: &str = "import { price } from \"./цена\";\n\nexport function total() {\n  \
    return price(1) + price(2);\n}\n\nexport function round(v: number) {\n  return v;\n}\n\nexport interface Money {\n  cents: number;\n}\n";

/// base: `src/цена.ts` with `price` and `legacy`; head renames it to `src/price.ts`,
/// changes `price` and deletes `legacy`.
fn scenario() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let d = dir.path();
    git(d, &["init", "-q", "-b", "main"]);
    write(d, "package.json", r#"{"name": "shop"}"#);
    write(
        d,
        "pnpm-lock.yaml",
        "lockfileVersion: '9.0'\nimporters:\n  .:\n    devDependencies:\n      typescript:\n        \
         specifier: 7.0.2\n        version: 7.0.2\n",
    );
    write(
        d,
        "tsconfig.json",
        "{\n  // app\n  \"compilerOptions\": {\"paths\": {\"@/*\": [\"./src/*\"]}},\n}\n",
    );
    write(d, "src/цена.ts", PRICE_BASE);
    write(d, "src/use.ts", USE);
    commit(d, "base");
    git(d, &["mv", "src/цена.ts", "src/price.ts"]);
    write(d, "src/price.ts", PRICE_HEAD);
    write(d, "src/use.ts", &USE.replace("./цена", "./price"));
    commit(d, "head");
    dir
}

fn server(capabilities: &serde_json::Value) -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("fake_lsp.py");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_lsp.py");
    common::write_executable(&script, &fs::read_to_string(fixture).expect("server"));
    fs::write(dir.path().join("caps.json"), capabilities.to_string()).expect("caps");
    dir
}

fn collect(repo: &Path, server: &Path) -> (LocalSnapshot, Vec<SemanticFacts>) {
    let snapshot = LocalSnapshot::capture(&SnapshotRequest {
        repo,
        base: "HEAD~1",
        head: "HEAD",
        diff_base: None,
        max_blob_bytes: 1 << 20,
    })
    .expect("snapshot");
    let script: PathBuf = server.join("fake_lsp.py");
    let resolve = move |_: &'static PinnedTool, _: &str| Ok(script.clone());
    let state = tempfile::tempdir().expect("state");
    let facts = semantic::collect(
        &snapshot,
        &Options {
            resolve: &resolve,
            cache: None,
            tooling: &Tooling::default(),
            state_dir: state.path(),
        },
    )
    .expect("facts");
    (snapshot, facts)
}

fn side(facts: &[SemanticFacts], side: SourceSide) -> &SemanticFacts {
    facts.iter().find(|f| f.side == side).expect("side present")
}

fn path(p: &str) -> RepoPath {
    p.parse().expect("repo path")
}

#[test]
fn base_and_head_are_separate_workspaces_with_exact_anchors() {
    let repo = scenario();
    let caps = json!({"referencesProvider": true, "implementationProvider": true,
                      "callHierarchyProvider": true, "definitionProvider": true,
                      "diagnosticProvider": {}});
    let server = server(&caps);
    let (snapshot, facts) = collect(repo.path(), server.path());
    let (base, head) = (
        side(&facts, SourceSide::DiffBase),
        side(&facts, SourceSide::Head),
    );
    for f in [base, head] {
        assert_eq!(f.status, BackendStatus::Ready, "{:?}", f.limitations);
        assert_eq!(f.producer.as_ref().expect("producer").name, "fake-lsp");
        assert_eq!(f.position_encoding.as_deref(), Some("utf-16"));
        assert_eq!(f.build_configuration["typescript"], "7.0.2");
        assert_eq!(f.build_configuration["package:."], "shop");
        assert_eq!(f.build_configuration["tsconfig:tsconfig.json"], "paths=@/*");
    }

    let symbol = |f: &SemanticFacts, name: &str| f.symbols.iter().find(|s| s.name == name).cloned();
    // Each side's definition points at its own path and blob, with the hash of those lines.
    let base_price = symbol(base, "price").expect("base price");
    let head_price = symbol(head, "price").expect("head price");
    assert_eq!(
        base_price.definition,
        snapshot
            .code_ref(SourceSide::DiffBase, &path("src/цена.ts"), 2, 6)
            .expect("base ref")
    );
    assert_eq!(
        head_price.definition,
        snapshot
            .code_ref(SourceSide::Head, &path("src/price.ts"), 2, 6)
            .expect("head ref")
    );
    assert_ne!(
        base_price.definition.blob_oid,
        head_price.definition.blob_oid
    );

    // A construct deleted by the change exists only on the base side.
    assert!(symbol(base, "legacy").is_some());
    assert!(symbol(head, "legacy").is_none());

    // Call sites and the construct that contains them, on the same side.
    let use_head = path("src/use.ts");
    assert_eq!(head_price.callers.status, QueryStatus::Answered);
    assert_eq!(head_price.callers.completeness, Completeness::Partial);
    assert_eq!(
        head_price.callers.refs,
        [snapshot
            .code_ref(SourceSide::Head, &use_head, 4, 4)
            .expect("call site")]
    );
    assert_eq!(
        head_price.calling_constructs,
        [snapshot
            .code_ref(SourceSide::Head, &use_head, 3, 5)
            .expect("total")]
    );

    // The head now calls `round`: its whole construct is a callee; the base calls nothing.
    assert_eq!(
        head_price.callees.refs,
        [snapshot
            .code_ref(SourceSide::Head, &use_head, 7, 9)
            .expect("round")]
    );
    // The head names `Money`; its whole interface is a used type definition.
    assert_eq!(
        head_price.type_definitions.refs,
        [snapshot
            .code_ref(SourceSide::Head, &use_head, 11, 13)
            .expect("Money")]
    );
    assert!(base_price.type_definitions.refs.is_empty());
    assert_eq!(base_price.callees.status, QueryStatus::Answered);
    assert!(base_price.callees.refs.is_empty());

    // An answered empty query is partial, never proof that nothing references `legacy`.
    let legacy = symbol(base, "legacy").expect("legacy");
    assert_eq!(legacy.references.status, QueryStatus::Answered);
    assert!(legacy.references.refs.is_empty());
    assert_eq!(legacy.references.completeness, Completeness::Partial);
    assert_eq!(legacy.implementations.status, QueryStatus::Answered);
}

#[test]
fn missing_capabilities_are_unsupported_not_empty_success() {
    let repo = scenario();
    let server = server(&json!({"referencesProvider": true}));
    let (_, facts) = collect(repo.path(), server.path());
    let head = side(&facts, SourceSide::Head);
    let price = head
        .symbols
        .iter()
        .find(|s| s.name == "price")
        .expect("price");
    assert_eq!(price.callers.status, QueryStatus::Unsupported);
    assert_eq!(price.callers.completeness, Completeness::Unknown);
    assert!(price.calling_constructs.is_empty());
    assert_eq!(price.callees.status, QueryStatus::Unsupported);
    assert_eq!(price.type_definitions.status, QueryStatus::Unsupported);
    assert_eq!(price.implementations.status, QueryStatus::Unsupported);
    assert!(
        head.limitations
            .iter()
            .any(|l| l.contains("pull diagnostics")),
        "{:?}",
        head.limitations
    );
}

#[test]
fn each_side_resolves_its_own_toolchain() {
    let repo = scenario();
    write(repo.path(), "pnpm-lock.yaml", "lockfileVersion: '9.0'\n");
    write(
        repo.path(),
        "src/use.ts",
        "export function total() {\n  return 1;\n}\n",
    );
    commit(repo.path(), "drop typescript");
    let server = server(&json!({}));
    let (_, facts) = collect(repo.path(), server.path());
    // The base still pins TypeScript; the head no longer does and is not given base's version.
    assert_eq!(
        side(&facts, SourceSide::DiffBase).status,
        BackendStatus::Ready
    );
    let head = side(&facts, SourceSide::Head);
    assert_eq!(head.status, BackendStatus::Unsupported);
    assert!(head.symbols.is_empty());
    assert!(
        head.limitations
            .iter()
            .any(|l| l.contains("does not pin typescript")),
        "{:?}",
        head.limitations
    );
}

#[test]
fn reads_and_writes_of_a_changed_variable_come_from_highlight_kinds() {
    let repo = scenario();
    write(repo.path(), "src/count.ts", "export let count = 0;\n");
    write(
        repo.path(),
        "src/bump.ts",
        "import { count } from \"./count\";\n\nexport function bump() {\n  count = count + 1;\n}\n",
    );
    commit(repo.path(), "count");
    write(repo.path(), "src/count.ts", "export let count = 1;\n");
    commit(repo.path(), "start at one");
    let with_highlight = server(&json!({"referencesProvider": true,
                                        "documentHighlightProvider": true}));
    let (snapshot, facts) = collect(repo.path(), with_highlight.path());
    let head = side(&facts, SourceSide::Head);
    let count = head
        .symbols
        .iter()
        .find(|s| s.name == "count")
        .expect("count");
    let at = |line| {
        snapshot
            .code_ref(SourceSide::Head, &path("src/bump.ts"), line, line)
            .expect("ref")
    };
    assert_eq!(count.writers.status, QueryStatus::Answered);
    assert_eq!(count.writers.refs, [at(4)]);
    assert_eq!(count.readers.refs, [at(1), at(4)]);
    assert!(count.readers.unresolved.is_empty(), "{:?}", count.readers);

    let bump = head.symbols.iter().find(|s| s.name == "bump");
    assert!(bump.is_none(), "bump did not change");
    let without = server(&json!({"referencesProvider": true}));
    let (_, facts) = collect(repo.path(), without.path());
    let count = side(&facts, SourceSide::Head)
        .symbols
        .iter()
        .find(|s| s.name == "count")
        .cloned()
        .expect("count");
    assert_eq!(count.writers.status, QueryStatus::Unsupported);
    assert_eq!(count.readers.status, QueryStatus::Unsupported);
}
