//! TypeScript and JavaScript facts from oxc. A method's `this` is its `self`; values are
//! shared, so a use of a property as a whole borrows it. Untyped values, `any` and JavaScript
//! leave members the index does not resolve.

use std::mem;
use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    AssignmentExpression, CallExpression, Class, ClassElement, ComputedMemberExpression,
    ExportAllDeclaration, ExportNamedDeclaration, Expression, Function, FunctionBody,
    ImportDeclaration, MethodDefinitionKind, PrivateFieldExpression, PropertyKey, Statement,
    StaticMemberExpression, TSAnyKeyword, TSType, UnaryExpression, UpdateExpression,
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_span::SourceType;
use oxc_syntax::operator::UnaryOperator;
use oxc_syntax::scope::ScopeFlags;

use super::{Access, Definition, Lines, Plain, Pos, Shape, SourceFacts};

const UNTYPED: &str =
    "JavaScript declares no types: members of values whose type is not inferred are not resolved";
const ANY: &str = "values typed `any` hide which members their code uses";

pub(super) fn read(path: &str, text: &str) -> Option<SourceFacts> {
    let source_type = SourceType::from_path(Path::new(path)).ok()?;
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, text, source_type).parse();
    if parsed.panicked || !parsed.diagnostics.is_empty() {
        return None;
    }

    let mut reader = Reader {
        lines: Lines::new(text),
        facts: SourceFacts::default(),
        ctx: Access::Borrow,
    };
    if !source_type.is_typescript() {
        reader.facts.unknowns.push(UNTYPED);
    }

    reader.visit_program(&parsed.program);
    Some(reader.facts)
}

struct Reader {
    lines: Lines,
    facts: SourceFacts,
    /// What the code around the expression being visited does with its value.
    ctx: Access,
}

impl Reader {
    fn at(&self, offset: u32) -> Pos {
        self.lines.pos(offset as usize)
    }

    fn with(&mut self, ctx: Access, visit: impl FnOnce(&mut Self)) {
        let saved = mem::replace(&mut self.ctx, ctx);
        visit(self);
        self.ctx = saved;
    }

    fn key(&self, key: &PropertyKey<'_>) -> Option<Pos> {
        match key {
            PropertyKey::StaticIdentifier(id) => Some(self.at(id.span.start)),
            PropertyKey::PrivateIdentifier(id) => Some(self.at(id.span.start + 1)),
            _ => None,
        }
    }

    fn class_members(&mut self, class: &Class<'_>) {
        for element in &class.body.body {
            match element {
                ClassElement::MethodDefinition(method) => {
                    let Some(name) = self.key(&method.key) else {
                        continue;
                    };
                    let span = method.span;
                    let mut definition = Definition::new(
                        name,
                        self.lines.span(span.start as usize, span.end as usize),
                    );
                    definition.takes_self = !method.r#static;
                    let params = params(&method.value);
                    definition.shape =
                        method
                            .value
                            .body
                            .as_ref()
                            .and_then(|body| match method.kind {
                                MethodDefinitionKind::Get => self.getter(body),
                                MethodDefinitionKind::Set => self.setter(body, &params),
                                MethodDefinitionKind::Method => self
                                    .getter(body)
                                    .filter(|_| params.is_empty())
                                    .or_else(|| self.forwarder(body, &params)),
                                MethodDefinitionKind::Constructor => None,
                            });
                    self.facts.definitions.push(definition);
                }
                ClassElement::PropertyDefinition(property) => {
                    let Some(name) = self.key(&property.key) else {
                        continue;
                    };
                    let span = property.span;
                    let mut definition = Definition::new(
                        name,
                        self.lines.span(span.start as usize, span.end as usize),
                    );
                    definition.plain = property
                        .type_annotation
                        .as_ref()
                        .map(|t| plain_type(&t.type_annotation))
                        .or_else(|| property.value.as_ref().map(valued))
                        .unwrap_or(Plain::No);
                    self.facts.definitions.push(definition);
                }
                _ => {}
            }
        }
    }

    /// `return this.f`.
    fn getter(&self, body: &FunctionBody<'_>) -> Option<Shape> {
        let [Statement::ReturnStatement(ret)] = body.statements.as_slice() else {
            return None;
        };
        let field = self.own(ret.argument.as_ref()?)?;
        Some(Shape::Access {
            lent: vec![field],
            copied: Vec::new(),
            to_read: true,
        })
    }

