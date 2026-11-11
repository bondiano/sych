//! Code the change moves: a block removed on BASE that reappears on HEAD is a move, so a test
//! moved into its own file is not read as a removed test.

use serde_json::json;
use sych_core::analysis::{Context, Move, moves};
use sych_core::{DiffManifest, SourceSide};

use super::{Reader, path};

/// An inline test module and a helper; on HEAD both move to files of their own.
const LIB_BASE: &str = "pub fn add(a: u32, b: u32) -> u32 {\n    a + b\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn adds_two_numbers() {\n        assert_eq!(super::add(1, 2), 3);\n    }\n}\n";
const LIB_HEAD: &str =
    "pub fn add(a: u32, b: u32) -> u32 {\n    a + b\n}\n\n#[cfg(test)]\nmod tests;\n";
const TESTS: &str = "#[test]\nfn adds_two_numbers() {\n    assert_eq!(super::add(1, 2), 3);\n}\n";
const SUITE_BASE: &str = "fn helper(dir: &str) -> String {\n    let full = format!(\"{dir}/fixture\");\n    full.to_uppercase()\n}\n\n#[test]\nfn uses_helper() {\n    assert!(!helper(\"a\").is_empty());\n}\n";
const SUITE_HEAD: &str =
    "#[test]\nfn uses_helper() {\n    assert!(!common::helper(\"a\").is_empty());\n}\n";
const COMMON: &str = "pub fn helper(dir: &str) -> String {\n    let full = format!(\"{dir}/fixture\");\n    full.to_uppercase()\n}\n";
/// Short lines that repeat anywhere: never a move.
const NOISE_BASE: &str = "a\nb\nc\n}\n}\n";
const NOISE_HEAD: &str = "";
const NOISE_ELSEWHERE: &str = "a\nb\nc\n}\n}\n";

fn side(p: &str, text: &str) -> serde_json::Value {
    json!({"path": p, "mode": "100644", "oid": "sha1:dddddddddddddddddddddddddddddddddddddddd",
           "content": "TEXT", "line_count": text.lines().count()})
}

fn file(p: &str, texts: [Option<&str>; 2], hunks: &[[u32; 4]]) -> serde_json::Value {
    let status = match texts {
        [None, _] => "ADDED",
        [_, None] => "DELETED",
        _ => "MODIFIED",
    };
    let hunks: Vec<_> = hunks
        .iter()
        .map(
            |h| json!({"old_start": h[0], "old_lines": h[1], "new_start": h[2], "new_lines": h[3]}),
        )
        .collect();
    json!({"status": status, "similarity": null, "generated": false, "limitations": [],
        "old": texts[0].map(|t| side(p, t)), "new": texts[1].map(|t| side(p, t)), "hunks": hunks})
}

fn fixture() -> (Reader, DiffManifest) {
    let mut reader = Reader::new();
    for (s, p, text) in [
        (SourceSide::DiffBase, "src/lib.rs", LIB_BASE),
        (SourceSide::Head, "src/lib.rs", LIB_HEAD),
        (SourceSide::Head, "src/tests.rs", TESTS),
        (SourceSide::DiffBase, "tests/suite.rs", SUITE_BASE),
        (SourceSide::Head, "tests/suite.rs", SUITE_HEAD),
        (SourceSide::Head, "tests/common/mod.rs", COMMON),
        (SourceSide::DiffBase, "src/noise.rs", NOISE_BASE),
        (SourceSide::Head, "src/noise.rs", NOISE_HEAD),
        (SourceSide::Head, "src/elsewhere.rs", NOISE_ELSEWHERE),
    ] {
        reader.0.insert((s, path(p)), text);
    }
    let manifest = serde_json::from_value(json!({
        "schema_version": 1, "method": "fixture",
        "diff_base": "sha1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "head": "sha1:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "files": [
            file("src/lib.rs", [Some(LIB_BASE), Some(LIB_HEAD)], &[[6, 6, 6, 1]]),
            file("src/tests.rs", [None, Some(TESTS)], &[[0, 0, 1, 4]]),
            file("tests/suite.rs", [Some(SUITE_BASE), Some(SUITE_HEAD)], &[[1, 5, 0, 0], [8, 1, 3, 1]]),
            file("tests/common/mod.rs", [None, Some(COMMON)], &[[0, 0, 1, 4]]),
            file("src/noise.rs", [Some(NOISE_BASE), Some(NOISE_HEAD)], &[[1, 5, 0, 0]]),
            file("src/elsewhere.rs", [None, Some(NOISE_ELSEWHERE)], &[[0, 0, 1, 5]]),
        ],
        "omissions": []
    }))
    .expect("manifest");
    (reader, manifest)
}

