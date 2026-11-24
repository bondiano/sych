//! A project's SCIP index and the syntax of its files, turned into the plain model. What the
//! index resolves decides who uses what; the syntax decides what each use does, which
//! functions only wrap others and which code is built only for tests.

mod modules;
mod wrappers;

use std::collections::{HashMap, HashSet};

use sych_core::architecture::{
    File, FileId, Item, ItemId, ItemKind, Model, NodeId, NodeKind, Reference, Tree,
};
use sych_core::{Language, PathClass, classify};

use super::scip::{Document, Index, Kind, Position, Symbol};
use super::source::{self, Access, Definition, Plain, Pos, SourceFacts, Span};

/// The model, and what the index and the syntax left unknown.
pub struct Built {
    pub model: Model,
    /// Member names the syntax shows that the index resolves to nothing, though the project
    /// has a field or function of that name: its uses may be missed. Always 0 for Rust and
    /// Go.
    pub unresolved: usize,
    /// Member names the syntax shows.
    pub members: usize,
    /// Indexed files whose syntax could not be read.
    pub unparsed: Vec<String>,
    /// What the index cannot see, once per kind.
    pub unknowns: Vec<&'static str>,
}

/// One indexed file: its document, text and syntax.
struct Unit<'i> {
    file: FileId,
    document: &'i Document<'i>,
    text: String,
    facts: SourceFacts,
    /// Where each occurrence's name starts, in byte columns.
    positions: Vec<Pos>,
    /// The first occurrence of a project or outside symbol at each position.
    at: HashMap<Pos, usize>,
    /// The items it defines whose code can hold references, by extent.
    holders: Vec<(Span, ItemId)>,
}

/// `text` gives a file's text by its path relative to the project; `dir` is the project's
/// directory in the repository, `""` for its root. `sources`: every source file of the
/// language in the project, by the same paths; those the index leaves out count for layout
/// only.
pub fn build(
    language: Language,
    dir: &str,
    index: &Index<'_>,
    text: &dyn Fn(&str) -> Option<String>,
    sources: &[String],
) -> Built {
    let mut builder = Builder {
        language,
        tree: Tree::new(),
        items: Vec::new(),
        symbols: HashMap::new(),
        definitions: HashMap::new(),
        packages: HashMap::new(),
    };
    let information: HashMap<&str, u64> = index
        .documents
        .iter()
        .flat_map(|d| &d.symbols)
        .map(|s| (s.symbol, s.kind))
        .collect();
    let mut files = Vec::new();
    let mut units = Vec::new();
    let mut unparsed = Vec::new();
    let mut unknowns: Vec<&'static str> = Vec::new();
    // scip-go also indexes what it compiled into the build cache, outside the project.
    let inside = index
        .documents
        .iter()
        .filter(|d| !d.path.starts_with("../") && !d.path.starts_with('/'));
    for document in inside {
        let path = if dir.is_empty() {
            document.path.clone()
        } else {
            format!("{dir}/{}", document.path)
        };
        let Some(text) = text(&document.path) else {
            unparsed.push(path);
            continue;
        };
        let (unit, parsed) = Unit::new(language, FileId::new(files.len()), document, text);
        if !parsed {
            unparsed.push(path.clone());
        }

        for unknown in &unit.facts.unknowns {
            if !unknowns.contains(unknown) {
                unknowns.push(unknown);
            }
        }
        let module = builder.file_module(document, &path);
        files.push(File {
            test: classify(&path, false) == PathClass::Test,
            path,
            module,
            lines: unit.text.lines().count(),
        });
        units.push(unit);
    }

    let indexed: HashSet<&str> = units.iter().map(|u| u.document.path.as_str()).collect();
    let unindexed = sources.iter().filter(|s| !indexed.contains(s.as_str()));
    files.extend(layout_only(dir, unindexed, text, builder.tree.root()));

    // Items first: a reference may name one a later document defines.
    for unit in &mut units {
        builder.define(unit, &information);
    }
    builder.parent_impls();
    let mut references = Vec::new();
    let mut accesses = Vec::new();
    for unit in &units {
        builder.refer(unit, &files, &mut references, &mut accesses);
    }
    let members = units.iter().map(|u| u.facts.members.len()).sum();
    // Rust's and Go's indexes are the compiler's: what they leave unresolved is outside the
    // project, or under a macro, which `unknowns` says.
    let unresolved = if matches!(language, Language::Rust | Language::Go) {
        0
    } else {
        unresolved(&units, &builder.items)
    };
    builder.mark_tests(&units, &mut files);
    for reference in &mut references {
        let holder_test = reference
            .holder
            .is_some_and(|h| builder.tree.is_test(builder.items[h.index()].owner));
        reference.test |= files[reference.file.index()].test
            || builder.tree.is_test(reference.from)
            || holder_test;
    }
    let wrappers = wrappers::wrappers(&builder, &units);
    let plain = builder.plain();
    wrappers::mark_copies(
        &builder.items,
        &mut references,
        &accesses,
        &wrappers,
        &plain,
    );
    Built {
        model: Model {
            tree: builder.tree,
            files,
            items: builder.items,
            references,
            wrappers,
        },
        unresolved,
        members,
        unparsed,
        unknowns,
    }
}