    /// `this.f = value`.
    fn setter(&self, body: &FunctionBody<'_>, params: &[&str]) -> Option<Shape> {
        let [Statement::ExpressionStatement(stmt)] = body.statements.as_slice() else {
            return None;
        };
        let Expression::AssignmentExpression(assign) = stmt.expression.without_parentheses() else {
            return None;
        };
        let Expression::Identifier(value) = assign.right.without_parentheses() else {
            return None;
        };
        let field = match assign.left.as_member_expression() {
            Some(member)
                if matches!(
                    member.object().without_parentheses(),
                    Expression::ThisExpression(_)
                ) =>
            {
                member
                    .static_property_info()
                    .map(|(span, _)| self.at(span.start))?
            }
            _ => return None,
        };
        params
            .contains(&value.name.as_str())
            .then(|| Shape::Access {
                lent: vec![field],
                copied: Vec::new(),
                to_read: false,
            })
    }

    /// One call given the parameters in order, of a function or of a method on `this` or its
    /// properties.
    fn forwarder(&self, body: &FunctionBody<'_>, params: &[&str]) -> Option<Shape> {
        let expr = match body.statements.as_slice() {
            [Statement::ReturnStatement(ret)] => ret.argument.as_ref()?,
            [Statement::ExpressionStatement(stmt)] => &stmt.expression,
            _ => return None,
        };
        let Expression::CallExpression(call) = expr.without_parentheses() else {
            return None;
        };
        let args: Vec<&str> = call
            .arguments
            .iter()
            .map(
                |arg| match arg.as_expression().map(Expression::without_parentheses) {
                    Some(Expression::Identifier(id)) => id.name.as_str(),
                    _ => "",
                },
            )
            .collect();
        if args != params {
            return None;
        }

        match call.callee.without_parentheses() {
            Expression::Identifier(id) => Some(Shape::Forward {
                callee: self.at(id.span.start),
                fields: Vec::new(),
            }),
            Expression::StaticMemberExpression(member) => {
                let mut fields = Vec::new();
                let mut at = member.object.without_parentheses();
                while let Expression::StaticMemberExpression(inner) = at {
                    fields.push(self.at(inner.property.span.start));
                    at = inner.object.without_parentheses();
                }
                matches!(at, Expression::ThisExpression(_)).then(|| Shape::Forward {
                    callee: self.at(member.property.span.start),
                    fields,
                })
            }
            _ => None,
        }
    }

    /// `this.f`: the position of `f`.
    fn own(&self, expr: &Expression<'_>) -> Option<Pos> {
        match expr.without_parentheses() {
            Expression::StaticMemberExpression(member)
                if matches!(
                    member.object.without_parentheses(),
                    Expression::ThisExpression(_)
                ) =>
            {
                Some(self.at(member.property.span.start))
            }
            Expression::PrivateFieldExpression(member)
                if matches!(
                    member.object.without_parentheses(),
                    Expression::ThisExpression(_)
                ) =>
            {
                Some(self.at(member.field.span.start + 1))
            }
            _ => None,
        }
    }
}

fn params<'a>(function: &'a Function<'_>) -> Vec<&'a str> {
    function
        .params
        .items
        .iter()
        .map(|p| p.pattern.get_identifier_name().map_or("", |n| n.as_str()))
        .collect()
}

impl<'a> Visit<'a> for Reader {
    fn visit_import_declaration(&mut self, it: &ImportDeclaration<'a>) {
        self.facts.imports.push(
            self.lines
                .span(it.span.start as usize, it.span.end as usize),
        );
    }

    fn visit_export_named_declaration(&mut self, it: &ExportNamedDeclaration<'a>) {
        if it.source.is_some() {
            self.facts.imports.push(
                self.lines
                    .span(it.span.start as usize, it.span.end as usize),
            );
        } else {
            walk::walk_export_named_declaration(self, it);
        }
    }

    fn visit_export_all_declaration(&mut self, it: &ExportAllDeclaration<'a>) {
        self.facts.imports.push(
            self.lines
                .span(it.span.start as usize, it.span.end as usize),
        );
    }

