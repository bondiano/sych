//! Effect ledger on real Git snapshots: paired unsafe/safe
//! scenarios of Go transactions and outbox delivery, a registry whose implementation
//! changed, React cleanup and stale results, cache invalidation and Python lifetime.

use std::path::Path;
use std::process::Command;
use std::{env, fs};

use sych_adapters::git::{LocalSnapshot, SnapshotRequest};
use sych_adapters::syntax;
use sych_core::config::Effect;
use sych_core::effects::{self, BindingStatus, EffectLedger, EffectMatch, LedgerEntry, Resolution};

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
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}");
}

fn commit(dir: &Path, files: &[(&str, &str)], message: &str) {
    for (path, text) in files {
        let path = dir.join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, text).expect("write");
    }
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
}

const REGISTRY: &str = "- id: order-outbox\n  match: {language: go, symbol: \"example.internal/orders.Store.AppendEvent\"}\n  effect_kind: event\n  guarantees: {durable_with_transaction: true}\n  implementation_refs: [\"orders/outbox.go\"]\n  contract_refs: [\".sych/contracts/order-created.md\"]\n";

/// The ledger of `base` → `head` (files written over the previous commit).
fn ledger(base: &[(&str, &str)], head: &[(&str, &str)]) -> EffectLedger {
    let dir = tempfile::tempdir().expect("tempdir");
    git(dir.path(), &["init", "-q", "-b", "main"]);
    commit(dir.path(), base, "base");
    commit(dir.path(), head, "head");
    let snapshot = LocalSnapshot::capture(&SnapshotRequest {
        repo: dir.path(),
        base: "HEAD~1",
        head: "HEAD",
        diff_base: None,
        max_blob_bytes: 1 << 20,
    })
    .expect("snapshot");
    let registry: Vec<Effect> = serde_yaml_ng::from_str(REGISTRY).expect("registry");
    let facts = syntax::collect(&snapshot, &registry);
    effects::ledger(
        &snapshot.snapshot.id,
        &snapshot.manifest,
        &facts,
        &[],
        effects::bindings(&registry, &snapshot.manifest),
    )
}

fn entry<'a>(ledger: &'a EffectLedger, target: &str) -> &'a LedgerEntry {
    ledger
        .entries
        .iter()
        .find(|e| e.target == target)
        .unwrap_or_else(|| panic!("no {target} in {:#?}", ledger.entries))
}

const STORE: &str = "package orders\n\nimport (\n\t\"context\"\n\t\"database/sql\"\n)\n\n\
    type Store struct {\n\tdb *sql.DB\n}\n";
const OUTBOX: &str = "package orders\n\nimport (\n\t\"context\"\n\t\"database/sql\"\n)\n\n\
    func (s *Store) AppendEvent(ctx context.Context, tx *sql.Tx, e string) error {\n\
    \t_, err := tx.ExecContext(ctx, \"INSERT INTO outbox VALUES ($1)\", e)\n\treturn err\n}\n";
const CREATE: &str = "package orders\n\nimport (\n\t\"context\"\n\t\"database/sql\"\n)\n\n\
    func (s *Store) Create(ctx context.Context, id string) error {\n\
    \ttx, err := s.db.BeginTx(ctx, nil)\n\tif err != nil {\n\t\treturn err\n\t}\n\
    \tdefer tx.Rollback()\n\
    \tif _, err := tx.ExecContext(ctx, \"INSERT INTO orders VALUES ($1)\", id); err != nil {\n\t\treturn err\n\t}\n\
    \tif err := s.AppendEvent(ctx, tx, id); err != nil {\n\t\treturn err\n\t}\n\
    \treturn tx.Commit()\n}\n";

fn go_base() -> Vec<(&'static str, &'static str)> {
    vec![
        ("orders/store.go", STORE),
        ("orders/outbox.go", OUTBOX),
        ("orders/create.go", CREATE),
    ]
}

