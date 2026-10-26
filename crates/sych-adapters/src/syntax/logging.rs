//! Values Rust logging macros format with `Debug`, and the parameter types that name them.

use proc_macro2::{TokenStream, TokenTree};
use syn::{FnArg, Pat, Type};

use super::rust::last_segment;

/// Logging macros of `tracing` and `log` whose fields may carry `?value`.
pub(super) const LOG_MACROS: &[&str] = &["trace", "debug", "info", "warn", "error", "event"];

/// Standard types whose `Debug` output is known: logging them hides nothing.
pub(super) fn std_debug(ty: &str) -> bool {
    matches!(
        ty,
        "u8" | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "f32"
            | "f64"
            | "bool"
            | "char"
            | "str"
            | "String"
            | "Duration"
            | "Instant"
            | "Path"
            | "PathBuf"
    )
}

/// `name: Type` of a typed parameter, references and paths reduced to the type's name.
pub(super) fn param(arg: &FnArg) -> Option<(String, String)> {
    let FnArg::Typed(typed) = arg else {
        return None;
    };
    let Pat::Ident(ident) = &*typed.pat else {
        return None;
    };
    let mut ty = &*typed.ty;
    while let Type::Reference(r) = ty {
        ty = &r.elem;
    }
    let Type::Path(path) = ty else {
        return None;
    };
    Some((ident.ident.to_string(), last_segment(&path.path)))
}

/// Values a logging macro formats with `Debug`: the root name after `?` in a field, the name
/// in `{name:?}`, `None` for a positional `{:?}` whose argument is not matched here.
pub(super) fn debug_values(tokens: TokenStream) -> Vec<Option<String>> {
    let mut out = Vec::new();
    let mut after_question = false;
    for tree in tokens {
        match tree {
            TokenTree::Punct(p) if p.as_char() == '?' => after_question = true,
            TokenTree::Ident(i) if after_question => {
                out.push(Some(i.to_string()));
                after_question = false;
            }
            TokenTree::Literal(lit) => {
                after_question = false;
                let lit = lit.to_string();
                let mut rest = lit.as_str();
                while let Some(at) = rest.find(":?}") {
                    let head = &rest[..at];
                    let name = head.rsplit('{').next().unwrap_or_default();
                    out.push((!name.is_empty() && !name.contains(' ')).then(|| name.to_owned()));
                    rest = &rest[at + 3..];
                }
            }
            _ => after_question = false,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use sych_core::SyntaxKind;

    use super::super::rust::facts;

    fn debug(source: &str) -> Vec<(u32, Option<String>)> {
        facts(source, &[])
            .expect("parses")
            .into_iter()
            .filter(|r| r.kind == SyntaxKind::DebugFormatted)
            .map(|r| (r.start, r.subject))
            .collect()
    }

    #[test]
    fn debug_logged_values_carry_their_parameter_type() {
        let source = "fn send(request: &Request, n: u8) {\n    tracing::debug!(request = ?request, \"go\");\n    tracing::info!(n = %n, len = request.body.len());\n    log::warn!(\"{n:?} {:?}\", x);\n}\n";
        // `{n:?}` of a `u8` hides nothing; the positional `{:?}` is not matched to `x`.
        assert_eq!(debug(source), [(2, Some("Request".into())), (4, None)]);
        let kinds: Vec<_> = facts(source, &[])
            .expect("parses")
            .into_iter()
            .map(|r| r.kind)
            .collect();
        assert!(
            !kinds.contains(&SyntaxKind::UnexpandedMacro),
            "logging macros are known: {kinds:?}"
        );
    }
}