    fn visit_ts_any_keyword(&mut self, _: &TSAnyKeyword) {
        if !self.facts.unknowns.contains(&ANY) {
            self.facts.unknowns.push(ANY);
        }
    }

    fn visit_class(&mut self, it: &Class<'a>) {
        self.class_members(it);
        walk::walk_class(self, it);
    }

    fn visit_function(&mut self, it: &Function<'a>, flags: ScopeFlags) {
        if let Some(id) = &it.id {
            let mut definition = Definition::new(
                self.at(id.span.start),
                self.lines
                    .span(it.span.start as usize, it.span.end as usize),
            );
            let params = params(it);
            definition.shape = it
                .body
                .as_ref()
                .and_then(|body| self.forwarder(body, &params));
            self.facts.definitions.push(definition);
        }

        self.with(Access::Borrow, |r| walk::walk_function(r, it, flags));
    }

    fn visit_expression(&mut self, it: &Expression<'a>) {
        match it {
            Expression::StaticMemberExpression(_)
            | Expression::ComputedMemberExpression(_)
            | Expression::PrivateFieldExpression(_)
            | Expression::CallExpression(_)
            | Expression::ParenthesizedExpression(_)
            | Expression::ChainExpression(_)
            | Expression::TSNonNullExpression(_)
            | Expression::AssignmentExpression(_)
            | Expression::UpdateExpression(_)
            | Expression::UnaryExpression(_) => walk::walk_expression(self, it),
            _ => self.with(Access::Borrow, |r| walk::walk_expression(r, it)),
        }
    }

    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        self.facts
            .members
            .push((self.at(it.property.span.start), self.ctx));
        self.with(Access::Reach, |r| r.visit_expression(&it.object));
    }

    fn visit_private_field_expression(&mut self, it: &PrivateFieldExpression<'a>) {
        self.facts
            .members
            .push((self.at(it.field.span.start + 1), self.ctx));
        self.with(Access::Reach, |r| r.visit_expression(&it.object));
    }

    fn visit_computed_member_expression(&mut self, it: &ComputedMemberExpression<'a>) {
        self.with(Access::Reach, |r| r.visit_expression(&it.object));
        self.with(Access::Borrow, |r| r.visit_expression(&it.expression));
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        match it.callee.without_parentheses() {
            Expression::StaticMemberExpression(member) => {
                self.facts
                    .members
                    .push((self.at(member.property.span.start), self.ctx));
                self.with(Access::Call, |r| r.visit_expression(&member.object));
            }
            Expression::PrivateFieldExpression(member) => {
                self.facts
                    .members
                    .push((self.at(member.field.span.start + 1), self.ctx));
                self.with(Access::Call, |r| r.visit_expression(&member.object));
            }
            callee => self.with(Access::Borrow, |r| r.visit_expression(callee)),
        }
        self.with(Access::Borrow, |r| {
            for arg in &it.arguments {
                r.visit_argument(arg);
            }
        });
    }

    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        self.with(Access::Reach, |r| r.visit_assignment_target(&it.left));
        self.with(Access::Borrow, |r| r.visit_expression(&it.right));
    }

    fn visit_update_expression(&mut self, it: &UpdateExpression<'a>) {
        self.with(Access::Reach, |r| {
            r.visit_simple_assignment_target(&it.argument);
        });
    }

    fn visit_unary_expression(&mut self, it: &UnaryExpression<'a>) {
        let ctx = if it.operator == UnaryOperator::Delete {
            Access::Reach
        } else {
            Access::Borrow
        };
        self.with(ctx, |r| r.visit_expression(&it.argument));
    }
}

