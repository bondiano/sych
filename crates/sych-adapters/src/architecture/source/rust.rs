//! Rust facts from `syn`, by deslop-please's rules (`f678d10`, MIT; see
//! `sych-core/src/architecture/UPSTREAM-LICENSE`): what code does with a field it names, what
//! a field's type holds, and which functions only hand out fields or pass a call on. Macros
//! are not expanded; a macro's arguments are read when they parse as expressions.

use proc_macro2::{LineColumn, Span as Spot};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{
    Attribute, BinOp, Block, Expr, Fields, FnArg, ImplItem, Item, Member, Meta, Pat, ReturnType,
    Signature, Stmt, Token, Type, UnOp,
};

use super::{Access, Definition, Plain, Pos, Shape, SourceFacts, Span};

/// Methods that hand out a copy of the value they are called on.
const COPIES_OUT: [&str; 4] = ["clone", "get", "load", "to_owned"];
/// Methods that borrow the value they are called on to read.
const BORROWS: [&str; 8] = [
    "as_bytes", "as_deref", "as_path", "as_ref", "as_slice", "as_str", "borrow", "deref",
];
/// Calls that overwrite what they are called on through a shared reference.
const OVERWRITES: [&str; 5] = ["replace", "set", "store", "swap", "take"];
const SCALARS: [&str; 16] = [
    "bool", "char", "f32", "f64", "i128", "i16", "i32", "i64", "i8", "isize", "u128", "u16", "u32",
    "u64", "u8", "usize",
];
const TEXT: [&str; 8] = [
    "CStr", "CString", "OsStr", "OsString", "Path", "PathBuf", "String", "str",
];
const HOLDS_DATA: [&str; 3] = ["Box", "Option", "Vec"];
const MARKERS: [&str; 2] = ["PhantomData", "PhantomPinned"];
const STD_DERIVES: [&str; 9] = [
    "Clone",
    "Copy",
    "Debug",
    "Default",
    "Eq",
    "Hash",
    "Ord",
    "PartialEq",
    "PartialOrd",
];

pub(super) fn read(text: &str) -> Option<SourceFacts> {
    let file = syn::parse_file(text).ok()?;
    let lines: Vec<&str> = text.lines().collect();
    let mut reader = Reader {
        lines,
        facts: SourceFacts::default(),
        receiver: None,
    };
    reader.visit_file(&file);
    Some(reader.facts)
}

struct Reader<'t> {
    lines: Vec<&'t str>,
    facts: SourceFacts,
    /// Inside an `impl` block: whether it implements a trait.
    receiver: Option<bool>,
}

