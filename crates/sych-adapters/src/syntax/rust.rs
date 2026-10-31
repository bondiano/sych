//! Rust shapes from `syn`: async boundaries, guards across
//! `.await`, detached tasks, discarded results, `Drop`, public API, cfg and serde attributes.
//! Macros are not expanded; their bodies are invisible and listed as `UnexpandedMacro`.
//! Code under a cfg the analysed build skips is `InactiveCfg`; logged `Debug` values are
//! `DebugFormatted` with their type when a parameter names it.

use proc_macro2::Span;
use quote::ToTokens;
use sych_core::SyntaxKind;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Attribute, Block, Expr, ImplItemFn, Item, ItemFn, Meta, Pat, Stmt, Visibility};

use super::logging::{LOG_MACROS, debug_values, param, std_debug};
use super::{Raw, cfg};

const GUARD_METHODS: &[&str] = &["lock", "read", "write", "borrow_mut", "borrow", "try_lock"];
const KNOWN_MACROS: &[&str] = &[
    "println",
    "print",
    "eprintln",
    "eprint",
    "format",
    "format_args",
    "vec",
    "assert",
    "assert_eq",
    "assert_ne",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
    "panic",
    "todo",
    "unimplemented",
    "unreachable",
    "write",
    "writeln",
    "matches",
    "env",
    "concat",
    "include_str",
    "include_bytes",
    "stringify",
    "line",
    "file",
    "column",
    "cfg",
    // Logging macros of `tracing` and `log`: their arguments are visible format arguments.
    // shortcut: matched by name, so a project macro called `info!` is taken as logging too.
    "trace",
    "debug",
    "info",
    "warn",
    "error",
    "event",
    // Error and data macros of `anyhow` and `serde_json`, and `tokio` joins: their effect is
    // fixed by the crate (`bail!` returns the error, `join!` awaits every future).
    "bail",
    "ensure",
    "anyhow",
    "format_err",
    "json",
    "join",
    "try_join",
    "pin",
];
const MAX_DETAIL: usize = 160;

fn text(tokens: &impl ToTokens) -> String {
    let mut s = tokens.to_token_stream().to_string();
    if s.len() > MAX_DETAIL {
        let cut = (0..=MAX_DETAIL)
            .rev()
            .find(|&i| s.is_char_boundary(i))
            .unwrap_or(0);
        s.truncate(cut);
        s.push('…');
    }

    s
}

pub(super) fn last_segment(path: &syn::Path) -> String {
    path.segments
        .last()
        .map(|s| s.ident.to_string())
        .unwrap_or_default()
}

/// Whether `expr` produces a lock or borrow guard, and which kind.
fn guard_kind(expr: &Expr) -> Option<&'static str> {
    match expr {
        Expr::MethodCall(m) if ["unwrap", "expect"].contains(&m.method.to_string().as_str()) => {
            guard_kind(&m.receiver)
        }
        Expr::Try(t) => guard_kind(&t.expr),
        Expr::Await(a) => guard_kind(&a.base).map(|_| "async lock"),
        Expr::MethodCall(m) if GUARD_METHODS.contains(&m.method.to_string().as_str()) => {
            Some("sync lock")
        }
        _ => None,
    }
}

#[derive(Default)]
struct AwaitFinder(Option<Span>);

impl Visit<'_> for AwaitFinder {
    fn visit_expr_await(&mut self, it: &syn::ExprAwait) {
        self.0.get_or_insert(it.await_token.span());
    }
    // A closure or async block runs later; its awaits do not hold this frame's guard.
    fn visit_expr_closure(&mut self, _: &syn::ExprClosure) {}
    fn visit_expr_async(&mut self, _: &syn::ExprAsync) {}
}

