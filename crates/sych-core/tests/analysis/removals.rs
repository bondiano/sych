//! What `TEST-INTEGRITY` sees of the tests a change touches: their removed lines, verbatim,
//! each hunk with the lines replacing it.

use serde_json::json;
use sych_core::analysis::Context;
use sych_core::{DiffManifest, SourceSide};

use super::{Reader, path};

const TEST_BASE: &str =
    "it('totals', () => {\n  expect(total([])).toBe(0);\n  expect(total()).toBe(0);\n});\n";

fn file(old: Option<&str>, new: &str, hunk: [u32; 4]) -> serde_json::Value {
    let side = |p: &str| {
        json!({"path": p, "mode": "100644",
        "oid": "sha1:dddddddddddddddddddddddddddddddddddddddd", "content": "TEXT", "line_count": 4})
    };
    json!({"status": if old.is_some() { "MODIFIED" } else { "ADDED" }, "similarity": null,
        "generated": false, "limitations": [], "old": old.map(side), "new": side(new),
        "hunks": [{"old_start": hunk[0], "old_lines": hunk[1], "new_start": hunk[2], "new_lines": hunk[3]}]})
}

#[test]
fn a_removed_assertion_is_shown_verbatim_and_untouched_tests_say_so() {
    let mut reader = Reader::new();
    reader
        .0
        .insert((SourceSide::DiffBase, path("src/order.test.ts")), TEST_BASE);
    let manifest: DiffManifest = serde_json::from_value(json!({
        "schema_version": 1, "method": "fixture",
        "diff_base": "sha1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "head": "sha1:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "files": [
            file(Some("src/order.test.ts"), "src/order.test.ts", [3, 1, 2, 0]),
            file(None, "tests/new.test.ts", [0, 0, 1, 3]),
            file(Some("src/order.ts"), "src/order.ts", [2, 3, 2, 1]),
        ],
        "omissions": []
    }))
    .expect("manifest");
    let mut context = Context::default();
    context.add_test_removals(&manifest, &reader);
    let block = context.blocks.last().expect("removals block");
    assert_eq!(
        block.content,
        "src/order.test.ts: 1 lines removed\n- \x20 expect(total()).toBe(0);\n\
         tests/new.test.ts: 0 lines removed\n"
    );
}

#[test]
fn a_replaced_assertion_is_shown_with_its_replacement() {
    let mut reader = Reader::new();
    reader.0.insert(
        (SourceSide::DiffBase, path("tests/import.rs")),
        "fn tables() {\n    assert_eq!(TABLES.len(), 24);\n}\n",
    );
    reader.0.insert(
        (SourceSide::Head, path("tests/import.rs")),
        "fn tables() {\n    assert_eq!(TABLES.len(), 25);\n}\n",
    );
    let manifest: DiffManifest = serde_json::from_value(json!({
        "schema_version": 1, "method": "fixture",
        "diff_base": "sha1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "head": "sha1:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "files": [file(Some("tests/import.rs"), "tests/import.rs", [2, 1, 2, 1])],
        "omissions": []
    }))
    .expect("manifest");
    let mut context = Context::default();
    context.add_test_removals(&manifest, &reader);
    let block = context.blocks.last().expect("removals block");
    assert_eq!(
        block.content,
        "tests/import.rs: 1 lines removed\n\
         - \x20   assert_eq!(TABLES.len(), 24);\n\
         + \x20   assert_eq!(TABLES.len(), 25);\n"
    );
}
