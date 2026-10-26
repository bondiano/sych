//! Go shapes from tree-sitter (spec §10.3, roadmap M3.1): receivers, `context.Context`
//! parameters, returned errors, `defer`, goroutines, channels and transaction boundaries.
//!
//! A SQL call's receiver type comes from the function's parameters and locals and the
//! package's struct fields, never from the SQL text, so `tx.ExecContext` and
//! `s.db.ExecContext` stay apart. A receiver the package does not show stays unresolved, a
//! call through an interface the package declares is `InterfaceCall` and `reflect` is
//! `DynamicDispatch`.

use std::collections::{BTreeMap, BTreeSet};
use std::mem;

use sych_core::SyntaxKind;
use tree_sitter::Node;

use super::Raw;
use super::tree::{lines, named, parse, short, text};

const DB_CALLS: &[&str] = &[
    "Exec",
    "ExecContext",
    "Query",
    "QueryContext",
    "QueryRow",
    "QueryRowContext",
    "Prepare",
    "PrepareContext",
    "NamedExec",
    "NamedExecContext",
    "MustExec",
];
const TX_BEGIN: &[&str] = &["Begin", "BeginTx", "Beginx", "BeginTxx"];
const TX_END: &[&str] = &["Commit", "Rollback"];
const OS_WRITES: &[&str] = &[
    "WriteFile",
    "Create",
    "OpenFile",
    "Remove",
    "RemoveAll",
    "Rename",
    "Mkdir",
    "MkdirAll",
];
const HTTP_CALLS: &[&str] = &[
    "Get",
    "Post",
    "PostForm",
    "Head",
    "NewRequest",
    "NewRequestWithContext",
];

/// Struct fields and interfaces the package declares, from all of its files on one side.
#[derive(Default)]
pub struct PackageTypes {
    /// `(struct, field)` → field type text.
    fields: BTreeMap<(String, String), String>,
    interfaces: BTreeSet<String>,
}

impl PackageTypes {
    /// Declarations of every file that parses; the others add nothing.
    pub fn new<'a>(files: impl IntoIterator<Item = &'a str>) -> Self {
        let mut types = Self::default();
        for src in files {
            let Ok(tree) = parse(src, &tree_sitter_go::LANGUAGE.into()) else {
                continue;
            };
            types.collect(tree.root_node(), src);
        }
        types
    }

    fn collect(&mut self, node: Node<'_>, src: &str) {
        if node.kind() == "type_spec"
            && let (Some(name), Some(ty)) = (
                node.child_by_field_name("name"),
                node.child_by_field_name("type"),
            )
        {
            let name = text(name, src).to_owned();
            match ty.kind() {
                "interface_type" => {
                    self.interfaces.insert(name);
                }
                "struct_type" => self.fields(&name, ty, src),
                _ => {}
            }
        }

        for child in named(node) {
            self.collect(child, src);
        }
    }

    fn fields(&mut self, owner: &str, ty: Node<'_>, src: &str) {
        let Some(list) = named(ty)
            .into_iter()
            .find(|n| n.kind() == "field_declaration_list")
        else {
            return;
        };
        for field in named(list)
            .into_iter()
            .filter(|n| n.kind() == "field_declaration")
        {
            let Some(field_ty) = field.child_by_field_name("type") else {
                continue;
            };
            let field_ty = text(field_ty, src).to_owned();
            let mut cursor = field.walk();
            let names: Vec<String> = field
                .children_by_field_name("name", &mut cursor)
                .map(|n| text(n, src).to_owned())
                .collect();
            // An embedded field is named after its type.
            let names = if names.is_empty() {
                vec![base(&field_ty).to_owned()]
            } else {
                names
            };
            for name in names {
                self.fields
                    .insert((owner.to_owned(), name), field_ty.clone());
            }
        }
    }
}

/// `Store` of `*orders.Store`, `Store` or `[]Store`.
fn base(ty: &str) -> &str {
    let ty = ty.trim_start_matches(['*', '[', ']']);
    ty.rsplit_once('.').map_or(ty, |(_, name)| name)
}

struct Visitor<'s> {
    src: &'s str,
    types: &'s PackageTypes,
    /// `(call name, registry symbol)` of registered effect wrappers.
    wrappers: &'s [(String, String)],
    imports: BTreeMap<String, String>,
    /// Declared types of the function's receiver, parameters and locals.
    env: BTreeMap<String, String>,
    symbol: Option<String>,
    guards: Vec<String>,
    out: Vec<Raw>,
}