impl Reader<'_> {
    /// proc-macro2 counts columns in chars.
    fn pos(&self, at: LineColumn) -> Pos {
        let line = at.line.saturating_sub(1);
        let text = self.lines.get(line).copied().unwrap_or_default();
        let column = text
            .char_indices()
            .nth(at.column)
            .map_or(text.len(), |(byte, _)| byte);
        Pos {
            line: u32::try_from(line).unwrap_or(u32::MAX),
            column: u32::try_from(column).unwrap_or(u32::MAX),
        }
    }

    fn span(&self, spot: Spot) -> Span {
        Span {
            start: self.pos(spot.start()),
            end: self.pos(spot.end()),
        }
    }

    fn attributes(&mut self, attrs: &[Attribute], extent: Spot) {
        let mut unknown = false;
        for attr in attrs {
            let path = attr.path();
            let name = path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .unwrap_or_default();
            let text = quote::ToTokens::to_token_stream(&attr.meta).to_string();
            if name == "test" || (name == "cfg" && text.contains("test") && !text.contains("not")) {
                let span = self.span(extent);
                self.facts.tests.push(span);
            }

            // A one-word attribute that is not the compiler's is a derive's helper, as
            // `serde` is; a derive macro is said by the derive.
            unknown |= path.segments.len() > 1;
            if name == "derive"
                && let Meta::List(list) = &attr.meta
            {
                unknown |= list
                    .tokens
                    .to_string()
                    .split(',')
                    .filter_map(|d| d.rsplit("::").next())
                    .map(str::trim)
                    .any(|d| !d.is_empty() && !STD_DERIVES.contains(&d));
            }
        }
        if unknown && !self.facts.unknowns.contains(&MACROS) {
            self.facts.unknowns.push(MACROS);
        }
    }

    fn function(&mut self, sig: &Signature, block: &Block, extent: Spot) {
        let mut definition = Definition::new(self.pos(sig.ident.span().start()), self.span(extent));
        let receiver = sig.receiver();
        definition.takes_self = receiver.is_some();
        definition.trait_impl = self.receiver == Some(true);
        let params: Vec<String> = sig
            .inputs
            .iter()
            .filter_map(|input| match input {
                FnArg::Receiver(_) => Some("self".to_owned()),
                FnArg::Typed(typed) => match &*typed.pat {
                    Pat::Ident(ident) => Some(ident.ident.to_string()),
                    _ => None,
                },
            })
            .collect();
        let all_named = params.len() == sig.inputs.len();
        let by_mut_ref = receiver.is_some_and(|r| r.reference.is_some() && r.mutability.is_some());
        let lends_to_write = matches!(&sig.output, ReturnType::Type(_, ty)
            if matches!(&**ty, Type::Reference(r) if r.mutability.is_some()));
        definition.shape = sole(block).and_then(|body| {
            self.accessor(body, receiver.is_some(), !by_mut_ref && !lends_to_write)
                .or_else(|| all_named.then(|| self.forwarder(body, &params)).flatten())
        });
        self.facts.definitions.push(definition);
    }

    /// `self.f`, `&self.f`, `self.f.clone()`, a tuple of them, or `self.f = value`.
    fn accessor(&self, body: &Expr, receiver: bool, reads: bool) -> Option<Shape> {
        if !receiver {
            return None;
        }

        let (mut lent, mut copied) = (Vec::new(), Vec::new());
        let mut to_read = reads;
        let mut parts = |expr: &Expr| -> bool {
            match unwrapped(expr) {
                Expr::Field(field) if is_self(&field.base) => {
                    copied.push(self.member(&field.member));
                    true
                }
                Expr::Reference(reference) => match unwrapped(&reference.expr) {
                    Expr::Field(field) if is_self(&field.base) => {
                        to_read &= reference.mutability.is_none();
                        lent.push(self.member(&field.member));
                        true
                    }
                    _ => false,
                },
                Expr::MethodCall(call) if call.args.is_empty() => match unwrapped(&call.receiver) {
                    Expr::Field(field) if is_self(&field.base) => {
                        let method = call.method.to_string();
                        if COPIES_OUT.contains(&method.as_str()) {
                            copied.push(self.member(&field.member));
                            true
                        } else if BORROWS.contains(&method.as_str()) {
                            lent.push(self.member(&field.member));
                            true
                        } else {
                            false
                        }
                    }
                    _ => false,
                },
                _ => false,
            }
        };
        let handed = match unwrapped(body) {
            Expr::Tuple(tuple) => tuple.elems.iter().all(&mut parts),
            Expr::Assign(assign) => match (unwrapped(&assign.left), unwrapped(&assign.right)) {
                (Expr::Field(field), Expr::Path(_)) if is_self(&field.base) => {
                    to_read = false;
                    lent.push(self.member(&field.member));
                    true
                }
                _ => false,
            },
            other => parts(other),
        };
        (handed && !(lent.is_empty() && copied.is_empty())).then_some(Shape::Access {
            lent,
            copied,
            to_read,
        })
    }

    /// One call of a function or of a method on `self` or its fields, given `params` in order.
    fn forwarder(&self, body: &Expr, params: &[String]) -> Option<Shape> {
        let passes = |args: &Punctuated<Expr, Token![,]>, given: &[String]| {
            args.len() == given.len()
                && args.iter().zip(given).all(|(arg, param)| {
                    matches!(unwrapped(arg), Expr::Path(path) if path.path.is_ident(param))
                })
        };
        match unwrapped(body) {
            Expr::MethodCall(call) => {
                let mut fields = Vec::new();
                let mut at = unwrapped(&call.receiver);
                while let Expr::Field(field) = at {
                    fields.push(self.member(&field.member));
                    at = unwrapped(&field.base);
                }
                let rest = params.strip_prefix(&["self".to_owned()]).unwrap_or(params);
                (is_self(at) && passes(&call.args, rest)).then(|| Shape::Forward {
                    callee: self.pos(call.method.span().start()),
                    fields,
                })
            }
            Expr::Call(call) => match unwrapped(&call.func) {
                Expr::Path(path) if passes(&call.args, params) => {
                    let last = path.path.segments.last()?;
                    Some(Shape::Forward {
                        callee: self.pos(last.ident.span().start()),
                        fields: Vec::new(),
                    })
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn member(&self, member: &Member) -> Pos {
        self.pos(member.span().start())
    }

    /// Records what is done with `expr`, which the code around it does `with`.
    fn expr(&mut self, expr: &Expr, with: Access) {
        match expr {
            Expr::Field(field) => {
                let at = self.member(&field.member);
                self.facts.members.push((at, with));
                self.expr(&field.base, Access::Reach);
            }
            Expr::MethodCall(call) => {
                let at = self.pos(call.method.span().start());
                self.facts.members.push((at, with));
                let method = call.method.to_string();
                let on = match method.as_str() {
                    "get" if !call.args.is_empty() => Access::Call,
                    m if COPIES_OUT.contains(&m) => Access::Copy,
                    m if BORROWS.contains(&m) => Access::Borrow,
                    m if OVERWRITES.contains(&m) => Access::Reach,
                    _ => Access::Call,
                };
                self.expr(&call.receiver, on);
                for arg in &call.args {
                    self.expr(arg, Access::Copy);
                }
            }
            Expr::Reference(reference) => {
                let inner = if reference.mutability.is_some() {
                    Access::Reach
                } else {
                    Access::Borrow
                };
                self.expr(&reference.expr, inner);
            }
            Expr::Assign(assign) => {
                self.expr(&assign.left, Access::Reach);
                self.expr(&assign.right, Access::Copy);
            }
            Expr::Binary(binary) => {
                let left = if compound(binary.op) {
                    Access::Reach
                } else {
                    Access::Copy
                };
                self.expr(&binary.left, left);
                self.expr(&binary.right, Access::Copy);
            }
            Expr::Index(index) => {
                self.expr(&index.expr, Access::Reach);
                self.expr(&index.index, Access::Copy);
            }
            Expr::Unary(unary) if matches!(unary.op, UnOp::Deref(_)) => {
                self.expr(&unary.expr, with);
            }
            Expr::Paren(paren) => self.expr(&paren.expr, with),
            Expr::Group(group) => self.expr(&group.expr, with),
            Expr::Macro(mac) => self.macro_arguments(&mac.mac),
            Expr::Closure(closure) => self.expr(&closure.body, Access::Copy),
            other => visit::visit_expr(self, other),
        }
    }

    fn macro_arguments(&mut self, mac: &syn::Macro) {
        let parser = Punctuated::<Expr, Token![,]>::parse_terminated;
        if let Ok(args) = mac.parse_body_with(parser) {
            for arg in &args {
                self.expr(arg, Access::Copy);
            }
        }
    }
}

const MACROS: &str =
    "attribute and derive macros are not expanded: what their code names is not seen";

impl<'ast> Visit<'ast> for Reader<'_> {
    fn visit_item(&mut self, item: &'ast Item) {
        match item {
            Item::Use(used) => {
                let span = self.span(used.span());
                self.facts.imports.push(span);
                return;
            }
            Item::Fn(function) => {
                self.attributes(&function.attrs, function.span());
                self.function(&function.sig, &function.block, function.span());
            }
            Item::Impl(block) => {
                self.attributes(&block.attrs, block.span());
                let outer = self.receiver.replace(block.trait_.is_some());
                for inner in &block.items {
                    match inner {
                        ImplItem::Fn(method) => {
                            self.attributes(&method.attrs, method.span());
                            self.function(&method.sig, &method.block, method.span());
                            self.visit_block(&method.block);
                        }
                        other => visit::visit_impl_item(self, other),
                    }
                }
                self.receiver = outer;
                return;
            }
            Item::Struct(structure) => {
                self.attributes(&structure.attrs, structure.span());
                self.fields(&structure.fields);
            }
            Item::Enum(enumeration) => {
                self.attributes(&enumeration.attrs, enumeration.span());
                for variant in &enumeration.variants {
                    self.fields(&variant.fields);
                }
            }
            Item::Mod(module) => self.attributes(&module.attrs, module.span()),
            Item::Const(_) | Item::Static(_) | Item::Trait(_) | Item::Type(_) | Item::Union(_) => {
                if let Some(attrs) = attributes(item) {
                    self.attributes(attrs, item.span());
                }
            }
            _ => {}
        }
        visit::visit_item(self, item);
    }

    fn visit_expr(&mut self, expr: &'ast Expr) {
        self.expr(expr, Access::Copy);
    }

    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        if let Stmt::Macro(mac) = stmt {
            self.macro_arguments(&mac.mac);
        } else {
            visit::visit_stmt(self, stmt);
        }
    }
}

impl Reader<'_> {
    fn fields(&mut self, fields: &Fields) {
        for field in fields {
            let Some(ident) = &field.ident else { continue };
            let mut definition =
                Definition::new(self.pos(ident.span().start()), self.span(field.span()));
            definition.plain = plain(&field.ty);
            self.facts.definitions.push(definition);
        }
    }
}

