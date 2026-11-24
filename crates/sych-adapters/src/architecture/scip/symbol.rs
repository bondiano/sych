//! SCIP's symbol strings: `<scheme> <manager> <package> <version> <descriptors>`, for example
//! `rust-analyzer cargo my-crate 0.1.0 kinds/epoll/Ready#`; a descriptor that is no plain name
//! is quoted in backticks. Ported from deslop-please (`f678d10`, MIT; see
//! `sych-core/src/architecture/UPSTREAM-LICENSE`), with Rust's `impl` blocks read apart from the
//! other languages' containers.

use std::borrow::Cow;
use std::fmt;

#[derive(Debug, PartialEq)]
pub struct Symbol<'a> {
    pub package: Cow<'a, str>,
    pub version: Cow<'a, str>,
    pub descriptors: Vec<Descriptor<'a>>,
    text: &'a str,
}

#[derive(Debug, PartialEq)]
pub struct Descriptor<'a> {
    pub name: Cow<'a, str>,
    pub kind: Kind,
    /// Where it ends in the symbol's text.
    end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Namespace,
    Type,
    Term,
    Method,
    TypeParameter,
    Parameter,
    Meta,
    Macro,
}

#[derive(Debug)]
pub struct BadSymbol(pub String);

impl fmt::Display for BadSymbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a SCIP symbol that cannot be read: {:?}", self.0)
    }
}

impl<'a> Symbol<'a> {
    /// `None` for a local symbol: one that only its own document can name.
    pub fn parse(text: &'a str) -> Result<Option<Symbol<'a>>, BadSymbol> {
        if text.starts_with("local ") {
            return Ok(None);
        }

        let bad = || BadSymbol(text.to_owned());
        let (_scheme, rest) = space_separated(text).ok_or_else(bad)?;
        let (_manager, rest) = space_separated(rest).ok_or_else(bad)?;
        let (package, rest) = space_separated(rest).ok_or_else(bad)?;
        let (version, rest) = space_separated(rest).ok_or_else(bad)?;
        let descriptors = descriptors(text, rest).ok_or_else(bad)?;
        Ok(Some(Symbol {
            package,
            version,
            descriptors,
            text,
        }))
    }

    /// The leading namespaces: the module the symbol is in, or is.
    pub fn namespaces(&self) -> Vec<&str> {
        self.descriptors
            .iter()
            .take_while(|d| d.kind == Kind::Namespace)
            .map(|d| d.name.as_ref())
            .collect()
    }

    /// The descriptors past the namespaces: the item inside its module.
    pub fn inner(&self) -> &[Descriptor<'a>] {
        let first = self
            .descriptors
            .iter()
            .position(|d| d.kind != Kind::Namespace)
            .unwrap_or(self.descriptors.len());
        &self.descriptors[first..]
    }

    /// The symbol of the item it is part of: `m/Ready#` for `m/Ready#field.`.
    pub fn container(&self) -> Option<&'a str> {
        match self.inner() {
            [.., container, _] => Some(&self.text[..container.end]),
            _ => None,
        }
    }

    /// For an item of a Rust `impl` block: the type it is for, as the index writes it
    /// (`Ready`, `&'a Tuple`), and whether the block implements a trait. `None` for the block
    /// itself and for everything else.
    pub fn impl_of(&self) -> Option<(&str, bool)> {
        match self.inner() {
            [block, ty, more @ ..] if block.kind == Kind::Type && block.name == "impl" => {
                let traits = more
                    .iter()
                    .take_while(|d| d.kind == Kind::TypeParameter)
                    .count();
                (traits < more.len()).then_some((ty.name.as_ref(), traits > 0))
            }
            _ => None,
        }
    }

    /// The `impl` block itself, which code names as `Self`.
    pub fn is_impl_block(&self) -> bool {
        matches!(self.inner(), [block, ty, rest @ ..]
            if block.kind == Kind::Type && block.name == "impl" && ty.kind == Kind::TypeParameter
                && rest.iter().all(|d| d.kind == Kind::TypeParameter))
    }

    /// How a reader writes the symbol inside its module: `Ready`, `Ready::new`,
    /// `<Ready as Display>::fmt`; a TypeScript accessor without its `<get>` mark.
    pub fn item_name(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut rest = self.inner().iter().peekable();
        while let Some(descriptor) = rest.next() {
            if descriptor.kind == Kind::Type && descriptor.name == "impl" {
                let mut types = Vec::new();
                while let Some(parameter) = rest.next_if(|d| d.kind == Kind::TypeParameter) {
                    types.push(parameter.name.as_ref());
                }
                parts.push(match types.as_slice() {
                    [ty, interface] => format!("<{ty} as {interface}>"),
                    _ => types.join(""),
                });
            } else if descriptor.kind == Kind::Macro {
                parts.push(format!("{}!", descriptor.name));
            } else {
                let name = descriptor.name.as_ref();
                let name = name
                    .strip_prefix("<get>")
                    .or_else(|| name.strip_prefix("<set>"))
                    .unwrap_or(name);
                parts.push(name.to_owned());
            }
        }
        parts.join("::")
    }

    /// What its last descriptor says the symbol is; a module's is a namespace.
    pub fn kind(&self) -> Kind {
        self.descriptors.last().map_or(Kind::Namespace, |d| d.kind)
    }
}

