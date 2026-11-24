//! What a change does to a project's structure: each finding on head against the diff base's.
//! A finding is known by its check and key, never by a line: the members of a circle, a
//! field's or a module's name, a path. A circle that loses or gains members is the same one,
//! smaller or larger; a file the change renames keeps its findings. Without a checked base
//! nothing is new or old.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::checks::{Check, Diagnostic};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Change {
    /// The base has nothing like it.
    New,
    /// The base has it, smaller.
    Worse,
    /// The base has it as it is: existing debt.
    Same,
    /// The base has it, larger: the change improved it.
    Better,
}

/// Head's findings, each with what the base says of it, and the base's that head no longer has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    pub found: Vec<Found>,
    /// The base's findings head has no longer: fixed, or moved out of sight.
    pub fixed: Vec<Diagnostic>,
}

/// A finding on head and what the base says of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub diagnostic: Diagnostic,
    /// `None` when the base was not checked.
    pub change: Option<Change>,
    /// The size of the base finding it matched.
    pub base_size: Option<usize>,
}

/// `base`: the diff base's findings, `None` when it was not checked. `head_path`: where a path
/// of the base is on head.
pub fn compare(
    base: Option<&[Diagnostic]>,
    head: Vec<Diagnostic>,
    head_path: &dyn Fn(&str) -> String,
) -> Comparison {
    let Some(base) = base else {
        return Comparison {
            found: head
                .into_iter()
                .map(|diagnostic| Found {
                    diagnostic,
                    change: None,
                    base_size: None,
                })
                .collect(),
            fixed: Vec::new(),
        };
    };
    let base: Vec<Diagnostic> = base.iter().map(|d| moved(d, head_path)).collect();
    let mut matched = vec![false; base.len()];
    let found = head
        .into_iter()
        .map(|diagnostic| {
            let at = same(&base, &diagnostic).or_else(|| overlapping(&base, &diagnostic));
            let base_size = at.map(|i| {
                matched[i] = true;
                base[i].size
            });
            let change = match base_size.map(|before| diagnostic.size.cmp(&before)) {
                None => Change::New,
                Some(Ordering::Greater) => Change::Worse,
                Some(Ordering::Equal) => Change::Same,
                Some(Ordering::Less) => Change::Better,
            };
            Found {
                diagnostic,
                change: Some(change),
                base_size,
            }
        })
        .collect();
    let fixed = base
        .into_iter()
        .zip(matched)
        .filter(|(_, matched)| !matched)
        .map(|(d, _)| d)
        .collect();
    Comparison { found, fixed }
}

/// The base finding, its paths as head writes them.
fn moved(d: &Diagnostic, head_path: &dyn Fn(&str) -> String) -> Diagnostic {
    let mut d = d.clone();
    if d.check == Check::LongFile {
        d.key = head_path(&d.key);
    }

    if let Some(location) = &mut d.location {
        location.file = head_path(&location.file);
    }

    d
}

/// The base finding of the same check and key; for a field or a module renamed with its file,
/// the one of the same last name in the same file.
fn same(base: &[Diagnostic], d: &Diagnostic) -> Option<usize> {
    let last = |key: &str| key.rsplit("::").next().unwrap_or(key).to_owned();
    let file = |d: &Diagnostic| d.location.as_ref().map(|l| l.file.clone());
    base.iter()
        .position(|b| b.check == d.check && b.key == d.key)
        .or_else(|| {
            matches!(d.check, Check::FieldLeak | Check::ShallowModule).then(|| {
                base.iter().position(|b| {
                    b.check == d.check
                        && last(&b.key) == last(&d.key)
                        && file(b).is_some()
                        && file(b) == file(d)
                })
            })?
        })
}

