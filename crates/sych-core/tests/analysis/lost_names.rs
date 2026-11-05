//! What a verifier sees of a hypothesis anchored on removed lines: the names they lose and
//! where else the change removes or adds them, so a guard removed with its field is one change.

use serde_json::json;
use sych_core::analysis::Context;
use sych_core::{DiffManifest, SourceSide};

use super::{Reader, path};

const SCHEMA_BASE: &str = "fn schema(item: &Item) {\n    if item.form.is_some() {\n        unsupported(\"a form\");\n        return;\n    }\n    emit(item);\n}\n";
const SCHEMA_HEAD: &str = "fn schema(item: &Item) {\n    if item.ty.is_empty() {\n        return;\n    }\n    emit(item);\n}\n";
const ITEM_BASE: &str =
    "pub struct Item {\n    pub ty: String,\n    pub form: Option<Vec<String>>,\n}\n";
const ITEM_HEAD: &str = "pub struct Item {\n    pub ty: String,\n}\n";
const PLUGIN: &str = "pub fn derive_form(fields: &[String]) -> String {\n    format!(\"form of {}\", fields.len())\n}\n";
const OTHER_BASE: &str =
    "fn other(item: u32) -> u32 {\n    if item == 0 {\n        return 1;\n    }\n    item\n}\n";
const OTHER_HEAD: &str = "fn other(item: u32) -> u32 {\n    item\n}\n";
const LEGACY: &str = "fn form() {}\n";

fn side(p: &str, text: &str) -> serde_json::Value {
    json!({"path": p, "mode": "100644", "oid": "sha1:dddddddddddddddddddddddddddddddddddddddd",
           "content": "TEXT", "line_count": text.lines().count()})
}

fn file(status: &str, p: &str, texts: [Option<&str>; 2], hunk: [u32; 4]) -> serde_json::Value {
    json!({"status": status, "similarity": null, "generated": false, "limitations": [],
        "old": texts[0].map(|t| side(p, t)), "new": texts[1].map(|t| side(p, t)),
        "hunks": [{"old_start": hunk[0], "old_lines": hunk[1], "new_start": hunk[2], "new_lines": hunk[3]}]})
}

/// A guard and the field it checks removed together, the feature added in a plugin, an
/// unrelated removal of names the guarded file keeps, and a deleted file.
fn fixture() -> (Reader, DiffManifest) {
    let mut reader = Reader::new();
    for (s, p, text) in [
        (SourceSide::DiffBase, "src/codegen/schema.rs", SCHEMA_BASE),
        (SourceSide::Head, "src/codegen/schema.rs", SCHEMA_HEAD),
        (SourceSide::DiffBase, "src/types/item.rs", ITEM_BASE),
        (SourceSide::Head, "src/types/item.rs", ITEM_HEAD),
        (SourceSide::Head, "src/plugin/form.rs", PLUGIN),
        (SourceSide::DiffBase, "src/other.rs", OTHER_BASE),
        (SourceSide::Head, "src/other.rs", OTHER_HEAD),
        (SourceSide::DiffBase, "src/legacy.rs", LEGACY),
    ] {
        reader.0.insert((s, path(p)), text);
    }
    let manifest = serde_json::from_value(json!({
        "schema_version": 1, "method": "fixture",
        "diff_base": "sha1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "head": "sha1:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "files": [
            file("MODIFIED", "src/codegen/schema.rs", [Some(SCHEMA_BASE), Some(SCHEMA_HEAD)], [2, 2, 2, 1]),
            file("DELETED", "src/legacy.rs", [Some(LEGACY), None], [1, 1, 0, 0]),
            file("MODIFIED", "src/other.rs", [Some(OTHER_BASE), Some(OTHER_HEAD)], [2, 3, 1, 0]),
            file("ADDED", "src/plugin/form.rs", [None, Some(PLUGIN)], [0, 0, 1, 3]),
            file("MODIFIED", "src/types/item.rs", [Some(ITEM_BASE), Some(ITEM_HEAD)], [3, 1, 2, 0]),
        ],
        "omissions": []
    }))
    .expect("manifest");
    (reader, manifest)
}

#[test]
fn a_removed_guard_shows_the_field_the_change_also_removes() {
    let (reader, manifest) = fixture();
    let guard = reader.at(SourceSide::DiffBase, "src/codegen/schema.rs", 2, 5);
    let mut context = Context::default();
    context.add_lost_names(&[&guard], &manifest, &reader);
    let block = context.blocks.last().expect("lost names block");
    // `item` and `return` stay in the guarded file, so src/other.rs is not followed.
    assert_eq!(
        block.content,
        "`form`\n\
         \x20 src/types/item.rs:3 -     pub form: Option<Vec<String>>,\n\
         \x20 src/legacy.rs: deleted, 1 removed lines name it\n\
         \x20 added: src/plugin/form.rs (1 lines)\n\
         `is_some`: no other file of the change removes or adds it\n\
         `unsupported`: no other file of the change removes or adds it\n"
    );
}

#[test]
fn only_removed_base_lines_of_a_kept_file_are_followed() {
    let (reader, manifest) = fixture();
    let head = reader.at(SourceSide::Head, "src/codegen/schema.rs", 2, 3);
    let unchanged = reader.at(SourceSide::DiffBase, "src/codegen/schema.rs", 6, 7);
    let deleted = reader.at(SourceSide::DiffBase, "src/legacy.rs", 1, 1);
    let mut context = Context::default();
    context.add_lost_names(&[&head, &unchanged, &deleted], &manifest, &reader);
    assert!(context.blocks.is_empty());
}

#[test]
fn a_guarded_file_no_longer_text_is_not_followed() {
    let (reader, mut manifest) = fixture();
    if let Some(new) = manifest.files[0].new.as_mut() {
        new.line_count = None;
    }

    let guard = reader.at(SourceSide::DiffBase, "src/codegen/schema.rs", 2, 5);
    let mut context = Context::default();
    context.add_lost_names(&[&guard], &manifest, &reader);
    assert!(context.blocks.is_empty());
}
