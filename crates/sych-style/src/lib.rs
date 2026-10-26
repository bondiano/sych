//! House style rules that neither rustfmt nor clippy express.
//!
//! `blank-line-after-if`: an `if` statement (with its `else` chain) that is followed by another
//! statement of the same block is separated from it by an empty line. The rule reads the syntax
//! tree, so `if` inside expressions, closing braces and `else` branches are not affected.

use std::path::{Path, PathBuf};
use std::{fs, io};

use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Block, Expr, Stmt};

/// A violation: 1-based line of the closing brace of the `if` statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub path: PathBuf,
    pub line: usize,
}

struct IfEnds<'s> {
    lines: &'s [&'s str],
    found: Vec<usize>,
}

impl<'ast> Visit<'ast> for IfEnds<'_> {
    fn visit_block(&mut self, block: &'ast Block) {
        for pair in block.stmts.windows(2) {
            if let Stmt::Expr(Expr::If(stmt), _) = &pair[0] {
                let end = stmt.span().end().line;
                // `lines` is 0-based, so `lines[end]` is the line after the closing brace.
                let next_blank = self.lines.get(end).is_none_or(|l| l.trim().is_empty());
                let same_line = pair[1].span().start().line == end;
                if !next_blank && !same_line {
                    self.found.push(end);
                }
            }
        }

        visit::visit_block(self, block);
    }
}

/// Lines (1-based) that end an `if` statement without a following blank line.
pub fn check_source(source: &str) -> Result<Vec<usize>, syn::Error> {
    let file = syn::parse_file(source)?;
    let lines: Vec<&str> = source.lines().collect();
    let mut visitor = IfEnds {
        lines: &lines,
        found: Vec::new(),
    };
    visitor.visit_file(&file);
    visitor.found.sort_unstable();
    visitor.found.dedup();
    Ok(visitor.found)
}

/// `source` with an empty line inserted after every reported line.
pub fn fix_source(source: &str, lines: &[usize]) -> String {
    let mut out = String::with_capacity(source.len() + lines.len());
    for (i, line) in source.split_inclusive('\n').enumerate() {
        out.push_str(line);
        if lines.contains(&(i + 1)) {
            out.push('\n');
        }
    }
    out
}

fn rust_files(root: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    if root.is_file() {
        out.push(root.to_path_buf());
        return Ok(());
    }

    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if path.is_dir() && !matches!(name, "target" | ".git") {
            rust_files(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }

    Ok(())
}

/// Checks (or, with `fix`, rewrites) every `.rs` file under `roots`.
pub fn run(roots: &[PathBuf], fix: bool) -> Result<Vec<Violation>, String> {
    let mut files = Vec::new();
    for root in roots {
        rust_files(root, &mut files).map_err(|e| format!("{}: {e}", root.display()))?;
    }

    files.sort();
    let mut violations = Vec::new();
    for path in files {
        let source = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let lines = check_source(&source).map_err(|e| format!("{}: {e}", path.display()))?;
        if fix && !lines.is_empty() {
            fs::write(&path, fix_source(&source, &lines))
                .map_err(|e| format!("{}: {e}", path.display()))?;
        } else {
            violations.extend(lines.into_iter().map(|line| Violation {
                path: path.clone(),
                line,
            }));
        }
    }

    Ok(violations)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn if_followed_by_a_statement_needs_a_blank_line() {
        let source = "fn f(a: bool) {\n    if a {\n        g();\n    }\n    h();\n}\n";
        assert_eq!(check_source(source).expect("parses"), [4]);
        let fixed = fix_source(source, &[4]);
        assert!(check_source(&fixed).expect("parses").is_empty());
        assert!(fixed.contains("    }\n\n    h();"));
    }

    #[test]
    fn else_chains_last_statements_and_expressions_are_fine() {
        let ok = [
            "fn f(a: bool) {\n    if a {\n        g();\n    } else if b {\n        h();\n    }\n}\n",
            "fn f(a: bool) -> u8 {\n    let x = if a { 1 } else { 2 };\n    x\n}\n",
            "fn f(a: bool) {\n    if a {\n        g();\n    }\n\n    h();\n}\n",
            "fn f(a: bool) {\n    if let Some(x) = a {\n        g(x);\n    }\n}\n",
        ];
        for source in ok {
            assert!(check_source(source).expect("parses").is_empty(), "{source}");
        }
        let nested = "fn f() {\n    for x in xs {\n        if x {\n            g();\n        }\n        h();\n    }\n}\n";
        assert_eq!(check_source(nested).expect("parses"), [5]);
    }
}