struct Builder<'i> {
    language: Language,
    tree: Tree,
    items: Vec<Item>,
    /// Item by symbol text, for the symbols the project defines.
    symbols: HashMap<&'i str, ItemId>,
    /// Each item's syntax, matched by where its definition is written.
    definitions: HashMap<ItemId, Definition>,
    packages: HashMap<String, NodeId>,
}

impl<'i> Builder<'i> {
    fn module(&mut self, symbol: &Symbol<'_>) -> Option<NodeId> {
        let place = modules::place(self.language, symbol)?;
        let root = self.tree.root();
        let package = *self
            .packages
            .entry(place.package.clone())
            .or_insert_with(|| self.tree.child(root, NodeKind::Package, &place.package));
        if place.test {
            self.tree.node_mut(package).test = true;
        }

        Some(place.modules.iter().fold(package, |node, name| {
            self.tree.child(node, NodeKind::Module, name)
        }))
    }

    /// The module a file is: the first module its index defines, else the one its first
    /// definition is in, else a package named after its directory.
    fn file_module(&mut self, document: &Document<'_>, path: &str) -> NodeId {
        let symbols = document
            .occurrences
            .iter()
            .filter(|o| o.definition)
            .filter_map(|o| Symbol::parse(o.symbol).ok().flatten());
        let (mut modules, mut items): (Vec<_>, Vec<_>) =
            symbols.partition(|s| s.inner().is_empty() || s.kind() == Kind::Meta);
        let first = if modules.is_empty() {
            items.drain(..).next()
        } else {
            Some(modules.remove(0))
        };
        first.and_then(|s| self.module(&s)).unwrap_or_else(|| {
            let root = self.tree.root();
            let directory = path.rsplit_once('/').map_or("", |(d, _)| d);
            self.tree.child(root, NodeKind::Package, directory)
        })
    }

