//! A SCIP index (github.com/sourcegraph/scip, `scip.proto`): the documents, and in each every
//! name written there with the symbol it resolves to. Only the fields read here are decoded;
//! the rest are skipped. Ported from deslop-please (`f678d10`, MIT; see
//! `sych-core/src/architecture/UPSTREAM-LICENSE`), with the typed ranges newer indexers write
//! and the position encoding of each document.

mod protobuf;
mod symbol;

pub use protobuf::Malformed;
pub use symbol::{Kind, Symbol};

use protobuf::{Fields, Value};

pub struct Index<'a> {
    pub documents: Vec<Document<'a>>,
}

pub struct Document<'a> {
    /// Relative to the indexed project's root, `/`-separated.
    pub path: String,
    pub occurrences: Vec<Occurrence<'a>>,
    /// What the document says about each symbol it defines.
    pub symbols: Vec<SymbolInformation<'a>>,
    /// Columns count UTF-16 code units, not bytes.
    pub utf16: bool,
}

pub struct SymbolInformation<'a> {
    pub symbol: &'a str,
    /// `scip.proto`'s `SymbolInformation.Kind`.
    pub kind: u64,
}

pub struct Occurrence<'a> {
    pub range: Range,
    pub symbol: &'a str,
    pub definition: bool,
    /// For a definition: the whole item, body included, when the indexer says.
    pub enclosing: Option<Range>,
}

/// Zero-based lines and columns; `end` is exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Position {
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

// scip.proto's field numbers.
const INDEX_DOCUMENTS: u32 = 2;
const DOCUMENT_RELATIVE_PATH: u32 = 1;
const DOCUMENT_OCCURRENCES: u32 = 2;
const DOCUMENT_SYMBOLS: u32 = 3;
const DOCUMENT_POSITION_ENCODING: u32 = 6;
const SYMBOL_INFORMATION_SYMBOL: u32 = 1;
const SYMBOL_INFORMATION_KIND: u32 = 5;
const OCCURRENCE_RANGE: u32 = 1;
const OCCURRENCE_SYMBOL: u32 = 2;
const OCCURRENCE_SYMBOL_ROLES: u32 = 3;
const OCCURRENCE_ENCLOSING_RANGE: u32 = 7;
const OCCURRENCE_SINGLE_LINE_RANGE: u32 = 8;
const OCCURRENCE_MULTI_LINE_RANGE: u32 = 9;
const OCCURRENCE_SINGLE_LINE_ENCLOSING: u32 = 10;
const OCCURRENCE_MULTI_LINE_ENCLOSING: u32 = 11;
const ROLE_DEFINITION: u64 = 1;
/// `UTF8CodeUnitOffsetFromLineStart`; rust-analyzer and scip-go say so, scip-typescript and
/// scip-python leave it unspecified and count UTF-16 code units, as their compilers do.
const UTF8: u64 = 1;

pub fn read(bytes: &[u8]) -> Result<Index<'_>, Malformed> {
    let mut documents = Vec::new();
    for field in Fields::new(bytes) {
        if let (INDEX_DOCUMENTS, value) = field? {
            documents.push(document(protobuf::bytes(value)?)?);
        }
    }
    Ok(Index { documents })
}

fn document(bytes: &[u8]) -> Result<Document<'_>, Malformed> {
    let mut path = "";
    let mut occurrences = Vec::new();
    let mut symbols = Vec::new();
    let mut utf16 = true;
    for field in Fields::new(bytes) {
        match field? {
            (DOCUMENT_RELATIVE_PATH, value) => path = protobuf::string(value)?,
            (DOCUMENT_OCCURRENCES, value) => occurrences.push(occurrence(protobuf::bytes(value)?)?),
            (DOCUMENT_SYMBOLS, value) => {
                symbols.push(symbol_information(protobuf::bytes(value)?)?);
            }
            (DOCUMENT_POSITION_ENCODING, Value::Varint(encoding)) => utf16 = encoding != UTF8,
            _ => {}
        }
    }
    Ok(Document {
        // `scip.proto` asks for `/` "including on Windows", but rust-analyzer writes `\` there.
        path: if cfg!(windows) {
            path.replace('\\', "/")
        } else {
            path.to_owned()
        },
        occurrences,
        symbols,
        utf16,
    })
}

fn symbol_information(bytes: &[u8]) -> Result<SymbolInformation<'_>, Malformed> {
    let mut information = SymbolInformation {
        symbol: "",
        kind: 0,
    };
    for field in Fields::new(bytes) {
        match field? {
            (SYMBOL_INFORMATION_SYMBOL, value) => information.symbol = protobuf::string(value)?,
            (SYMBOL_INFORMATION_KIND, Value::Varint(kind)) => information.kind = kind,
            _ => {}
        }
    }
    Ok(information)
}