#[test]
fn tx_to_db_loses_atomicity_while_a_tx_helper_is_a_safe_move() {
    let unsafe_head = CREATE.replace(
        "tx.ExecContext(ctx, \"INSERT INTO orders",
        "s.db.ExecContext(ctx, \"INSERT INTO orders",
    );
    let l = ledger(&go_base(), &[("orders/create.go", &unsafe_head)]);
    let exec = entry(&l, "ExecContext");
    assert_eq!(exec.matched, EffectMatch::Changed);
    assert_eq!(exec.resolution, Resolution::Resolved);
    assert!(
        exec.notes
            .iter()
            .any(|n| n.contains("transaction (*sql.Tx) → none: *sql.DB")),
        "{:?}",
        exec.notes
    );

    let safe_head = CREATE.replace(
        "\tif _, err := tx.ExecContext(ctx, \"INSERT INTO orders VALUES ($1)\", id); err != nil {\n\t\treturn err\n\t}\n",
        "\tif err := insert(ctx, tx, id); err != nil {\n\t\treturn err\n\t}\n",
    ) + "\nfunc insert(ctx context.Context, tx *sql.Tx, id string) error {\n\
        \t_, err := tx.ExecContext(ctx, \"INSERT INTO orders VALUES ($1)\", id)\n\treturn err\n}\n";
    let l = ledger(&go_base(), &[("orders/create.go", &safe_head)]);
    let exec = entry(&l, "ExecContext");
    assert_eq!(exec.matched, EffectMatch::Moved, "{:?}", exec.notes);
    assert_eq!(exec.symbol.as_deref(), Some("insert"));
    assert_eq!(exec.head[0].transaction_scope, "transaction (*sql.Tx)");
    // No semantic facts here, so the call edge stays unproven.
    assert_eq!(exec.resolution, Resolution::Partial);
}

#[test]
fn outbox_moved_to_a_goroutine_is_changed_but_not_declared_lost() {
    let head = CREATE
        .replace(
            "\tif err := s.AppendEvent(ctx, tx, id); err != nil {\n\t\treturn err\n\t}\n\treturn tx.Commit()\n",
            "\tif err := tx.Commit(); err != nil {\n\t\treturn err\n\t}\n\
             \tgo s.AppendEvent(context.Background(), nil, id)\n\treturn nil\n",
        );
    let l = ledger(&go_base(), &[("orders/create.go", &head)]);
    let event = entry(&l, "example.internal/orders.Store.AppendEvent");
    assert_eq!(event.matched, EffectMatch::Changed);
    assert_eq!(event.registry_id.as_deref(), Some("order-outbox"));
    assert_eq!(event.durable_with_transaction, Some(true));
    assert!(event.head[0].lifetime_scope.starts_with("detached"));
    let notes = event.notes.join("\n");
    assert!(notes.contains("recovery path"), "{notes}");
    assert!(!notes.contains("lost"), "loss is not asserted: {notes}");
    assert!(!event.contract_refs.is_empty());

    // The safe pair keeps the call in the transaction.
    let l = ledger(
        &go_base(),
        &[(
            "orders/create.go",
            &CREATE.replace("return tx.Commit()", "return tx.Commit() // done"),
        )],
    );
    assert!(
        l.entries
            .iter()
            .filter(|e| e.registry_id.is_some())
            .all(|e| e.matched == EffectMatch::Preserved)
    );
}

#[test]
fn changed_wrapper_implementation_is_not_trusted() {
    let changed = OUTBOX.replace("tx.ExecContext", "s.db.ExecContext");
    let l = ledger(
        &go_base(),
        &[
            ("orders/outbox.go", &changed),
            (
                "orders/create.go",
                &CREATE.replace("return tx.Commit()", "return tx.Commit() // v2"),
            ),
        ],
    );
    let binding = &l.registry[0];
    assert_eq!(binding.status, BindingStatus::ImplementationChanged);
    assert!(binding.implementation[0].base_oid != binding.implementation[0].head_oid);
    let event = entry(&l, "example.internal/orders.Store.AppendEvent");
    assert_eq!(event.durable_with_transaction, None);
    assert!(event.notes.iter().any(|n| n.contains("not carried")));
    assert!(
        l.dependencies
            .iter()
            .any(|d| d.path.as_str() == "orders/outbox.go")
    );
}

