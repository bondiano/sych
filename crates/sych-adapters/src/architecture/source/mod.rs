//! What the syntax of one indexed file says that its SCIP index does not: where each item's
//! code is, whether a function takes a receiver or implements an interface, which functions
//! only wrap others, what each field holds, and what the code does with each member it names.
//! One reader per language; positions are zero-based lines and byte columns. The readers
//! parse; they never run the code they read.

mod go;
mod python;
mod rust;
mod typescript;

use std::iter;

use sych_core::Language;

/// Zero-based line and byte column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Pos {
    pub line: u32,
    pub column: u32,
}

/// `end` exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: Pos,
    pub end: Pos,
}

impl Span {
    pub fn holds(self, at: Pos) -> bool {
        self.start <= at && at < self.end
    }
}

#[derive(Debug, Default)]
pub struct SourceFacts {
    /// Import declarations: what they name is no use.
    pub imports: Vec<Span>,
    /// Code built only for tests, inside a file that is not a test file.
    pub tests: Vec<Span>,
    pub definitions: Vec<Definition>,
    /// Each name of a member (`x.f`'s `f`, `x.m()`'s `m`) with what the code does there.
    pub members: Vec<(Pos, Access)>,
    /// What the index cannot see in this file, said once per kind.
    pub unknowns: Vec<&'static str>,
}

#[derive(Debug, Clone)]
pub struct Definition {
    /// Where its name is written.
    pub name: Pos,
    /// The whole item, body included.
    pub extent: Span,
    pub takes_self: bool,
    pub trait_impl: bool,
    /// For a field: what its declared type holds.
    pub plain: Plain,
    pub shape: Option<Shape>,
}

/// A function that adds nothing of its own, by the names its body writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    /// One call passing its parameters on: the called name, and the fields of `self` it is
    /// called on.
    Forward { callee: Pos, fields: Vec<Pos> },
    /// Hands out or overwrites fields of `self`: those it lends or overwrites, those whose
    /// values it hands out, and whether it overwrites nothing.
    Access {
        lent: Vec<Pos>,
        copied: Vec<Pos>,
        to_read: bool,
    },
}

/// What code does with a member it names (deslop-please's `Access`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Takes a copy of its value, or gives or takes the value whole.
    Copy,
    /// Borrows it to read; where values are shared references (Go, TypeScript, Python), any
    /// use of the value as a whole.
    Borrow,
    /// Calls a method on it.
    Call,
    /// Writes it, borrows it to write, indexes it or reaches into it.
    Reach,
}

/// What a field's declared type holds (deslop-please's `Plain`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plain {
    /// A number, `bool`, `char`, or an immutable string where strings are values: nothing can
    /// be done to it but read or overwrite it.
    Scalar,
    /// Numbers and text in a list, option, box, array or tuple: nothing inside but its value.
    Data,
    /// A marker, `PhantomData` or `PhantomPinned`: nothing at all.
    Nothing,
    /// Anything else, and anything undeclared.
    No,
}

impl Definition {
    fn new(name: Pos, extent: Span) -> Definition {
        Definition {
            name,
            extent,
            takes_self: false,
            trait_impl: false,
            plain: Plain::No,
            shape: None,
        }
    }
}

/// The facts of `text`, a file of `language`; `None` when it does not parse.
pub fn read(language: Language, path: &str, text: &str) -> Option<SourceFacts> {
    match language {
        Language::Rust => rust::read(text),
        Language::Go => go::read(text),
        Language::Typescript => typescript::read(path, text),
        Language::Python => python::read(text),
    }
}

/// Byte offsets of each line's start, to turn offsets into positions.
pub(crate) struct Lines {
    starts: Vec<usize>,
}

impl Lines {
    pub fn new(text: &str) -> Lines {
        let starts = iter::once(0)
            .chain(text.match_indices('\n').map(|(at, _)| at + 1))
            .collect();
        Lines { starts }
    }

    pub fn pos(&self, offset: usize) -> Pos {
        let line = self.starts.partition_point(|&start| start <= offset) - 1;
        Pos {
            line: u32::try_from(line).unwrap_or(u32::MAX),
            column: u32::try_from(offset - self.starts[line]).unwrap_or(u32::MAX),
        }
    }

    pub fn span(&self, start: usize, end: usize) -> Span {
        Span {
            start: self.pos(start),
            end: self.pos(end),
        }
    }
}

/// The byte column of `column`, counted in UTF-16 code units or in chars, on `line`.
pub fn byte_column(line: &str, column: u32, utf16: bool) -> u32 {
    if line.is_ascii() {
        return column;
    }

    let mut units = 0u32;
    for (at, c) in line.char_indices() {
        if units >= column {
            return u32::try_from(at).unwrap_or(u32::MAX);
        }

        units += if utf16 {
            u32::try_from(c.len_utf16()).unwrap_or(1)
        } else {
            1
        };
    }
    u32::try_from(line.len()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_become_lines_and_byte_columns() {
        let lines = Lines::new("ab\ncd\n\nef");
        assert_eq!(lines.pos(0), Pos { line: 0, column: 0 });
        assert_eq!(lines.pos(4), Pos { line: 1, column: 1 });
        assert_eq!(lines.pos(7), Pos { line: 3, column: 0 });
        assert_eq!(byte_column("let é = x", 6, true), 7);
        assert_eq!(byte_column("😀.f", 2, true), 4);
        assert_eq!(byte_column("😀.f", 1, false), 4);
        assert_eq!(byte_column("abc", 2, true), 2);
    }
}