/// Attributes of a statement, e.g. `#[cfg(feature = "x")] call();`.
fn stmt_attrs(stmt: &Stmt) -> &[Attribute] {
    match stmt {
        Stmt::Local(l) => &l.attrs,
        Stmt::Macro(m) => &m.attrs,
        Stmt::Expr(e, _) => match e {
            Expr::Call(x) => &x.attrs,
            Expr::MethodCall(x) => &x.attrs,
            Expr::Macro(x) => &x.attrs,
            Expr::Assign(x) => &x.attrs,
            Expr::Block(x) => &x.attrs,
            Expr::If(x) => &x.attrs,
            Expr::Match(x) => &x.attrs,
            Expr::ForLoop(x) => &x.attrs,
            _ => &[],
        },
        Stmt::Item(_) => &[],
    }
}

fn is_spawn(expr: &Expr) -> bool {
    match expr {
        Expr::Call(c) => match &*c.func {
            Expr::Path(p) => last_segment(&p.path).starts_with("spawn"),
            _ => false,
        },
        _ => false,
    }
}

#[derive(Default)]
struct Visitor {
    out: Vec<Raw>,
    /// Default features of the crate: their cfg code is analysed.
    features: Vec<String>,
    /// Parameter names and type names of the enclosing functions, innermost last.
    params: Vec<Vec<(String, String)>>,
    /// Enclosing functions: name and whether they are `async`.
    frames: Vec<(String, bool)>,
}

impl Visitor {
    fn push(&mut self, kind: SyntaxKind, span: Span, detail: String) {
        self.push_subject(kind, span, detail, None);
    }

    fn push_subject(
        &mut self,
        kind: SyntaxKind,
        span: Span,
        detail: String,
        subject: Option<String>,
    ) {
        self.out.push(Raw {
            kind,
            start: u32::try_from(span.start().line).unwrap_or(u32::MAX),
            end: u32::try_from(span.end().line).unwrap_or(u32::MAX),
            symbol: self.frames.last().map(|(name, _)| name.clone()),
            detail,
            subject,
            guard: None,
        });
    }

    /// The type name of parameter `name` of the innermost function, if it has one.
    fn param_type(&self, name: &str) -> Option<String> {
        self.params
            .last()?
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, t)| t.clone())
    }

    fn in_async(&self) -> bool {
        self.frames.last().is_some_and(|(_, is_async)| *is_async)
    }

    /// Facts of `attrs` on the item or statement spanning `item`.
    fn attributes(&mut self, attrs: &[Attribute], item: Span) {
        for attr in attrs {
            let path = last_segment(attr.path());
            match path.as_str() {
                "cfg" => {
                    self.push(SyntaxKind::CfgGate, attr.span(), text(attr));
                    let active = attr
                        .parse_args::<Meta>()
                        .is_ok_and(|m| cfg::active(&m, &self.features));
                    if !active {
                        self.push(
                            SyntaxKind::InactiveCfg,
                            item,
                            format!("{} is not compiled in the analysed build", text(attr)),
                        );
                    }
                }
                "cfg_attr" => self.push(SyntaxKind::CfgGate, attr.span(), text(attr)),
                "serde" => self.push(SyntaxKind::SerdeAttribute, attr.span(), text(attr)),
                "allow" | "expect" => {
                    self.push(SyntaxKind::LintSuppressed, attr.span(), text(attr));
                }
                "ignore" => self.push(SyntaxKind::TestSkipped, attr.span(), text(attr)),
                _ => {}
            }
        }
    }

    fn function(
        &mut self,
        attrs: &[Attribute],
        vis: &Visibility,
        sig: &syn::Signature,
        block: &Block,
        item: Span,
    ) {
        self.attributes(attrs, item);
        let name = sig.ident.to_string();
        let is_async = sig.asyncness.is_some();
        self.frames.push((name, is_async));
        self.params
            .push(sig.inputs.iter().filter_map(param).collect());
        if is_async {
            self.push(SyntaxKind::AsyncFn, sig.span(), text(sig));
        }

        if matches!(vis, Visibility::Public(_)) {
            self.push(SyntaxKind::PublicItem, sig.span(), text(sig));
        }

        self.visit_block(block);
        self.frames.pop();
        self.params.pop();
    }

    /// Guards bound by `let` and still alive at a later `.await` of the same block.
    fn guards(&mut self, block: &Block) {
        if !self.in_async() {
            return;
        }

        let mut live: Vec<(String, &'static str, usize)> = Vec::new();
        for stmt in &block.stmts {
            if let Stmt::Expr(Expr::Call(call), _) = stmt
                && let Expr::Path(func) = &*call.func
                && last_segment(&func.path) == "drop"
            {
                let dropped = call.args.first().map(text).unwrap_or_default();
                live.retain(|(name, _, _)| *name != dropped);
                continue;
            }

            let mut finder = AwaitFinder::default();
            finder.visit_stmt(stmt);
            if let Some(at) = finder.0 {
                for (name, kind, line) in live.drain(..) {
                    self.push(
                        SyntaxKind::GuardAcrossAwait,
                        at,
                        format!(
                            "guard `{name}` ({kind}, bound at line {line}) is alive at this .await"
                        ),
                    );
                }
            }

            if let Stmt::Local(local) = stmt
                && let (Pat::Ident(ident), Some(init)) = (&local.pat, &local.init)
                && let Some(kind) = guard_kind(&init.expr)
            {
                live.push((ident.ident.to_string(), kind, local.span().start().line));
            }
        }
    }
}

