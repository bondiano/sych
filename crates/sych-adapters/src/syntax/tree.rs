//! Shared tree-sitter plumbing of the Go and Python shapes: parsing, node text and lines.

use tree_sitter::{Language, Node, Parser, Tree};

const MAX_DETAIL: usize = 160;

/// The tree of `source`; a file with a syntax error is refused, as `syn` and `oxc` refuse it.
pub(super) fn parse(source: &str, language: &Language) -> Result<Tree, String> {
    let mut parser = Parser::new();
    parser.set_language(language).map_err(|e| e.to_string())?;
    let tree = parser.parse(source, None).ok_or("parse failed")?;
    let root = tree.root_node();
    if root.has_error() {
        let mut cursor = root.walk();
        let mut at = root;
        // Descend to the first error so the message names its line.
        while let Some(child) = at
            .children(&mut cursor)
            .find(|c| c.is_error() || c.is_missing() || c.has_error())
        {
            at = child;
            if at.is_error() || at.is_missing() {
                break;
            }
        }
        return Err(format!(
            "syntax error at line {}",
            at.start_position().row + 1
        ));
    }

    Ok(tree)
}

pub(super) fn text<'s>(node: Node<'_>, src: &'s str) -> &'s str {
    src.get(node.byte_range()).unwrap_or_default()
}

/// 1-based inclusive lines of `node`.
pub(super) fn lines(node: Node<'_>) -> (u32, u32) {
    let line = |row: usize| u32::try_from(row + 1).unwrap_or(u32::MAX);
    (
        line(node.start_position().row),
        line(node.end_position().row),
    )
}

/// One-line detail of repository text, whitespace collapsed and capped.
pub(super) fn short(s: &str) -> String {
    let mut out = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.len() > MAX_DETAIL {
        let cut = (0..=MAX_DETAIL)
            .rev()
            .find(|&i| out.is_char_boundary(i))
            .unwrap_or(0);
        out.truncate(cut);
        out.push('…');
    }

    out
}

/// Named children of `node`, in order.
pub(super) fn named(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}