/// A space ends a field; two spaces are one space inside it.
fn space_separated(text: &str) -> Option<(Cow<'_, str>, &str)> {
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut escaped = false;
    while i < bytes.len() {
        if bytes[i] == b' ' {
            if bytes.get(i + 1) == Some(&b' ') {
                escaped = true;
                i += 2;
                continue;
            }

            let field = &text[..i];
            let field = if escaped {
                Cow::Owned(field.replace("  ", " "))
            } else {
                Cow::Borrowed(field)
            };
            return Some((field, &text[i + 1..]));
        }

        i += 1;
    }
    None
}

/// The descriptors `rest`, the end of `symbol`, writes.
fn descriptors<'a>(symbol: &str, mut rest: &'a str) -> Option<Vec<Descriptor<'a>>> {
    let mut parsed = Vec::new();
    while !rest.is_empty() {
        let (name, kind, after) = descriptor(rest)?;
        rest = after;
        let end = symbol.len() - rest.len();
        parsed.push(Descriptor { name, kind, end });
    }
    (!parsed.is_empty()).then_some(parsed)
}

fn descriptor(text: &str) -> Option<(Cow<'_, str>, Kind, &str)> {
    let enclosed = |open: &str, close: char, kind: Kind| {
        let (name, rest) = name(text.strip_prefix(open)?)?;
        Some((name, kind, rest.strip_prefix(close)?))
    };
    if text.starts_with('[') {
        return enclosed("[", ']', Kind::TypeParameter);
    }

    if text.starts_with('(') {
        return enclosed("(", ')', Kind::Parameter);
    }

    let (name, rest) = name(text)?;
    let mut chars = rest.chars();
    let kind = match chars.next()? {
        '/' => Kind::Namespace,
        '#' => Kind::Type,
        '.' => Kind::Term,
        ':' => Kind::Meta,
        '!' => Kind::Macro,
        '(' => {
            let close = rest.find(").")?;
            return Some((name, Kind::Method, &rest[close + 2..]));
        }
        _ => return None,
    };
    Some((name, kind, chars.as_str()))
}

fn name(text: &str) -> Option<(Cow<'_, str>, &str)> {
    if let Some(quoted) = text.strip_prefix('`') {
        let mut end = 0;
        loop {
            end += quoted[end..].find('`')?;
            if quoted[end + 1..].starts_with('`') {
                end += 2;
            } else {
                break;
            }
        }
        let raw = &quoted[..end];
        let name = if raw.contains("``") {
            Cow::Owned(raw.replace("``", "`"))
        } else {
            Cow::Borrowed(raw)
        };
        return Some((name, &quoted[end + 1..]));
    }

    let end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-' | '$')))
        .unwrap_or(text.len());
    (end > 0).then(|| (Cow::Borrowed(&text[..end]), &text[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Symbol<'_> {
        Symbol::parse(text).expect("a symbol").expect("not a local")
    }

    #[test]
    fn a_module_symbol_gives_its_package_and_path() {
        let symbol = parse("rust-analyzer cargo my-crate 0.1.0 lib/kinds/epoll/");
        assert_eq!(symbol.package, "my-crate");
        assert_eq!(symbol.version, "0.1.0");
        assert_eq!(symbol.namespaces(), ["lib", "kinds", "epoll"]);
        assert_eq!(symbol.inner(), []);
        let go = parse("scip-go gomod example.com/fx . `example.com/fx/store`/Store#Push().");
        assert_eq!(go.package, "example.com/fx");
        assert_eq!(go.namespaces(), ["example.com/fx/store"]);
        assert_eq!(go.item_name(), "Store::Push");
        assert_eq!(
            go.container(),
            Some("scip-go gomod example.com/fx . `example.com/fx/store`/Store#")
        );
    }

    #[test]
    fn rust_impl_blocks_say_their_type_and_whether_a_trait_is_implemented() {
        let method = parse("rust-analyzer cargo a 0.1.0 lib/kinds/impl#[Ready]new().");
        assert_eq!(method.impl_of(), Some(("Ready", false)));
        assert_eq!(method.item_name(), "Ready::new");
        let fmt = parse("rust-analyzer cargo a 0.1.0 lib/impl#[`&'a Ready<T>`][Display]fmt().");
        assert_eq!(fmt.impl_of(), Some(("&'a Ready<T>", true)));
        assert_eq!(fmt.item_name(), "<&'a Ready<T> as Display>::fmt");
        assert!(parse("rust-analyzer cargo a 0.1.0 lib/impl#[Ready]").is_impl_block());
        assert!(!method.is_impl_block());
        assert_eq!(
            parse("rust-analyzer cargo a 0.1.0 lib/Ready#x.").impl_of(),
            None
        );
    }

    #[test]
    fn escaped_names_accessors_and_spaces() {
        let quoted = parse("rust-analyzer cargo a 0.1.0 `a``b`#");
        assert_eq!(quoted.descriptors[0].name, "a`b");
        let getter = parse("scip-typescript npm fx 1.0.0 src/`store.ts`/Store#`<get>all`().");
        assert_eq!(getter.namespaces(), ["src", "store.ts"]);
        assert_eq!(getter.item_name(), "Store::all");
        assert_eq!(
            parse("rust-analyzer cargo my  crate 1 m/").package,
            "my crate"
        );
        let python = parse("scip-python python fx 0 `pkg.store`/__init__:");
        assert_eq!(python.namespaces(), ["pkg.store"]);
        assert_eq!(python.kind(), Kind::Meta);
    }

    #[test]
    fn locals_are_not_symbols_and_garbage_is_an_error() {
        assert!(Symbol::parse("local 12").expect("a local").is_none());
        assert!(Symbol::parse("rust-analyzer cargo a 0.1.0 ").is_err());
        assert!(Symbol::parse("rust-analyzer cargo a 0.1.0 x%").is_err());
    }
}