/// The base circle that shares members with this one: the same circle, grown or shrunk.
fn overlapping(base: &[Diagnostic], d: &Diagnostic) -> Option<usize> {
    if d.members.is_empty() {
        return None;
    }

    let members: BTreeSet<&String> = d.members.iter().collect();
    base.iter()
        .enumerate()
        .filter(|(_, b)| b.check == d.check)
        .map(|(i, b)| (i, b.members.iter().filter(|m| members.contains(m)).count()))
        .filter(|(_, shared)| *shared > 0)
        .max_by_key(|(_, shared)| *shared)
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::architecture::checks::{Location, Picture};

    fn finding(check: Check, key: &str, size: usize, members: &[&str], file: &str) -> Diagnostic {
        Diagnostic {
            check,
            key: key.into(),
            size,
            members: members.iter().map(|m| (*m).to_owned()).collect(),
            message: String::new(),
            location: Some(Location {
                file: file.into(),
                line: 1,
            }),
            notes: Vec::new(),
            picture: Picture::Bar {
                label: String::new(),
                length: size,
                most: 0,
            },
        }
    }

    fn changes(comparison: &Comparison) -> Vec<Option<Change>> {
        comparison.found.iter().map(|f| f.change).collect()
    }

    #[test]
    fn new_worse_same_and_better_by_key_and_size_and_what_head_lost_is_fixed() {
        let base = [
            finding(Check::LongFile, "src/a.rs", 2_100, &[], "src/a.rs"),
            finding(Check::WideDirectory, "src/", 16, &[], "src/lib.rs"),
            finding(Check::FieldLeak, "p::q::Q::items", 2, &[], "src/app.rs"),
            finding(Check::ShallowModule, "p::old", 9, &[], "src/old.rs"),
        ];
        let head = vec![
            finding(Check::LongFile, "src/a.rs", 2_300, &[], "src/a.rs"),
            finding(Check::WideDirectory, "src/", 16, &[], "src/lib.rs"),
            finding(Check::FieldLeak, "p::q::Q::items", 1, &[], "src/app.rs"),
            finding(Check::LongFile, "src/b.rs", 2_001, &[], "src/b.rs"),
        ];
        let compared = compare(Some(&base), head, &|p| p.to_owned());
        assert_eq!(
            changes(&compared),
            [
                Some(Change::Worse),
                Some(Change::Same),
                Some(Change::Better),
                Some(Change::New)
            ]
        );
        assert_eq!(compared.found[0].base_size, Some(2_100));
        let fixed: Vec<&str> = compared.fixed.iter().map(|d| d.key.as_str()).collect();
        assert_eq!(fixed, ["p::old"]);
    }

    #[test]
    fn a_circle_that_shrinks_or_grows_is_the_same_circle() {
        let base = [finding(
            Check::ModuleCycle,
            "p::a,p::b,p::c",
            3,
            &["p::a", "p::b", "p::c"],
            "src/a.rs",
        )];
        let shrunk = vec![finding(
            Check::ModuleCycle,
            "p::a,p::b",
            2,
            &["p::a", "p::b"],
            "src/a.rs",
        )];
        let grown = vec![finding(
            Check::ModuleCycle,
            "p::a,p::b,p::c,p::d",
            4,
            &["p::a", "p::b", "p::c", "p::d"],
            "src/a.rs",
        )];
        let other = vec![finding(
            Check::ModuleCycle,
            "p::x,p::y",
            2,
            &["p::x", "p::y"],
            "src/x.rs",
        )];
        assert_eq!(
            changes(&compare(Some(&base), shrunk, &|p| p.to_owned())),
            [Some(Change::Better)]
        );
        assert_eq!(
            changes(&compare(Some(&base), grown, &|p| p.to_owned())),
            [Some(Change::Worse)]
        );
        let unrelated = compare(Some(&base), other, &|p| p.to_owned());
        assert_eq!(changes(&unrelated), [Some(Change::New)]);
        assert_eq!(unrelated.fixed.len(), 1);
    }

    #[test]
    fn a_renamed_file_keeps_its_findings_and_without_a_base_nothing_is_new() {
        let base = [
            finding(Check::LongFile, "src/old.rs", 2_001, &[], "src/old.rs"),
            finding(Check::FieldLeak, "p::old::Q::items", 1, &[], "src/old.rs"),
        ];
        let head = vec![
            finding(Check::LongFile, "src/new.rs", 2_001, &[], "src/new.rs"),
            finding(Check::FieldLeak, "p::new::Q::items", 1, &[], "src/new.rs"),
        ];
        let renamed = |p: &str| p.replace("src/old.rs", "src/new.rs");
        let compared = compare(Some(&base), head.clone(), &renamed);
        assert_eq!(changes(&compared), [Some(Change::Same), Some(Change::Same)]);
        assert_eq!(compared.fixed.len(), 0);
        let unknown = compare(None, head, &renamed);
        assert_eq!(changes(&unknown), [None, None]);
        assert_eq!(unknown.fixed.len(), 0);
    }
}
