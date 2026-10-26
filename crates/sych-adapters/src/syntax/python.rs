//! Python shapes from tree-sitter (spec §10.4, roadmap M3.2): the fallback without a
//! language server. Imports, enclosing functions and classes, `with`/`finally` scopes, async
//! tasks, writes, swallowed exceptions, and the dynamic shapes no static reading resolves:
//! `getattr`/`eval`/`import_module`, monkey patching and import-time registration.
//!
//! Python has no declared receiver types here, so a call is matched by its name and its
//! receiver's text; that is a shape, not a resolved callee.

use std::collections::BTreeSet;
use std::mem;

use sych_core::SyntaxKind;
use tree_sitter::Node;

use super::Raw;
use super::tree::{lines, named, parse, short, text};

const DYNAMIC: &[&str] = &[
    "getattr",
    "setattr",
    "delattr",
    "eval",
    "exec",
    "__import__",
    "import_module",
    "globals",
    "locals",
];
const TASKS: &[&str] = &["create_task", "ensure_future", "run_coroutine_threadsafe"];
const WRITES: &[&str] = &[
    "write",
    "writelines",
    "write_text",
    "write_bytes",
    "unlink",
    "rmtree",
    "remove",
    "rename",
    "replace",
    "makedirs",
    "mkdir",
];
const DB: &[&str] = &[
    "execute",
    "executemany",
    "executescript",
    "bulk_create",
    "bulk_update",
    "save",
];
const TX: &[&str] = &["commit", "rollback", "begin", "atomic", "begin_nested"];
const HTTP_MODULES: &[&str] = &["requests", "httpx", "aiohttp", "urllib"];
const HTTP: &[&str] = &[
    "get", "post", "put", "patch", "delete", "request", "urlopen", "head",
];
/// Decorators that only wrap or describe the function; anything else may register it.
const PLAIN_DECORATORS: &[&str] = &[
    "property",
    "staticmethod",
    "classmethod",
    "dataclass",
    "wraps",
    "cache",
    "lru_cache",
    "cached_property",
    "abstractmethod",
    "override",
    "contextmanager",
    "asynccontextmanager",
    "total_ordering",
    "fixture",
    "parametrize",
    "setter",
    "getter",
    "deleter",
    "final",
    "overload",
];
const SKIPS: &[&str] = &["skip", "skipif", "skipIf", "skipUnless", "xfail"];

struct Visitor<'s> {
    src: &'s str,
    wrappers: &'s [(String, String)],
    /// Local names bound by imports.
    imports: BTreeSet<String>,
    symbols: Vec<String>,
    guards: Vec<String>,
    out: Vec<Raw>,
}

/// `name` of `a.b.name(..)`; the receiver text `a.b`, empty for a plain call.
fn split_callee(callee: &str) -> (&str, &str) {
    callee.rsplit_once('.').unwrap_or(("", callee))
}