#[test]
fn react_cleanup_stale_results_and_invalidation_are_separate_effects() {
    let base = "export function Orders({ id }) {\n  useEffect(() => {\n    const off = bus.subscribe(id, load);\n    return () => off();\n  }, [id]);\n  useEffect(() => {\n    let ignore = false;\n    fetch(`/o/${id}`).then(r => r.json()).then(o => { if (!ignore) setOrder(o); });\n    return () => { ignore = true; };\n  }, [id]);\n  const save = useMutation({ mutationFn: post, onSuccess: () => client.invalidateQueries(['orders']) });\n}\n";
    let leaky = "export function Orders({ id }) {\n  useEffect(() => {\n    bus.subscribe(id, load);\n  }, [id]);\n  useEffect(() => {\n    fetch(`/o/${id}`).then(r => r.json()).then(o => setOrder(o));\n  }, [id]);\n  const save = useMutation({ mutationFn: post });\n}\n";
    let l = ledger(&[("web/orders.tsx", base)], &[("web/orders.tsx", leaky)]);
    let cleanup = entry(&l, "useEffect");
    assert_eq!(cleanup.matched, EffectMatch::Changed);
    assert!(cleanup.notes.iter().any(|n| n.starts_with("lifetime")));
    let stale = entry(&l, "state set from an async result");
    assert_eq!(stale.matched, EffectMatch::Changed);
    assert_eq!(stale.head[0].lifetime_scope, "stale results are applied");
    let invalidation = l
        .entries
        .iter()
        .find(|e| e.target.contains("invalidateQueries"))
        .expect("invalidation");
    assert_eq!(invalidation.matched, EffectMatch::Removed);

    // Invalidation moved into the mutation hook is a move, not a loss.
    let hook = "export function useSaveOrder() {\n  return useMutation({ mutationFn: post, onSuccess: () => client.invalidateQueries(['orders']) });\n}\n";
    let moved = base.replace(
        "useMutation({ mutationFn: post, onSuccess: () => client.invalidateQueries(['orders']) })",
        "useSaveOrder()",
    );
    let l = ledger(
        &[("web/orders.tsx", base)],
        &[("web/orders.tsx", &moved), ("web/hooks.ts", hook)],
    );
    let invalidation = l
        .entries
        .iter()
        .find(|e| e.target.contains("invalidateQueries"))
        .expect("invalidation");
    assert_eq!(invalidation.matched, EffectMatch::Moved);
    assert_eq!(invalidation.symbol.as_deref(), Some("useSaveOrder"));
}

#[test]
fn python_write_moved_out_of_with_changes_its_lifetime() {
    let base = "import json\n\n\ndef export(path, data):\n    with open(path, \"w\") as f:\n        f.write(json.dumps(data))\n";
    let unsafe_head = "import json\n\n\ndef export(path, data):\n    f = open(path, \"w\")\n    f.write(json.dumps(data))\n";
    let l = ledger(
        &[("app/export.py", base)],
        &[("app/export.py", unsafe_head)],
    );
    let write = entry(&l, "write");
    assert_eq!(write.matched, EffectMatch::Changed);
    assert!(write.notes.iter().any(|n| n.starts_with("lifetime")));
    assert_eq!(write.resolution, Resolution::Partial, "Python is untyped");

    let safe_head = base.replace("json.dumps(data)", "json.dumps(data, indent=2)");
    let l = ledger(&[("app/export.py", base)], &[("app/export.py", &safe_head)]);
    assert_eq!(entry(&l, "write").matched, EffectMatch::Preserved);
}

#[test]
fn dynamic_dispatch_leaves_the_effect_unresolved() {
    let base = "def save(repo, order):\n    repo.session.add(order)\n";
    let head = "def save(repo, order):\n    handler = getattr(repo, order.kind)\n    repo.session.add(order)\n";
    let l = ledger(&[("app/save.py", base)], &[("app/save.py", head)]);
    let add = entry(&l, "add");
    assert_eq!(add.resolution, Resolution::DynamicUnknown);
    assert_eq!(add.matched, EffectMatch::Unresolved);
}