fn found(m: &Move) -> (String, (u32, u32), String, (u32, u32)) {
    (m.from_path.to_string(), m.from, m.to_path.to_string(), m.to)
}

#[test]
fn moved_blocks_are_found_indentation_aside_and_short_lines_never_move() {
    let (reader, manifest) = fixture();
    let moved: Vec<_> = moves(&manifest, &reader).iter().map(found).collect();
    assert_eq!(
        moved,
        [
            // The test module's body, one level less indented in its own file.
            ("src/lib.rs".into(), (7, 10), "src/tests.rs".into(), (1, 4)),
            // The helper, now `pub`: its first line differs, the rest moved.
            (
                "tests/suite.rs".into(),
                (2, 4),
                "tests/common/mod.rs".into(),
                (2, 4)
            ),
        ]
    );
}

#[test]
fn a_moved_test_helper_is_listed_as_moved_not_as_removed() {
    let (reader, manifest) = fixture();
    let mut context = Context::default();
    context.add_test_removals(&manifest, &reader);
    let block = context.blocks.last().expect("removals block");
    let text = &block.content;
    assert!(
        text.contains("tests/suite.rs: 6 lines removed, 3 of them moved"),
        "{text}"
    );
    assert!(
        text.contains("tests/suite.rs:2-4 moved to tests/common/mod.rs:2-4 (same text)"),
        "{text}"
    );
    // The changed first line and the changed assertion are shown; the moved lines are not.
    assert!(
        text.contains("- fn helper(dir: &str) -> String {"),
        "{text}"
    );
    assert!(
        text.contains("-     assert!(!helper(\"a\").is_empty());"),
        "{text}"
    );
    assert!(
        !text.contains("- full.to_uppercase()") && !text.contains("-     full.to_uppercase()"),
        "{text}"
    );
}

#[test]
fn a_context_lists_the_moves_of_the_files_it_shows() {
    let (reader, manifest) = fixture();
    let mut none = Context::default();
    none.add_moves(&manifest, &reader);
    assert!(none.blocks.is_empty(), "no shown file, no block");

    let mut shown = Context::default();
    let span = reader.at(SourceSide::Head, "src/tests.rs", 1, 4);
    shown.add_span(&span, "the moved tests", &reader);
    shown.add_moves(&manifest, &reader);
    let block = shown.blocks.last().expect("moves block");
    assert!(
        block.label.starts_with("blocks this change moves"),
        "{}",
        block.label
    );
    assert_eq!(block.content, "src/lib.rs:7-10 moved to src/tests.rs:1-4\n");
}

#[test]
fn the_longest_block_wins_even_when_a_shorter_one_comes_first() {
    const SHORT: &str =
        "let total = items.len();\nlet first = items[0];\nlet last = items[total - 1];\n";
    const LONG: &str = "let total = items.len();\nlet first = items[0];\nlet last = items[total - 1];\nlet spread = last - first;\nreport(spread);\n";
    let mut reader = Reader::new();
    for (s, p, text) in [
        (SourceSide::DiffBase, "src/a.rs", SHORT),
        (SourceSide::DiffBase, "src/b.rs", LONG),
        (SourceSide::Head, "src/c.rs", LONG),
    ] {
        reader.0.insert((s, path(p)), text);
    }
    let manifest: DiffManifest = serde_json::from_value(json!({
        "schema_version": 1, "method": "fixture",
        "diff_base": "sha1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "head": "sha1:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "files": [
            file("src/a.rs", [Some(SHORT), None], &[[1, 3, 0, 0]]),
            file("src/b.rs", [Some(LONG), None], &[[1, 5, 0, 0]]),
            file("src/c.rs", [None, Some(LONG)], &[[0, 0, 1, 5]]),
        ],
        "omissions": []
    }))
    .expect("manifest");
    let moved: Vec<_> = moves(&manifest, &reader).iter().map(found).collect();
    assert_eq!(
        moved,
        [("src/b.rs".into(), (1, 5), "src/c.rs".into(), (1, 5))]
    );
}