    fn define(&mut self, unit: &mut Unit<'i>, information: &HashMap<&str, u64>) {
        let by_name: HashMap<Pos, &Definition> =
            unit.facts.definitions.iter().map(|d| (d.name, d)).collect();
        let lines: Vec<&str> = unit.text.lines().collect();
        for (index, occurrence) in unit.document.occurrences.iter().enumerate() {
            if !occurrence.definition || self.symbols.contains_key(occurrence.symbol) {
                continue;
            }

            let Ok(Some(symbol)) = Symbol::parse(occurrence.symbol) else {
                continue;
            };
            let container = symbol
                .container()
                .and_then(|c| self.symbols.get(c).copied());
            let in_type = container.is_some_and(|c| {
                matches!(
                    self.items[c.index()].kind,
                    ItemKind::Type | ItemKind::Variant
                )
            }) || symbol.impl_of().is_some();
            let Some(kind) = modules::kind(
                &symbol,
                information.get(occurrence.symbol).copied(),
                in_type,
            ) else {
                continue;
            };
            let Some(owner) = self.module(&symbol) else {
                continue;
            };
            let id = ItemId::new(self.items.len());
            let definition = by_name.get(&unit.positions[index]).map(|d| (*d).clone());
            let trait_impl = symbol.impl_of().is_some_and(|(_, of_trait)| of_trait)
                || definition.as_ref().is_some_and(|d| d.trait_impl);
            self.items.push(Item {
                name: symbol.item_name(),
                owner,
                module: None,
                file: Some(unit.file),
                kind,
                parent: container.filter(|_| kind != ItemKind::Impl),
                trait_impl,
                takes_self: definition.as_ref().is_some_and(|d| d.takes_self),
                holds_nothing: definition
                    .as_ref()
                    .is_some_and(|d| d.plain == Plain::Nothing),
            });
            self.symbols.insert(occurrence.symbol, id);
            let utf16 = unit.document.utf16;
            let extent = occurrence
                .enclosing
                .map(|r| Span {
                    start: byte_pos(&lines, r.start, utf16),
                    end: byte_pos(&lines, r.end, utf16),
                })
                .or(definition.as_ref().map(|d| d.extent));
            if let Some(extent) = extent
                && kind != ItemKind::Impl
            {
                unit.holders.push((extent, id));
            }

            if let Some(definition) = definition {
                self.definitions.insert(id, definition);
            }
        }
        // Innermost last among those that start before a position: sorted by start, the
        // longest first.
        unit.holders
            .sort_by(|(a, _), (b, _)| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    }

    /// The type each Rust `impl` block's items are for, by the type's name in the block's
    /// module, else by a unique name in the project.
    fn parent_impls(&mut self) {
        let mut by_name: HashMap<String, Vec<ItemId>> = HashMap::new();
        for (index, item) in self.items.iter().enumerate() {
            if item.kind == ItemKind::Type && item.parent.is_none() {
                by_name
                    .entry(item.name.clone())
                    .or_default()
                    .push(ItemId::new(index));
            }
        }
        let symbols: Vec<(&str, ItemId)> = self.symbols.iter().map(|(s, i)| (*s, *i)).collect();
        for (text, id) in symbols {
            let Ok(Some(symbol)) = Symbol::parse(text) else {
                continue;
            };
            let Some((self_type, _)) = symbol.impl_of() else {
                continue;
            };
            let bare = self_type
                .trim_start_matches('&')
                .trim_start_matches("mut ")
                .split('<')
                .next()
                .unwrap_or(self_type)
                .rsplit("::")
                .next()
                .unwrap_or(self_type)
                .trim();
            let owner = self.items[id.index()].owner;
            let candidates = by_name.get(bare).map_or(&[][..], Vec::as_slice);
            let parent = candidates
                .iter()
                .find(|c| self.items[c.index()].owner == owner)
                .or(match candidates {
                    [only] => Some(only),
                    _ => None,
                })
                .copied();
            self.items[id.index()].parent = parent;
        }
    }

    fn refer(
        &self,
        unit: &Unit<'_>,
        files: &[File],
        out: &mut Vec<Reference>,
        accesses: &mut Vec<Access>,
    ) {
        let access: HashMap<Pos, Access> = unit.facts.members.iter().copied().collect();
        let module = files[unit.file.index()].module;
        for (occurrence, pos) in unit.document.occurrences.iter().zip(&unit.positions) {
            if occurrence.definition {
                continue;
            }

            let Some(&item) = self.symbols.get(occurrence.symbol) else {
                continue;
            };
            if self.items[item.index()].kind == ItemKind::Impl
                || unit.facts.imports.iter().any(|span| span.holds(*pos))
            {
                continue;
            }

            let holder = innermost(&unit.holders, *pos);
            let from = holder.map_or(module, |h| self.items[h.index()].owner);
            out.push(Reference {
                file: unit.file,
                line: pos.line + 1,
                from,
                holder,
                item,
                copies: false,
                test: unit.facts.tests.iter().any(|span| span.holds(*pos)),
            });
            accesses.push(access.get(pos).copied().unwrap_or(Access::Copy));
        }
    }

    /// Modules built only for tests: those declared inside test code, Rust's `tests` and
    /// `test` modules, and those only test files define.
    fn mark_tests(&mut self, units: &[Unit<'_>], files: &mut [File]) {
        let mut test_modules = HashSet::new();
        let mut other_modules = HashSet::new();
        for unit in units {
            let file_module = files[unit.file.index()].module;
            for (occurrence, pos) in unit.document.occurrences.iter().zip(&unit.positions) {
                if !unit.facts.tests.iter().any(|span| span.holds(*pos)) {
                    continue;
                }

                let Ok(Some(symbol)) = Symbol::parse(occurrence.symbol) else {
                    continue;
                };
                if !symbol.inner().is_empty() {
                    continue;
                }

                // Declared here: below this file's module. `super::` names an ancestor.
                let Some(module) = self.module(&symbol) else {
                    continue;
                };
                if module != file_module && self.tree.is_within(module, file_module) {
                    self.tree.node_mut(module).test = true;
                }
            }
            let file = &files[unit.file.index()];
            if file.test {
                test_modules.insert(file.module);
            } else {
                other_modules.insert(file.module);
            }
        }
        let ancestors: Vec<NodeId> = other_modules
            .iter()
            .flat_map(|&id| self.tree.ancestors(id))
            .collect();
        other_modules.extend(ancestors);
        for id in self.tree.ids() {
            let node = self.tree.node(id);
            let named_test = self.language == Language::Rust
                && node.kind == NodeKind::Module
                && matches!(node.name.as_str(), "tests" | "test");
            if named_test || (test_modules.contains(&id) && !other_modules.contains(&id)) {
                self.tree.node_mut(id).test = true;
            }
        }
        for file in files.iter_mut() {
            file.test |= self.tree.is_test(file.module);
        }
    }

    fn plain(&self) -> HashMap<ItemId, Plain> {
        self.definitions
            .iter()
            .filter(|(id, _)| self.items[id.index()].kind == ItemKind::Field)
            .map(|(id, d)| (*id, d.plain))
            .collect()
    }
}

impl<'i> Unit<'i> {
    /// The unit of `document`, and whether its syntax parsed.
    fn new(
        language: Language,
        file: FileId,
        document: &'i Document<'i>,
        text: String,
    ) -> (Unit<'i>, bool) {
        let read = source::read(language, &document.path, &text);
        let parsed = read.is_some();
        let lines: Vec<&str> = text.lines().collect();
        let positions: Vec<Pos> = document
            .occurrences
            .iter()
            .map(|o| byte_pos(&lines, o.range.start, document.utf16))
            .collect();
        let mut at = HashMap::new();
        for (index, occurrence) in document.occurrences.iter().enumerate() {
            if !occurrence.symbol.starts_with("local ") {
                at.entry(positions[index]).or_insert(index);
            }
        }
        let unit = Unit {
            file,
            document,
            text,
            facts: read.unwrap_or_default(),
            positions,
            at,
            holders: Vec::new(),
        };
        (unit, parsed)
    }
}

/// Member names no occurrence resolves, though the project has a field or function so named.
fn unresolved(units: &[Unit<'_>], items: &[Item]) -> usize {
    let own: HashSet<&str> = items
        .iter()
        .filter(|i| matches!(i.kind, ItemKind::Field | ItemKind::Function))
        .filter_map(|i| i.name.rsplit("::").next())
        .collect();
    units
        .iter()
        .map(|unit| {
            unit.facts
                .members
                .iter()
                .filter(|(pos, _)| !unit.at.contains_key(pos))
                .filter(|(pos, _)| own.contains(identifier(&unit.text, *pos)))
                .count()
        })
        .sum()
}

/// Files the index leaves out, which count for layout only: in no module but the project.
fn layout_only<'s>(
    dir: &str,
    sources: impl Iterator<Item = &'s String>,
    text: &dyn Fn(&str) -> Option<String>,
    project: NodeId,
) -> Vec<File> {
    sources
        .filter_map(|source| {
            let lines = text(source)?.lines().count();
            let path = if dir.is_empty() {
                source.clone()
            } else {
                format!("{dir}/{source}")
            };
            Some(File {
                test: classify(&path, false) == PathClass::Test,
                path,
                module: project,
                lines,
            })
        })
        .collect()
}

/// The item whose extent holds `pos` and starts last: the innermost, as extents nest.
fn innermost(holders: &[(Span, ItemId)], pos: Pos) -> Option<ItemId> {
    let before = holders.partition_point(|(span, _)| span.start <= pos);
    holders[..before]
        .iter()
        .rev()
        .find(|(span, _)| span.holds(pos))
        .map(|(_, id)| *id)
}

/// The identifier that starts at `pos` in `text`.
fn identifier(text: &str, pos: Pos) -> &str {
    let line = text.lines().nth(pos.line as usize).unwrap_or_default();
    let rest = line.get(pos.column as usize..).unwrap_or_default();
    let end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .unwrap_or(rest.len());
    &rest[..end]
}

/// A SCIP position on `lines`, in a byte column.
fn byte_pos(lines: &[&str], at: Position, utf16: bool) -> Pos {
    let text = lines.get(at.line as usize).copied().unwrap_or_default();
    Pos {
        line: at.line,
        column: source::byte_column(text, at.column, utf16),
    }
}
