//! Code the change moves: a block removed on BASE that reappears on HEAD is a move, so a test
//! moved into its own file is not read as a removed test.

use std::fmt::Write;

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
    let moved: Vec<_> = moves(&manifest, &reader).blocks.iter().map(found).collect();
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
    let moved: Vec<_> = moves(&manifest, &reader).blocks.iter().map(found).collect();
    assert_eq!(
        moved,
        [("src/b.rs".into(), (1, 5), "src/c.rs".into(), (1, 5))]
    );
}

/// One-file manifest helper: `files` as `(path, BASE text, HEAD text)`, each side wholly changed.
fn whole(
    reader: &mut Reader,
    files: &[(&str, Option<&'static str>, Option<&'static str>)],
) -> DiffManifest {
    let mut changes = Vec::new();
    for &(p, base, head) in files {
        let count =
            |t: Option<&str>| t.map_or(0, |t| u32::try_from(t.lines().count()).expect("lines"));
        if let Some(t) = base {
            reader.0.insert((SourceSide::DiffBase, path(p)), t);
        }

        if let Some(t) = head {
            reader.0.insert((SourceSide::Head, path(p)), t);
        }

        let hunk = [
            u32::from(base.is_some()),
            count(base),
            u32::from(head.is_some()),
            count(head),
        ];
        changes.push(file(p, [base, head], &[hunk]));
    }
    serde_json::from_value(json!({
        "schema_version": 1, "method": "fixture",
        "diff_base": "sha1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "head": "sha1:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "files": changes, "omissions": []
    }))
    .expect("manifest")
}

#[test]
fn a_block_partly_taken_by_a_longer_one_keeps_its_free_part() {
    const HEAD: &str = "let alpha_value = one();\nlet beta_value = two();\nlet gamma_value = three();\nlet delta_value = four();\nlet epsilon_value = five();\nlet zeta_value = six();\nlet eta_value = seven();\nlet theta_value = eight();\nlet iota_value = nine();\nlet kappa_value = ten();\n";
    // HEAD lines 5-10 and 1-6: equal length, so `a.rs` (first in the manifest) wins lines 5-6.
    const A: &str = "let epsilon_value = five();\nlet zeta_value = six();\nlet eta_value = seven();\nlet theta_value = eight();\nlet iota_value = nine();\nlet kappa_value = ten();\n";
    const B: &str = "let alpha_value = one();\nlet beta_value = two();\nlet gamma_value = three();\nlet delta_value = four();\nlet epsilon_value = five();\nlet zeta_value = six();\n";
    let mut reader = Reader::new();
    let manifest = whole(
        &mut reader,
        &[
            ("src/a.rs", Some(A), None),
            ("src/b.rs", Some(B), None),
            ("src/c.rs", None, Some(HEAD)),
        ],
    );
    let moved: Vec<_> = moves(&manifest, &reader).blocks.iter().map(found).collect();
    assert_eq!(
        moved,
        [
            ("src/a.rs".into(), (1, 6), "src/c.rs".into(), (5, 10)),
            // Not dropped whole: its first four lines are still free and still a block.
            ("src/b.rs".into(), (1, 4), "src/c.rs".into(), (1, 4)),
        ]
    );
}

#[test]
fn a_block_starting_at_a_line_with_many_copies_is_still_found() {
    const MOVED: &str = "#[test]\nfn case_70() {\n    assert_eq!(value(70), 70);\n}\n";
    // 100 tests all start with `#[test]`; the 70th moves out on its own.
    let mut suite = String::new();
    for n in 1..=100 {
        let _ = write!(
            suite,
            "#[test]\nfn case_{n}() {{\n    assert_eq!(value({n}), {n});\n}}\n"
        );
    }
    let suite: &'static str = Box::leak(suite.into_boxed_str());
    let mut reader = Reader::new();
    let manifest = whole(
        &mut reader,
        &[
            ("tests/one.rs", Some(MOVED), None),
            ("tests/all.rs", None, Some(suite)),
        ],
    );
    let found_blocks = moves(&manifest, &reader);
    assert!(!found_blocks.cut);
    let moved: Vec<_> = found_blocks.blocks.iter().map(found).collect();
    assert_eq!(
        moved,
        [(
            "tests/one.rs".into(),
            (1, 4),
            "tests/all.rs".into(),
            (277, 280)
        )]
    );
}

#[test]
fn a_change_of_many_identical_lines_stops_at_the_work_bound_and_says_so() {
    let same: &'static str = Box::leak("let unit_value = 1;\n".repeat(1100).into_boxed_str());
    let mut reader = Reader::new();
    let manifest = whole(
        &mut reader,
        &[
            ("src/a.rs", Some(same), None),
            ("src/b.rs", None, Some(same)),
        ],
    );
    let found_blocks = moves(&manifest, &reader);
    assert!(found_blocks.cut, "over a million candidate places");
    let moved: Vec<_> = found_blocks.blocks.iter().map(found).collect();
    assert_eq!(
        moved[0],
        ("src/a.rs".into(), (1, 1100), "src/b.rs".into(), (1, 1100))
    );

    let mut context = Context::default();
    let span = reader.at(SourceSide::Head, "src/b.rs", 1, 3);
    context.add_span(&span, "the moved lines", &reader);
    context.add_moves(&manifest, &reader);
    let block = context.blocks.last().expect("moves block");
    assert!(
        block
            .content
            .contains("move detection stopped at its work bound"),
        "{}",
        block.content
    );
}