impl Visitor<'_> {
    fn push(&mut self, kind: SyntaxKind, node: Node<'_>, detail: String, subject: Option<String>) {
        let (start, end) = lines(node);
        self.out.push(Raw {
            kind,
            start,
            end,
            symbol: (!self.symbols.is_empty()).then(|| self.symbols.join(".")),
            detail,
            subject,
            guard: self.guards.last().cloned(),
        });
    }

    fn walk(&mut self, node: Node<'_>) {
        match node.kind() {
            "import_statement" | "import_from_statement" => self.import(node),
            "decorator" => self.decorator(node),
            "function_definition" | "class_definition" => {
                let name = node
                    .child_by_field_name("name")
                    .map_or("", |n| text(n, self.src));
                self.symbols.push(name.to_owned());
                let guards = mem::take(&mut self.guards);
                for child in named(node) {
                    self.walk(child);
                }
                self.guards = guards;
                self.symbols.pop();
                return;
            }
            "with_statement" => {
                let head = named(node)
                    .into_iter()
                    .find(|n| n.kind() == "with_clause")
                    .map_or("", |c| text(c, self.src));
                self.push(
                    SyntaxKind::ResourceScope,
                    node,
                    format!("with {}", short(head)),
                    None,
                );
            }
            "try_statement" => {
                if named(node).iter().any(|n| n.kind() == "finally_clause") {
                    self.push(SyntaxKind::ResourceScope, node, "try/finally".into(), None);
                }
            }
            "except_clause" => self.except(node),
            "if_statement" => return self.branch(node),
            "assignment" => self.assignment(node),
            "call" => self.call(node),
            _ => {}
        }
        for child in named(node) {
            self.walk(child);
        }
    }

    fn import(&mut self, node: Node<'_>) {
        let module = node
            .child_by_field_name("module_name")
            .map(|m| text(m, self.src).to_owned());
        let mut cursor = node.walk();
        let names: Vec<Node<'_>> = node.children_by_field_name("name", &mut cursor).collect();
        for name in &names {
            let (dotted, alias) = match name.kind() {
                "aliased_import" => (
                    name.child_by_field_name("name")
                        .map_or("", |n| text(n, self.src)),
                    name.child_by_field_name("alias").map(|a| text(a, self.src)),
                ),
                _ => (text(*name, self.src), None),
            };
            let local = alias.unwrap_or_else(|| dotted.split('.').next().unwrap_or(dotted));
            self.imports.insert(local.to_owned());
            if module.is_none() {
                self.push(SyntaxKind::Import, node, dotted.to_owned(), None);
            }
        }
        if let Some(module) = module {
            let star = named(node).iter().any(|n| n.kind() == "wildcard_import");
            let detail = if star {
                format!("{module} (*)")
            } else {
                module
            };
            self.push(SyntaxKind::Import, node, detail, None);
        }
    }

    fn decorator(&mut self, node: Node<'_>) {
        let expr = text(node, self.src).trim_start_matches('@');
        let callee = expr.split('(').next().unwrap_or(expr);
        let (_, name) = split_callee(callee);
        if SKIPS.contains(&name) {
            self.push(SyntaxKind::TestSkipped, node, short(expr), None);
        } else if !PLAIN_DECORATORS.contains(&name) && !callee.starts_with("pytest.mark.") {
            self.push(SyntaxKind::RuntimeRegistration, node, short(expr), None);
        }
    }

    /// An `except` whose body only passes, continues, breaks or returns nothing.
    fn except(&mut self, node: Node<'_>) {
        let Some(block) = named(node).into_iter().find(|n| n.kind() == "block") else {
            return;
        };
        let swallows = named(block).iter().all(|s| {
            matches!(
                s.kind(),
                "pass_statement" | "continue_statement" | "break_statement"
            ) || (s.kind() == "return_statement"
                && matches!(text(*s, self.src), "return" | "return None"))
        });
        if swallows {
            let head = text(node, self.src).lines().next().unwrap_or_default();
            self.push(SyntaxKind::SwallowedException, node, short(head), None);
        }
    }

    fn branch(&mut self, node: Node<'_>) {
        let condition = node
            .child_by_field_name("condition")
            .map(|c| short(text(c, self.src)))
            .unwrap_or_default();
        if let Some(c) = node.child_by_field_name("condition") {
            self.walk(c);
        }

        if let Some(body) = node.child_by_field_name("consequence") {
            self.guards.push(condition.clone());
            self.walk(body);
            self.guards.pop();
        }

        let mut cursor = node.walk();
        let alternatives: Vec<Node<'_>> = node
            .children_by_field_name("alternative", &mut cursor)
            .collect();
        self.guards.push(format!("not ({condition})"));
        for alternative in alternatives {
            self.walk(alternative);
        }
        self.guards.pop();
    }

    /// `module.attr = value` replaces an imported module's attribute for every caller.
    fn assignment(&mut self, node: Node<'_>) {
        let Some(left) = node.child_by_field_name("left") else {
            return;
        };
        if left.kind() == "attribute"
            && let Some(object) = left.child_by_field_name("object")
            && self.imports.contains(text(object, self.src))
        {
            let detail = short(text(node, self.src));
            self.push(SyntaxKind::MonkeyPatch, node, detail, None);
        }
    }

    fn call(&mut self, node: Node<'_>) {
        let Some(function) = node.child_by_field_name("function") else {
            return;
        };
        let callee = text(function, self.src);
        let (receiver, name) = split_callee(callee);
        let root = callee.split('.').next().unwrap_or(callee);
        let detail = short(text(node, self.src));
        let discarded = node
            .parent()
            .is_some_and(|p| p.kind() == "expression_statement");
        let args = node
            .child_by_field_name("arguments")
            .map_or("", |a| text(a, self.src));
        let kind = if DYNAMIC.contains(&name) {
            let first = args
                .trim_start_matches('(')
                .split([',', ')'])
                .next()
                .unwrap_or("");
            if name == "setattr" && self.imports.contains(first.trim()) {
                Some(SyntaxKind::MonkeyPatch)
            } else {
                Some(SyntaxKind::DynamicDispatch)
            }
        } else if TASKS.contains(&name) {
            Some(if discarded {
                SyntaxKind::DetachedSpawn
            } else {
                SyntaxKind::AsyncTask
            })
        } else if name == "start" && receiver.ends_with(')') && receiver.contains("Thread(") {
            Some(SyntaxKind::DetachedSpawn)
        } else if name == "skip" && root == "pytest" {
            Some(SyntaxKind::TestSkipped)
        } else if (name == "open" && receiver.is_empty() && opens_for_write(args))
            || (WRITES.contains(&name) && !receiver.is_empty())
        {
            Some(SyntaxKind::WriteCall)
        } else if TX.contains(&name) && !receiver.is_empty() {
            Some(SyntaxKind::TxBoundary)
        } else if DB.contains(&name)
            || (["add", "delete", "merge", "flush"].contains(&name)
                && receiver.to_lowercase().contains("session"))
            || callee.contains(".objects.")
        {
            Some(SyntaxKind::DbCall)
        } else if HTTP.contains(&name)
            && (HTTP_MODULES.contains(&root) && self.imports.contains(root)
                || ["client", "session"]
                    .iter()
                    .any(|s| receiver.to_lowercase().ends_with(s)))
        {
            Some(SyntaxKind::NetworkCall)
        } else if ["register", "connect", "subscribe"].contains(&name) && self.symbols.is_empty() {
            Some(SyntaxKind::RuntimeRegistration)
        } else {
            None
        };
        if let Some(kind) = kind {
            let subject = (!receiver.is_empty()).then(|| receiver.to_owned());
            self.push(kind, node, detail, subject);
        }

        if let Some((_, symbol)) = self.wrappers.iter().find(|(call, _)| call == name) {
            let symbol = symbol.clone();
            self.push(SyntaxKind::RegisteredEffectCall, node, symbol, None);
        }
    }
}

/// `open(path, "w")`, `open(path, mode="a")`: any mode that writes, appends or creates.
fn opens_for_write(args: &str) -> bool {
    args.split(',')
        .skip(1)
        .map(|a| a.trim().trim_start_matches("mode=").trim_end_matches(')'))
        .any(|a| {
            a.starts_with(['"', '\'']) && a.trim_matches(['"', '\'']).contains(['w', 'a', 'x', '+'])
        })
}

/// Facts of one Python file.
pub fn facts(source: &str, wrappers: &[(String, String)]) -> Result<Vec<Raw>, String> {
    let tree = parse(source, &tree_sitter_python::LANGUAGE.into())?;
    let mut visitor = Visitor {
        src: source,
        wrappers,
        imports: BTreeSet::new(),
        symbols: Vec::new(),
        guards: Vec::new(),
        out: Vec::new(),
    };
    visitor.walk(tree.root_node());
    Ok(visitor.out)
}

#[cfg(test)]
mod tests;