fn plain_type(ty: &TSType<'_>) -> Plain {
    match ty {
        TSType::TSNumberKeyword(_)
        | TSType::TSStringKeyword(_)
        | TSType::TSBooleanKeyword(_)
        | TSType::TSBigIntKeyword(_)
        | TSType::TSLiteralType(_) => Plain::Scalar,
        TSType::TSArrayType(array) if plain_type(&array.element_type) != Plain::No => Plain::Data,
        TSType::TSTypeOperatorType(op) => match plain_type(&op.type_annotation) {
            Plain::No => Plain::No,
            _ => Plain::Data,
        },
        TSType::TSUnionType(union) => {
            let parts: Vec<Plain> = union
                .types
                .iter()
                .filter(|t| !matches!(t, TSType::TSNullKeyword(_) | TSType::TSUndefinedKeyword(_)))
                .map(plain_type)
                .collect();
            if parts.iter().all(|p| *p == Plain::Scalar) {
                Plain::Scalar
            } else if parts.iter().all(|p| *p != Plain::No) {
                Plain::Data
            } else {
                Plain::No
            }
        }
        _ => Plain::No,
    }
}

fn valued(value: &Expression<'_>) -> Plain {
    match value.without_parentheses() {
        Expression::NumericLiteral(_)
        | Expression::StringLiteral(_)
        | Expression::BooleanLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::TemplateLiteral(_) => Plain::Scalar,
        _ => Plain::No,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STORE: &str = "import { x } from './x';\nexport class Store {\n  private items: number[] = [];\n  limit = 3;\n  \
        next: Store | null = null;\n  push(v: number): void {\n    if (this.items.length < this.limit) this.items.push(v);\n  }\n  \
        get all(): number[] { return this.items; }\n  set cap(v: number) { this.limit = v; }\n  \
        add(v: number): void { this.next!.push(v); }\n  relay(v: number): void { this.inner.push(v); }\n}\n\
        export function run(s: Store): number {\n  s.all[0] = 2;\n  s.limit++;\n  return s.limit;\n}\n";

    fn find(line: &str, after: &str) -> Pos {
        let (row, text) = STORE
            .lines()
            .enumerate()
            .find(|(_, l)| l.contains(line))
            .expect("a line");
        Pos {
            line: u32::try_from(row).expect("small"),
            column: u32::try_from(text.find(after).expect("text") + after.len()).expect("small"),
        }
    }

    #[test]
    fn properties_methods_and_their_shapes() {
        let facts = read("store.ts", STORE).expect("parses");
        assert_eq!(facts.imports.len(), 1);
        assert_eq!(facts.unknowns, Vec::<&str>::new());
        let definition = |line: &str, after: &str| {
            let at = find(line, after);
            facts
                .definitions
                .iter()
                .find(|d| d.name == at)
                .cloned()
                .expect("a definition")
        };
        assert_eq!(definition("private items", "private ").plain, Plain::Data);
        assert_eq!(definition("limit = 3", "  ").plain, Plain::Scalar);
        assert_eq!(definition("next: Store", "  ").plain, Plain::No);
        assert_eq!(
            definition("get all()", "get ").shape,
            Some(Shape::Access {
                lent: vec![find("get all()", "return this.")],
                copied: vec![],
                to_read: true
            })
        );
        assert!(matches!(
            definition("set cap(", "set ").shape,
            Some(Shape::Access { to_read: false, .. })
        ));
        assert!(
            matches!(definition("relay(v", "  ").shape, Some(Shape::Forward { ref fields, .. }) if fields.len() == 1)
        );
        assert_eq!(definition("push(v: number): void {", "  ").shape, None);
        assert!(definition("push(v: number): void {", "  ").takes_self);
        assert!(!definition("export function run", "function ").takes_self);
    }

    #[test]
    fn what_code_does_with_a_property() {
        let facts = read("store.ts", STORE).expect("parses");
        let access = |line: &str, after: &str| {
            let at = find(line, after);
            facts
                .members
                .iter()
                .find(|(p, _)| *p == at)
                .map(|(_, a)| *a)
        };
        assert_eq!(access("if (this.items", "(this."), Some(Access::Reach));
        assert_eq!(access("if (this.items", "< this."), Some(Access::Borrow));
        assert_eq!(access("if (this.items", ") this."), Some(Access::Call));
        assert_eq!(access("s.all[0]", "s."), Some(Access::Reach));
        assert_eq!(access("s.limit++", "s."), Some(Access::Reach));
        assert_eq!(access("return s.limit", "s."), Some(Access::Borrow));
        assert!(
            read("a.js", "const a = b.c;\n")
                .expect("parses")
                .unknowns
                .contains(&UNTYPED)
        );
        assert!(read("a.ts", "const a = ;").is_none());
    }
}
