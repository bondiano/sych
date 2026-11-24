//! The six checks on the fixtures of each language, and what a change does to them against its
//! diff base.

use std::fs;
use std::path::Path;

use sych_adapters::architecture::Built;
use sych_core::Language;
use sych_core::architecture::{Change, Limits, Model, check, compare};

mod common;

use common::architecture::{LANGUAGES, built, fixture, fixture_dir, sources};

fn keys(model: &Model) -> Vec<String> {
    check(model, &Limits::default())
        .into_iter()
        .map(|d| format!("{} {}", d.check.name(), d.key))
        .collect()
}

fn semantic(language: Language) -> [String; 4] {
    let (p, src) = match language {
        Language::Go => ("example.com/fx", ""),
        Language::Typescript => ("fx", "::src"),
        Language::Rust | Language::Python => ("fx", ""),
    };
    let module = |m: &str| format!("{p}{src}::{m}");
    let (cycle, tangle, field) = match language {
        Language::Go => (
            format!("{p}::net::kinds,{p}::net::socket"),
            (0..8)
                .map(|i| format!("{p}::ring::T{i}"))
                .collect::<Vec<_>>()
                .join(","),
            "Queue::Items",
        ),
        Language::Python => (
            format!("{},{}", module("kinds"), module("sockets")),
            format!(
                "{},{}",
                module("kernel::Tasks"),
                module("kernel::timers::Timers")
            ),
            "Queue::items",
        ),
        _ => (
            format!("{},{}", module("kinds"), module("socket")),
            format!(
                "{},{}",
                module("kernel::Tasks"),
                module("kernel::timers::Timers")
            ),
            "Queue::items",
        ),
    };
    [
        format!("module-cycle {cycle}"),
        format!("tangle {tangle}"),
        format!("field-leak {}::{field}", module("queue")),
        format!("shallow-module {}", module("inventory")),
    ]
}

#[test]
fn the_four_semantic_checks_find_the_smelly_fixture_and_nothing_in_the_clean_one() {
    for (language, name) in LANGUAGES {
        assert_eq!(
            keys(&fixture(language, name, "smelly").model),
            semantic(language),
            "{name}"
        );
        assert_eq!(
            keys(&fixture(language, name, "clean").model),
            Vec::<String>::new(),
            "{name}"
        );
    }
}

fn extension(language: Language) -> &'static str {
    match language {
        Language::Rust => "rs",
        Language::Go => "go",
        Language::Typescript => "ts",
        Language::Python => "py",
    }
}

/// `dir`'s files with `count` more in `wide/`, one of them `lines` lines long.
fn laid_out(
    language: Language,
    dir: &Path,
    count: usize,
    lines: usize,
) -> (tempfile::TempDir, Built) {
    let copy = tempfile::tempdir().expect("copy");
    for (path, bytes) in sources(dir) {
        let to = copy.path().join(path);
        fs::create_dir_all(to.parent().expect("parent")).expect("mkdir");
        fs::write(to, bytes).expect("write");
    }
    let extension = extension(language);
    let wide = copy.path().join("wide");
    fs::create_dir_all(&wide).expect("wide");
    for i in 0..count {
        let text = if i == 0 {
            "\n".repeat(lines)
        } else {
            "\n".to_owned()
        };
        fs::write(wide.join(format!("f{i:02}.{extension}")), text).expect("write");
    }
    let index = fs::read(dir.join("index.scip")).expect("an index");
    let built = built(language, &index, copy.path());
    (copy, built)
}

#[test]
fn a_directory_of_sixteen_files_and_a_file_of_2001_lines_are_found_in_every_language() {
    for (language, name) in LANGUAGES {
        let dir = fixture_dir(name, "clean");
        let (_copy, past) = laid_out(language, &dir, 16, 2_001);
        let layout: Vec<String> = keys(&past.model)
            .into_iter()
            .filter(|k| k.starts_with("wide-") || k.starts_with("long-"))
            .collect();
        let long = format!("long-file wide/f00.{}", extension(language));
        assert_eq!(layout, ["wide-directory wide/".to_owned(), long], "{name}");
        let (_copy, at) = laid_out(language, &dir, 15, 2_000);
        assert_eq!(
            keys(&at.model),
            Vec::<String>::new(),
            "{name}: 15 and 2000 are within bounds"
        );
    }
}

#[test]
fn against_the_base_a_change_says_what_it_introduced_kept_and_fixed() {
    let same = |p: &str| p.to_owned();
    for (language, name) in LANGUAGES {
        let check =
            |variant: &str| check(&fixture(language, name, variant).model, &Limits::default());
        let (smelly, clean) = (check("smelly"), check("clean"));
        let introduced = compare(Some(&clean), smelly.clone(), &same);
        assert!(
            introduced
                .found
                .iter()
                .all(|f| f.change == Some(Change::New) && f.base_size.is_none()),
            "{name}"
        );
        assert_eq!(introduced.found.len(), 4, "{name}");
        let kept = compare(Some(&smelly), smelly.clone(), &same);
        assert!(
            kept.found
                .iter()
                .all(|f| f.change == Some(Change::Same) && f.base_size.is_some()),
            "{name}"
        );
        let fixed = compare(Some(&smelly), clean, &same);
        assert_eq!(fixed.found.len(), 0, "{name}");
        assert_eq!(fixed.fixed.len(), 4, "{name}");
        let unknown = compare(None, smelly, &same);
        assert!(unknown.found.iter().all(|f| f.change.is_none()), "{name}");
    }
}