impl<'ast> Visit<'ast> for Visitor {
    fn visit_item_fn(&mut self, it: &'ast ItemFn) {
        self.function(&it.attrs, &it.vis, &it.sig, &it.block, it.span());
    }

    fn visit_impl_item_fn(&mut self, it: &'ast ImplItemFn) {
        self.function(&it.attrs, &it.vis, &it.sig, &it.block, it.span());
    }

    fn visit_item(&mut self, it: &'ast Item) {
        match it {
            Item::Struct(s) => {
                self.attributes(&s.attrs, s.span());
                for field in &s.fields {
                    self.attributes(&field.attrs, field.span());
                }
                if matches!(s.vis, Visibility::Public(_)) {
                    self.push(
                        SyntaxKind::PublicItem,
                        s.ident.span(),
                        format!("struct {}", s.ident),
                    );
                }
            }
            Item::Enum(e) => {
                self.attributes(&e.attrs, e.span());
                if matches!(e.vis, Visibility::Public(_)) {
                    self.push(
                        SyntaxKind::PublicItem,
                        e.ident.span(),
                        format!("enum {}", e.ident),
                    );
                }
            }
            Item::Trait(t) => {
                self.attributes(&t.attrs, t.span());
                if matches!(t.vis, Visibility::Public(_)) {
                    let items: Vec<String> = t.items.iter().map(text).collect();
                    let detail: String = format!("trait {} {{ {} }}", t.ident, items.join("; "));
                    self.push(
                        SyntaxKind::PublicItem,
                        t.ident.span(),
                        detail.chars().take(MAX_DETAIL).collect(),
                    );
                }
            }
            Item::Impl(i) => {
                self.attributes(&i.attrs, i.span());
                if let Some((_, path, _)) = &i.trait_
                    && last_segment(path) == "Drop"
                {
                    self.push(
                        SyntaxKind::DropImpl,
                        i.span(),
                        format!("impl Drop for {}", text(&i.self_ty)),
                    );
                }
            }
            Item::Use(u) => self.push(SyntaxKind::Import, u.span(), text(&u.tree)),
            Item::Mod(m) => {
                self.attributes(&m.attrs, m.span());
                if m.attrs
                    .iter()
                    .any(|a| last_segment(a.path()) == "cfg" && text(a).contains("test"))
                {
                    self.push(SyntaxKind::TestModule, m.span(), format!("mod {}", m.ident));
                }
            }
            _ => {}
        }
        visit::visit_item(self, it);
    }