fn attributes(item: &Item) -> Option<&[Attribute]> {
    Some(match item {
        Item::Const(i) => &i.attrs,
        Item::Static(i) => &i.attrs,
        Item::Trait(i) => &i.attrs,
        Item::Type(i) => &i.attrs,
        Item::Union(i) => &i.attrs,
        _ => return None,
    })
}

/// The one expression a body computes: its tail, a `return`, or one statement.
fn sole(block: &Block) -> Option<&Expr> {
    match block.stmts.as_slice() {
        [Stmt::Expr(Expr::Return(ret), _)] => ret.expr.as_deref(),
        [Stmt::Expr(expr, _)] => Some(expr),
        _ => None,
    }
}

fn unwrapped(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(paren) => unwrapped(&paren.expr),
        Expr::Group(group) => unwrapped(&group.expr),
        Expr::Return(ret) => ret.expr.as_deref().map_or(expr, unwrapped),
        other => other,
    }
}

fn is_self(expr: &Expr) -> bool {
    matches!(unwrapped(expr), Expr::Path(path) if path.path.is_ident("self"))
}

fn compound(op: BinOp) -> bool {
    matches!(
        op,
        BinOp::AddAssign(_)
            | BinOp::SubAssign(_)
            | BinOp::MulAssign(_)
            | BinOp::DivAssign(_)
            | BinOp::RemAssign(_)
            | BinOp::BitXorAssign(_)
            | BinOp::BitAndAssign(_)
            | BinOp::BitOrAssign(_)
            | BinOp::ShlAssign(_)
            | BinOp::ShrAssign(_)
    )
}

