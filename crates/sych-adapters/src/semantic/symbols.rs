//! Picks the changed constructs out of a `documentSymbol` answer.

use serde_json::Value;

/// Type names a construct uses, by the Rust and TS convention of an upper-case first letter:
/// each with the 0-based position of its first use, in the negotiated encoding (`utf-8`
/// counts bytes, otherwise UTF-16 units). The construct's own name and prelude variants are
/// skipped; at most `limit` names.
pub fn type_names(
    text: &str,
    range: &Value,
    own: &str,
    utf8: bool,
    limit: usize,
) -> Vec<(String, Value)> {
    let line = |p: &Value| usize::try_from(p["line"].as_u64().unwrap_or(0)).unwrap_or(usize::MAX);
    let (first, last) = (line(&range["start"]), line(&range["end"]));
    let mut found: Vec<(String, Value)> = Vec::new();
    for (number, source) in text.lines().enumerate().take(last + 1).skip(first) {
        let mut rest = source;
        while let Some(start) = rest.find(|c: char| c.is_ascii_uppercase()) {
            let prefix_ok = rest[..start]
                .chars()
                .next_back()
                .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            let word: String = rest[start..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            let offset = source.len() - rest.len() + start;
            rest = &rest[start + word.len()..];
            let prelude = ["Self", "Some", "None", "Ok", "Err"].contains(&word.as_str());
            if !prefix_ok || prelude || word == own || found.iter().any(|(w, _)| *w == word) {
                continue;
            }

            let column = if utf8 {
                offset
            } else {
                source[..offset].encode_utf16().count()
            };
            found.push((
                word,
                serde_json::json!({"line": number, "character": column}),
            ));
            if found.len() == limit {
                return found;
            }
        }
    }
    found
}

/// LSP 0-based `Range` to 1-based inclusive lines; an end at column 0 excludes that line.
pub fn lines(range: &Value) -> (u32, u32) {
    let line = |p: &Value| u32::try_from(p["line"].as_u64().unwrap_or(0)).unwrap_or(u32::MAX);
    let start = line(&range["start"]);
    let end = line(&range["end"]);
    let end = if range["end"]["character"] == 0 && end > start {
        end
    } else {
        end.saturating_add(1)
    };
    (start.saturating_add(1), end.max(start.saturating_add(1)))
}

pub struct Changed {
    pub name: String,
    pub kind: String,
    pub container: Vec<String>,
    pub range: Value,
    pub position: Value,
    /// Functions, methods, constructors and module-level variables (arrow functions).
    pub callable: bool,
}

const SYMBOL_KINDS: [&str; 26] = [
    "file",
    "module",
    "namespace",
    "package",
    "class",
    "method",
    "property",
    "field",
    "constructor",
    "enum",
    "interface",
    "function",
    "variable",
    "constant",
    "string",
    "number",
    "boolean",
    "array",
    "object",
    "key",
    "null",
    "enum_member",
    "struct",
    "event",
    "operator",
    "type_parameter",
];

/// Constructs worth a neighbourhood query. Module-level variables and constants count
/// because React components and hooks are usually `const` arrow functions.
fn selectable(kind: u64, top_level: bool) -> bool {
    matches!(kind, 5 | 6 | 9 | 10 | 11 | 12 | 23) || (top_level && matches!(kind, 13 | 14))
}

/// The innermost selectable symbols whose range intersects a changed line range.
pub fn changed_symbols(symbols: &Value, changed: &[(u32, u32)]) -> Vec<Changed> {
    fn visit(
        symbol: &Value,
        container: &[String],
        changed: &[(u32, u32)],
        out: &mut Vec<Changed>,
    ) -> bool {
        // `SymbolInformation` has no children or selection range; its location range is used.
        let range = symbol.get("range").unwrap_or(&symbol["location"]["range"]);
        let (start, end) = lines(range);
        let hit = changed.iter().any(|&(s, e)| s <= end && start <= e);
        let name = symbol["name"].as_str().unwrap_or_default().to_owned();
        let mut inner = container.to_vec();
        inner.push(name.clone());
        let mut child_hit = false;
        for child in symbol["children"].as_array().into_iter().flatten() {
            child_hit |= visit(child, &inner, changed, out);
        }
        let kind = symbol["kind"].as_u64().unwrap_or(0);
        // TypeScript names anonymous functions `<call> callback`; they have no references, so
        // the change is attributed to the nearest named construct instead.
        let anonymous = name.ends_with(" callback") || name.starts_with('<');
        // Import and re-export bindings are reported as variables whose range is just the name.
        let binding = matches!(kind, 13 | 14) && symbol.get("selectionRange") == Some(range);
        let mine = hit && !anonymous && !binding && selectable(kind, container.is_empty());
        if mine && !child_hit {
            let selection = symbol.get("selectionRange").unwrap_or(range);
            out.push(Changed {
                name,
                kind: usize::try_from(kind)
                    .ok()
                    .and_then(|k| SYMBOL_KINDS.get(k.wrapping_sub(1)))
                    .map_or_else(|| kind.to_string(), |k| (*k).to_owned()),
                container: container.to_vec(),
                range: range.clone(),
                position: selection["start"].clone(),
                callable: matches!(kind, 6 | 9 | 12 | 13),
            });
        }

        mine || child_hit
    }
    let mut out = Vec::new();
    for symbol in symbols.as_array().into_iter().flatten() {
        visit(symbol, &[], changed, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn range(start: u64, end: u64, end_char: u64) -> Value {
        json!({"start": {"line": start, "character": 0}, "end": {"line": end, "character": end_char}})
    }

    #[test]
    fn type_names_are_found_once_with_their_position() {
        let text =
            "fn price(x: Money) -> Total {\n    let цена = Money::new(x);\n    Tax::of(цена)\n}\n";
        let names = type_names(text, &range(0, 3, 1), "price", false, 5);
        let found: Vec<_> = names.iter().map(|(n, p)| (n.as_str(), p.clone())).collect();
        assert_eq!(
            found,
            [
                ("Money", json!({"line": 0, "character": 12})),
                ("Total", json!({"line": 0, "character": 22})),
                ("Tax", json!({"line": 2, "character": 4})),
            ]
        );
        let utf8 = type_names("  let ц = Tax;", &range(0, 0, 9), "x", true, 5);
        assert_eq!(
            utf8[0].1,
            json!({"line": 0, "character": 11}),
            "bytes, not chars"
        );
        assert_eq!(
            type_names(text, &range(0, 3, 1), "price", false, 1).len(),
            1
        );
    }

    #[test]
    fn lsp_ranges_become_inclusive_lines() {
        assert_eq!(lines(&range(0, 0, 5)), (1, 1));
        assert_eq!(lines(&range(2, 4, 1)), (3, 5));
        assert_eq!(lines(&range(2, 4, 0)), (3, 4));
    }

    #[test]
    fn innermost_changed_construct_is_selected() {
        let symbols = json!([
            {"name": "spawn", "kind": 13, "range": range(3, 3, 9), "selectionRange": range(3, 3, 9)},
            {"name": "Store", "kind": 23, "range": range(0, 2, 1), "selectionRange": range(0, 0, 1)},
            {"name": "impl Store", "kind": 19, "range": range(4, 20, 1), "selectionRange": range(4, 4, 1),
             "children": [
                {"name": "open", "kind": 6, "range": range(5, 9, 1), "selectionRange": range(5, 5, 1)},
                {"name": "close", "kind": 6, "range": range(11, 19, 1), "selectionRange": range(11, 11, 1),
                 "children": [{"name": "local", "kind": 13, "range": range(12, 12, 1)}]}
             ]},
            {"name": "useOrders", "kind": 13, "range": range(22, 30, 1), "selectionRange": range(22, 22, 1),
             "children": [{"name": "useEffect() callback", "kind": 12, "range": range(24, 26, 1)}]}
        ]);
        let picked = changed_symbols(&symbols, &[(4, 4), (13, 13), (25, 25)]);
        let names: Vec<_> = picked
            .iter()
            .map(|c| (c.name.as_str(), c.kind.as_str()))
            .collect();
        assert_eq!(names, [("close", "method"), ("useOrders", "variable")]);
        assert_eq!(picked[0].container, ["impl Store"]);
    }
}