impl Visitor<'_> {
    fn push(&mut self, kind: SyntaxKind, node: Node<'_>, detail: String, subject: Option<String>) {
        let (start, end) = lines(node);
        self.out.push(Raw {
            kind,
            start,
            end,
            symbol: self.symbol.clone(),
            detail,
            subject,
            guard: self.guards.last().cloned(),
        });
    }

    fn walk(&mut self, node: Node<'_>) {
        match node.kind() {
            "import_spec" => self.import(node),
            "comment" if text(node, self.src).starts_with("//go:build") => {
                let detail = short(text(node, self.src));
                self.push(SyntaxKind::CfgGate, node, detail, None);
            }
            "function_declaration" | "method_declaration" => return self.function(node),
            "func_literal" => {
                if let Some(params) = node.child_by_field_name("parameters") {
                    self.params(params);
                }
            }
            "short_var_declaration" => self.locals(node),
            "var_spec" => {
                if let (Some(name), Some(ty)) = (
                    node.child_by_field_name("name"),
                    node.child_by_field_name("type"),
                ) {
                    self.env
                        .insert(text(name, self.src).into(), text(ty, self.src).into());
                }
            }
            "defer_statement" => {
                let call = text(node, self.src).trim_start_matches("defer").trim();
                self.push(SyntaxKind::Defer, node, short(call), None);
            }
            "go_statement" => {
                let detail = short(text(node, self.src));
                self.push(SyntaxKind::DetachedSpawn, node, detail, None);
            }
            "send_statement" => {
                let channel = node
                    .child_by_field_name("channel")
                    .map_or("", |c| text(c, self.src));
                self.push(
                    SyntaxKind::ChannelOp,
                    node,
                    format!("send on {channel}"),
                    None,
                );
            }
            "unary_expression"
                if node
                    .child_by_field_name("operator")
                    .is_some_and(|o| o.kind() == "<-") =>
            {
                let detail = format!("receive {}", short(text(node, self.src)));
                self.push(SyntaxKind::ChannelOp, node, detail, None);
            }
            "select_statement" => self.push(SyntaxKind::ChannelOp, node, "select".into(), None),
            "if_statement" => return self.branch(node),
            "assignment_statement" => self.discarded(node),
            "call_expression" => self.call(node),
            _ => {}
        }
        for child in named(node) {
            self.walk(child);
        }
    }

    fn import(&mut self, node: Node<'_>) {
        let Some(path) = node.child_by_field_name("path") else {
            return;
        };
        let path = text(path, self.src).trim_matches(['"', '`']).to_owned();
        let local = node.child_by_field_name("name").map_or_else(
            || path.rsplit('/').next().unwrap_or(&path).to_owned(),
            |n| text(n, self.src).to_owned(),
        );
        self.push(SyntaxKind::Import, node, path.clone(), None);
        self.imports.insert(local, path);
    }

    fn function(&mut self, node: Node<'_>) {
        let name = node
            .child_by_field_name("name")
            .map_or("", |n| text(n, self.src));
        let mut env = BTreeMap::new();
        let mut symbol = name.to_owned();
        if let Some(receiver) = node.child_by_field_name("receiver") {
            for (var, ty) in self.declared(receiver) {
                symbol = format!("{}.{name}", base(&ty));
                env.insert(var, ty);
            }
        }

        let saved = (
            mem::replace(&mut self.env, env),
            self.symbol.replace(symbol),
            mem::take(&mut self.guards),
        );
        if let Some(params) = node.child_by_field_name("parameters") {
            self.params(params);
        }

        let signature_end = node
            .child_by_field_name("body")
            .map_or(lines(node).1, |b| lines(b).0);
        let start = lines(node).0;
        let mut signature = |kind, detail: String| {
            self.out.push(Raw {
                kind,
                start,
                end: signature_end,
                symbol: self.symbol.clone(),
                detail,
                subject: None,
                guard: None,
            });
        };
        if let Some((var, _)) = self.env.iter().find(|(_, ty)| *ty == "context.Context") {
            signature(SyntaxKind::ContextParam, format!("{var} context.Context"));
        }

        if let Some(result) = node.child_by_field_name("result")
            && named(result).iter().chain([&result]).any(|n| {
                text(*n, self.src)
                    .split([' ', ',', '(', ')'])
                    .any(|t| t == "error")
            })
        {
            signature(SyntaxKind::ReturnsError, short(text(result, self.src)));
        }

        if let Some(body) = node.child_by_field_name("body") {
            self.walk(body);
        }

        (self.env, self.symbol, self.guards) = saved;
    }

    /// `(name, type)` of every named parameter of a list.
    fn declared(&self, list: Node<'_>) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for param in named(list)
            .into_iter()
            .filter(|n| n.kind() == "parameter_declaration")
        {
            let Some(ty) = param.child_by_field_name("type") else {
                continue;
            };
            let mut cursor = param.walk();
            for name in param.children_by_field_name("name", &mut cursor) {
                out.push((text(name, self.src).into(), text(ty, self.src).into()));
            }
        }
        out
    }

    fn params(&mut self, list: Node<'_>) {
        let declared = self.declared(list);
        self.env.extend(declared);
    }

    /// `x := expr` and `tx, err := db.BeginTx(..)`: the first name takes the call's type.
    fn locals(&mut self, node: Node<'_>) {
        let (Some(left), Some(right)) = (
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
        ) else {
            return;
        };
        for (name, value) in named(left).into_iter().zip(named(right)) {
            if let Some(ty) = self.type_of(value) {
                self.env.insert(text(name, self.src).into(), ty);
            }
        }
    }

    /// The declared type of an expression, when the function or package shows it.
    fn type_of(&self, expr: Node<'_>) -> Option<String> {
        match expr.kind() {
            "identifier" => self.env.get(text(expr, self.src)).cloned(),
            "selector_expression" => {
                let owner = self.type_of(expr.child_by_field_name("operand")?)?;
                let field = text(expr.child_by_field_name("field")?, self.src);
                self.types
                    .fields
                    .get(&(base(&owner).to_owned(), field.to_owned()))
                    .cloned()
            }
            "unary_expression" => self
                .type_of(expr.child_by_field_name("operand")?)
                .map(|t| format!("*{t}")),
            "composite_literal" => Some(text(expr.child_by_field_name("type")?, self.src).into()),
            "parenthesized_expression" => self.type_of(*named(expr).first()?),
            "call_expression" => {
                let function = expr.child_by_field_name("function")?;
                if function.kind() != "selector_expression" {
                    return None;
                }

                let operand = function.child_by_field_name("operand")?;
                let method = text(function.child_by_field_name("field")?, self.src);
                let owner = text(operand, self.src);
                match method {
                    "Open" if self.package(owner) == Some("database/sql") => Some("*sql.DB".into()),
                    "Begin" | "BeginTx" => Some("*sql.Tx".into()),
                    "Beginx" | "BeginTxx" => Some("*sqlx.Tx".into()),
                    "Conn" if self.type_of(operand).is_some_and(|t| t.ends_with("sql.DB")) => {
                        Some("*sql.Conn".into())
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn package(&self, local: &str) -> Option<&str> {
        // A variable shadows a package of the same name.
        if self.env.contains_key(local) {
            return None;
        }

        self.imports.get(local).map(String::as_str)
    }

    fn branch(&mut self, node: Node<'_>) {
        let condition = node
            .child_by_field_name("condition")
            .map(|c| short(text(c, self.src)))
            .unwrap_or_default();
        for field in ["initializer", "condition"] {
            if let Some(child) = node.child_by_field_name(field) {
                self.walk(child);
            }
        }
        for (field, guard) in [
            ("consequence", condition.clone()),
            ("alternative", format!("!({condition})")),
        ] {
            if let Some(child) = node.child_by_field_name(field) {
                self.guards.push(guard);
                self.walk(child);
                self.guards.pop();
            }
        }
    }

    /// `_ = call(..)`: a dropped result, often an error.
    fn discarded(&mut self, node: Node<'_>) {
        let (Some(left), Some(right)) = (
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
        ) else {
            return;
        };
        if named(left).iter().all(|n| text(*n, self.src) == "_")
            && named(right).iter().any(|n| n.kind() == "call_expression")
        {
            let detail = short(text(node, self.src));
            self.push(SyntaxKind::DiscardedResult, node, detail, None);
        }
    }

    fn call(&mut self, node: Node<'_>) {
        let Some(function) = node.child_by_field_name("function") else {
            return;
        };
        match function.kind() {
            "identifier" => {
                let name = text(function, self.src);
                if name == "close" {
                    let detail = short(text(node, self.src));
                    self.push(SyntaxKind::ChannelOp, node, detail, None);
                }

                self.wrapper(node, name, None);
            }
            "selector_expression" => self.method(node, function),
            _ => {}
        }
    }

    fn method(&mut self, node: Node<'_>, function: Node<'_>) {
        let (Some(operand), Some(field)) = (
            function.child_by_field_name("operand"),
            function.child_by_field_name("field"),
        ) else {
            return;
        };
        let method = text(field, self.src);
        let receiver = text(operand, self.src);
        if let Some(package) = self.package(receiver) {
            let kind = match package {
                "reflect" => Some(SyntaxKind::DynamicDispatch),
                "os" if OS_WRITES.contains(&method) => Some(SyntaxKind::WriteCall),
                "net/http" if HTTP_CALLS.contains(&method) => Some(SyntaxKind::NetworkCall),
                _ => None,
            };
            if let Some(kind) = kind {
                self.push(
                    kind,
                    node,
                    format!("{receiver}.{method}"),
                    Some(package.into()),
                );
            }

            return self.wrapper(node, method, None);
        }

        let ty = self.type_of(operand);
        let typed = |s: &Option<String>| {
            s.as_deref()
                .unwrap_or("receiver type unresolved")
                .to_owned()
        };
        let on = format!("{method} on `{}` ({})", short(receiver), typed(&ty));
        let tx_like = ty.as_deref().is_none_or(|t| t.contains("Tx"));
        if TX_BEGIN.contains(&method) || (TX_END.contains(&method) && tx_like) {
            self.push(SyntaxKind::TxBoundary, node, on, ty.clone());
        } else if DB_CALLS.contains(&method) {
            self.push(SyntaxKind::DbCall, node, on, ty.clone());
        } else if ty.as_deref().is_some_and(|t| t.ends_with("http.Client"))
            && (method == "Do" || HTTP_CALLS.contains(&method))
        {
            self.push(SyntaxKind::NetworkCall, node, on, ty.clone());
        } else if ty
            .as_deref()
            .is_some_and(|t| t.ends_with("testing.T") || t.ends_with("testing.B"))
            && method.starts_with("Skip")
        {
            self.push(
                SyntaxKind::TestSkipped,
                node,
                short(text(node, self.src)),
                None,
            );
        } else if let Some(t) = &ty
            && self.types.interfaces.contains(base(t))
        {
            let detail = format!("{method} via interface {t}: implementation chosen at runtime");
            self.push(SyntaxKind::InterfaceCall, node, detail, Some(t.clone()));
        }

        self.wrapper(node, method, ty.as_deref());
    }

    /// A call of a registered wrapper: matched by name and, when the receiver type is known,
    /// by its owner type; `subject` is the type of a transaction passed in.
    fn wrapper(&mut self, node: Node<'_>, name: &str, owner: Option<&str>) {
        let Some((_, symbol)) = self.wrappers.iter().find(|(call, symbol)| {
            call == name
                && owner.is_none_or(|o| {
                    let mut parts = symbol.rsplit('.').skip(1);
                    parts.next().is_none_or(|ty| ty == base(o))
                })
        }) else {
            return;
        };
        let tx = node
            .child_by_field_name("arguments")
            .into_iter()
            .flat_map(named)
            .filter_map(|a| self.type_of(a))
            .find(|t| t.contains("Tx"));
        self.push(SyntaxKind::RegisteredEffectCall, node, symbol.clone(), tx);
    }
}

/// Facts of one Go file; `package` holds the texts of its package's files on the same side.
pub fn facts(
    source: &str,
    package: &PackageTypes,
    wrappers: &[(String, String)],
) -> Result<Vec<Raw>, String> {
    let tree = parse(source, &tree_sitter_go::LANGUAGE.into())?;
    let mut visitor = Visitor {
        src: source,
        types: package,
        wrappers,
        imports: BTreeMap::new(),
        env: BTreeMap::new(),
        symbol: None,
        guards: Vec::new(),
        out: Vec::new(),
    };
    visitor.walk(tree.root_node());
    Ok(visitor.out)
}

#[cfg(test)]
mod tests;