/// What a field's declared type holds.
fn plain(ty: &Type) -> Plain {
    let data = |is: bool| if is { Plain::Data } else { Plain::No };
    match ty {
        Type::Path(path) => {
            let Some(last) = path.path.segments.last() else {
                return Plain::No;
            };
            let name = last.ident.to_string();
            let name = name.as_str();
            if MARKERS.contains(&name) {
                Plain::Nothing
            } else if path.path.segments.len() == 1 && SCALARS.contains(&name) {
                Plain::Scalar
            } else if TEXT.contains(&name) {
                Plain::Data
            } else if HOLDS_DATA.contains(&name) {
                match &last.arguments {
                    syn::PathArguments::AngleBracketed(args) => data(args.args.iter().all(|arg| {
                        matches!(arg, syn::GenericArgument::Type(inner) if plain(inner) != Plain::No)
                    })),
                    _ => Plain::No,
                }
            } else {
                Plain::No
            }
        }
        Type::Reference(reference) => data(plain(&reference.elem) != Plain::No),
        Type::Tuple(tuple) => data(tuple.elems.iter().all(|t| plain(t) != Plain::No)),
        Type::Array(array) => data(plain(&array.elem) != Plain::No),
        Type::Slice(slice) => data(plain(&slice.elem) != Plain::No),
        Type::Paren(paren) => plain(&paren.elem),
        Type::Group(group) => plain(&group.elem),
        _ => Plain::No,
    }
}

#[cfg(test)]
mod tests;