fn occurrence(bytes: &[u8]) -> Result<Occurrence<'_>, Malformed> {
    let mut range = None;
    let mut enclosing = None;
    let mut symbol = "";
    let mut roles = 0;
    for field in Fields::new(bytes) {
        match field? {
            (OCCURRENCE_RANGE, value) => range = range.or(packed(value)?),
            (OCCURRENCE_SYMBOL, value) => symbol = protobuf::string(value)?,
            (OCCURRENCE_SYMBOL_ROLES, Value::Varint(value)) => roles = value,
            (OCCURRENCE_ENCLOSING_RANGE, value) => enclosing = enclosing.or(packed(value)?),
            (OCCURRENCE_SINGLE_LINE_RANGE, value) => range = Some(typed(value, false)?),
            (OCCURRENCE_MULTI_LINE_RANGE, value) => range = Some(typed(value, true)?),
            (OCCURRENCE_SINGLE_LINE_ENCLOSING, value) => enclosing = Some(typed(value, false)?),
            (OCCURRENCE_MULTI_LINE_ENCLOSING, value) => enclosing = Some(typed(value, true)?),
            _ => {}
        }
    }
    Ok(Occurrence {
        range: range.ok_or(Malformed("an occurrence's range"))?,
        symbol,
        definition: roles & ROLE_DEFINITION != 0,
        enclosing,
    })
}

/// The deprecated packed range: `[line, start, end]` on one line, `[start line, start, end
/// line, end]` otherwise.
fn packed(value: Value<'_>) -> Result<Option<Range>, Malformed> {
    let mut numbers = Vec::new();
    protobuf::integers(value, &mut numbers)?;
    let numbers: Option<Vec<u32>> = numbers.iter().map(|&n| u32::try_from(n).ok()).collect();
    Ok(match numbers.as_deref() {
        Some(&[line, start, end]) => Some(span(line, start, line, end)),
        Some(&[start_line, start, end_line, end]) => Some(span(start_line, start, end_line, end)),
        _ => None,
    })
}

/// `SingleLineRange { line, start_character, end_character }` or `MultiLineRange
/// { start_line, start_character, end_line, end_character }`; an absent field is zero.
fn typed(value: Value<'_>, multi: bool) -> Result<Range, Malformed> {
    let mut at = [0u32; 4];
    for field in Fields::new(protobuf::bytes(value)?) {
        if let (number @ 1..=4, Value::Varint(n)) = field? {
            at[number as usize - 1] = u32::try_from(n).map_err(|_| Malformed("a position"))?;
        }
    }
    Ok(if multi {
        span(at[0], at[1], at[2], at[3])
    } else {
        span(at[0], at[1], at[0], at[2])
    })
}

fn span(start_line: u32, start: u32, end_line: u32, end: u32) -> Range {
    Range {
        start: Position {
            line: start_line,
            column: start,
        },
        end: Position {
            line: end_line,
            column: end,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A varint field `number` of `value`.
    fn varint(number: u32, value: u64) -> Vec<u8> {
        let mut out = vec![u8::try_from(number << 3).expect("a small field number")];
        let mut rest = value;
        loop {
            let byte = u8::try_from(rest & 0x7f).expect("seven bits");
            rest >>= 7;
            if rest == 0 {
                out.push(byte);
                return out;
            }

            out.push(byte | 0x80);
        }
    }

    fn message(number: u32, body: &[u8]) -> Vec<u8> {
        let mut out = vec![u8::try_from((number << 3) | 2).expect("a small field number")];
        out.push(u8::try_from(body.len()).expect("a short message"));
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn typed_and_packed_ranges_read_the_same_and_an_absent_field_is_zero() {
        let single = [varint(2, 8), varint(3, 11)].concat();
        let multi = [varint(1, 4), varint(3, 9), varint(4, 1)].concat();
        let occurrence = [
            message(OCCURRENCE_SINGLE_LINE_RANGE, &single),
            message(OCCURRENCE_SYMBOL, b"s"),
            varint(OCCURRENCE_SYMBOL_ROLES, 1),
            message(OCCURRENCE_MULTI_LINE_ENCLOSING, &multi),
        ]
        .concat();
        let read = super::occurrence(&occurrence).expect("an occurrence");
        assert_eq!(read.range, span(0, 8, 0, 11));
        assert_eq!(read.enclosing, Some(span(4, 0, 9, 1)));
        assert!(read.definition);
        let old = [
            message(OCCURRENCE_RANGE, &[2, 5, 10]),
            message(OCCURRENCE_SYMBOL, b"s"),
            message(OCCURRENCE_ENCLOSING_RANGE, &[2, 0, 5, 1]),
        ]
        .concat();
        let read = super::occurrence(&old).expect("an occurrence");
        assert_eq!(read.range, span(2, 5, 2, 10));
        assert_eq!(read.enclosing, Some(span(2, 0, 5, 1)));
        assert!(!read.definition);
    }

    #[test]
    fn a_document_counts_utf16_unless_it_says_utf8() {
        let path = message(DOCUMENT_RELATIVE_PATH, b"src/a.rs");
        let utf8 = [path.clone(), varint(DOCUMENT_POSITION_ENCODING, UTF8)].concat();
        assert!(!document(&utf8).expect("a document").utf16);
        assert!(document(&path).expect("a document").utf16);
        assert!(super::occurrence(&message(OCCURRENCE_SYMBOL, b"s")).is_err());
    }
}