    fn visit_block(&mut self, it: &'ast Block) {
        self.guards(it);
        for stmt in &it.stmts {
            self.attributes(stmt_attrs(stmt), stmt.span());
            match stmt {
                Stmt::Local(local) if matches!(local.pat, Pat::Wild(_)) => {
                    if let Some(init) = &local.init {
                        let kind = if is_spawn(&init.expr) {
                            SyntaxKind::DetachedSpawn
                        } else {
                            SyntaxKind::DiscardedResult
                        };
                        if matches!(
                            &*init.expr,
                            Expr::Call(_) | Expr::MethodCall(_) | Expr::Await(_) | Expr::Try(_)
                        ) {
                            self.push(kind, local.span(), text(&init.expr));
                        }
                    }
                }
                Stmt::Expr(expr, Some(_)) if is_spawn(expr) => {
                    self.push(SyntaxKind::DetachedSpawn, expr.span(), text(expr));
                }
                Stmt::Expr(Expr::MethodCall(m), Some(_)) if m.method == "ok" => {
                    self.push(SyntaxKind::DiscardedResult, m.span(), text(m));
                }
                _ => {}
            }
        }
        visit::visit_block(self, it);
    }

    fn visit_expr_method_call(&mut self, it: &'ast syn::ExprMethodCall) {
        if it.method == "unwrap" || it.method == "expect" {
            self.push(
                SyntaxKind::Unwrap,
                it.method.span(),
                format!(".{}()", it.method),
            );
        }

        visit::visit_expr_method_call(self, it);
    }

    fn visit_expr_call(&mut self, it: &'ast syn::ExprCall) {
        if self.in_async()
            && let Expr::Path(p) = &*it.func
        {
            let path = text(&p.path).replace(' ', "");
            let blocking = path.ends_with("thread::sleep")
                || path.ends_with("block_on")
                || (path.contains("fs::") && !path.contains("tokio"));
            if blocking {
                self.push(SyntaxKind::BlockingInAsync, it.span(), path);
            }
        }

        visit::visit_expr_call(self, it);
    }

    fn visit_expr_closure(&mut self, it: &'ast syn::ExprClosure) {
        // A closure is not async unless it says so; guards and blocking checks follow it.
        self.frames.push((
            self.frames
                .last()
                .map_or_else(String::new, |(n, _)| n.clone()),
            it.asyncness.is_some(),
        ));
        visit::visit_expr_closure(self, it);
        self.frames.pop();
    }

    fn visit_macro(&mut self, it: &'ast syn::Macro) {
        let name = last_segment(&it.path);
        if name == "select" {
            self.push(
                SyntaxKind::SelectMacro,
                it.span(),
                format!("{}!", text(&it.path).replace(' ', "")),
            );
        } else if !KNOWN_MACROS.contains(&name.as_str()) {
            self.push(
                SyntaxKind::UnexpandedMacro,
                it.span(),
                format!("{name}! is not expanded"),
            );
        } else if LOG_MACROS.contains(&name.as_str()) {
            for value in debug_values(it.tokens.clone()) {
                let subject = value.as_deref().and_then(|v| self.param_type(v));
                if subject.as_deref().is_some_and(std_debug) {
                    continue;
                }

                let detail = match (&value, &subject) {
                    (Some(v), Some(t)) => format!("`{v}` ({t}) logged with Debug in {name}!"),
                    (Some(v), None) => {
                        format!("`{v}` of an unknown type logged with Debug in {name}!")
                    }
                    (None, _) => format!("a positional argument logged with Debug in {name}!"),
                };
                self.push_subject(SyntaxKind::DebugFormatted, it.span(), detail, subject);
            }
        }
    }
}

/// Facts of one file; `features` are the crate's default features.
pub fn facts(source: &str, features: &[String]) -> Result<Vec<Raw>, String> {
    let file = syn::parse_file(source).map_err(|e| {
        let at = e.span().start();
        format!("parse error at {}:{}: {e}", at.line, at.column + 1)
    })?;
    let mut visitor = Visitor {
        features: features.to_vec(),
        ..Visitor::default()
    };
    visitor.visit_file(&file);
    Ok(visitor.out)
}

#[cfg(test)]
mod tests;
