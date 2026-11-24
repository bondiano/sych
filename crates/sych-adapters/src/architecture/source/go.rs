//! Go facts from tree-sitter. A method's receiver is its `self`; values are shared, so a use
//! of a field as a whole borrows it, and a selector of a selector, an index, `&` or an
//! assignment reaches into it. `reflect` reads fields no index resolves.

use tree_sitter::Node;

use super::{Access, Definition, Plain, Pos, Shape, SourceFacts, Span};
use crate::syntax::tree::{named, parse, text};

const SCALARS: [&str; 19] = [
    "bool",
    "byte",
    "complex128",
    "complex64",
    "float32",
    "float64",
    "int",
    "int16",
    "int32",
    "int64",
    "int8",
    "rune",
    "string",
    "uint",
    "uint16",
    "uint32",
    "uint64",
    "uint8",
    "uintptr",
];
const REFLECTION: &str = "reflect reads and writes fields by name, which the index does not see";

pub(super) fn read(text: &str) -> Option<SourceFacts> {
    let tree = parse(text, &tree_sitter_go::LANGUAGE.into()).ok()?;
    let mut facts = SourceFacts::default();
    walk(tree.root_node(), text, &mut facts);
    Some(facts)
}

fn pos(node: Node<'_>) -> Pos {
    let at = node.start_position();
    Pos {
        line: u32::try_from(at.row).unwrap_or(u32::MAX),
        column: u32::try_from(at.column).unwrap_or(u32::MAX),
    }
}

fn span(node: Node<'_>) -> Span {
    let end = node.end_position();
    Span {
        start: pos(node),
        end: Pos {
            line: u32::try_from(end.row).unwrap_or(u32::MAX),
            column: u32::try_from(end.column).unwrap_or(u32::MAX),
        },
    }
}

fn walk(node: Node<'_>, src: &str, facts: &mut SourceFacts) {
    match node.kind() {
        "import_declaration" => {
            facts.imports.push(span(node));
            if text(node, src).contains("\"reflect\"") && !facts.unknowns.contains(&REFLECTION) {
                facts.unknowns.push(REFLECTION);
            }

            return;
        }
        "function_declaration" | "method_declaration" => function(node, src, facts),
        "field_declaration" => {
            let plain = node
                .child_by_field_name("type")
                .map_or(Plain::No, |ty| plain(ty, src));
            let mut cursor = node.walk();
            for name in node.children_by_field_name("name", &mut cursor) {
                let mut definition = Definition::new(pos(name), span(node));
                definition.plain = plain;
                facts.definitions.push(definition);
            }
        }
        "selector_expression" => {
            if let Some(field) = node.child_by_field_name("field") {
                let access = match node.parent() {
                    Some(call)
                        if call.kind() == "call_expression"
                            && call.child_by_field_name("function") == Some(node) =>
                    {
                        context(call)
                    }
                    _ => context(node),
                };
                facts.members.push((pos(field), access));
            }
        }
        _ => {}
    }
    for child in named(node) {
        walk(child, src, facts);
    }
}

fn function(node: Node<'_>, src: &str, facts: &mut SourceFacts) {
    let Some(name) = node.child_by_field_name("name") else {
        return;
    };
    let mut definition = Definition::new(pos(name), span(node));
    let receiver = node
        .child_by_field_name("receiver")
        .and_then(|list| named(list).into_iter().next())
        .and_then(|param| param.child_by_field_name("name"))
        .map(|name| text(name, src));
    definition.takes_self = node.kind() == "method_declaration";
    let params: Vec<&str> = node
        .child_by_field_name("parameters")
        .map(|list| {
            named(list)
                .into_iter()
                .flat_map(|param| {
                    let mut cursor = param.walk();
                    param
                        .children_by_field_name("name", &mut cursor)
                        .map(|n| text(n, src))
                        .collect::<Vec<_>>()
                })
                .collect()
        })
        .unwrap_or_default();
    definition.shape = node
        .child_by_field_name("body")
        .and_then(|body| sole(body))
        .and_then(|stmt| shape(stmt, src, receiver, &params));
    facts.definitions.push(definition);
}

/// The one statement of a body.
fn sole(body: Node<'_>) -> Option<Node<'_>> {
    let statements: Vec<Node<'_>> = named(body)
        .into_iter()
        .filter(|n| n.kind() != "comment")
        .collect();
    match statements.as_slice() {
        [list] if list.kind() == "statement_list" => {
            let inner: Vec<Node<'_>> = named(*list)
                .into_iter()
                .filter(|n| n.kind() != "comment")
                .collect();
            match inner.as_slice() {
                [one] => Some(*one),
                _ => None,
            }
        }
        [one] => Some(*one),
        _ => None,
    }
}

fn shape(stmt: Node<'_>, src: &str, receiver: Option<&str>, params: &[&str]) -> Option<Shape> {
    let expr = match stmt.kind() {
        "return_statement" => match named(stmt).as_slice() {
            [list] if list.kind() == "expression_list" => match named(*list).as_slice() {
                [one] => *one,
                _ => return None,
            },
            [one] => *one,
            _ => return None,
        },
        "expression_statement" => *named(stmt).first()?,
        "assignment_statement" => {
            let left = single(stmt.child_by_field_name("left")?)?;
            let right = single(stmt.child_by_field_name("right")?)?;
            let field = own_field(left, src, receiver?)?;
            return (right.kind() == "identifier" && params.contains(&text(right, src))).then(
                || Shape::Access {
                    lent: vec![field],
                    copied: Vec::new(),
                    to_read: false,
                },
            );
        }
        _ => return None,
    };
    let expr = unwrapped(expr);
    if let Some(receiver) = receiver {
        let field = match expr.kind() {
            "unary_expression"
                if expr.child_by_field_name("operator").map(|o| text(o, src)) == Some("&") =>
            {
                expr.child_by_field_name("operand")
                    .and_then(|operand| own_field(unwrapped(operand), src, receiver))
            }
            _ => own_field(expr, src, receiver),
        };
        if let Some(field) = field {
            return Some(Shape::Access {
                lent: vec![field],
                copied: Vec::new(),
                to_read: true,
            });
        }
    }

    forward(expr, src, receiver, params)
}

/// One call given `params` in order, of a function or of a method on the receiver or its
/// fields.
fn forward(expr: Node<'_>, src: &str, receiver: Option<&str>, params: &[&str]) -> Option<Shape> {
    if expr.kind() != "call_expression" {
        return None;
    }

    let args: Vec<&str> = named(expr.child_by_field_name("arguments")?)
        .into_iter()
        .map(|arg| {
            if arg.kind() == "identifier" {
                text(arg, src)
            } else {
                ""
            }
        })
        .collect();
    if args != params {
        return None;
    }

    let function = unwrapped(expr.child_by_field_name("function")?);
    match function.kind() {
        "identifier" => Some(Shape::Forward {
            callee: pos(function),
            fields: Vec::new(),
        }),
        "selector_expression" => {
            let callee = function.child_by_field_name("field")?;
            let mut fields = Vec::new();
            let mut at = unwrapped(function.child_by_field_name("operand")?);
            while at.kind() == "selector_expression" {
                fields.push(pos(at.child_by_field_name("field")?));
                at = unwrapped(at.child_by_field_name("operand")?);
            }
            let on_receiver = at.kind() == "identifier" && Some(text(at, src)) == receiver;
            // A package's function, `strings.Join(a, b)`, is called on no value.
            (on_receiver || (fields.is_empty() && at.kind() == "identifier")).then_some(
                Shape::Forward {
                    callee: pos(callee),
                    fields,
                },
            )
        }
        _ => None,
    }
}

/// `r.f` for the receiver `r`: the position of `f`.
fn own_field(expr: Node<'_>, src: &str, receiver: &str) -> Option<Pos> {
    if expr.kind() != "selector_expression" {
        return None;
    }

    let operand = unwrapped(expr.child_by_field_name("operand")?);
    (operand.kind() == "identifier" && text(operand, src) == receiver)
        .then(|| expr.child_by_field_name("field").map(pos))
        .flatten()
}

fn single(list: Node<'_>) -> Option<Node<'_>> {
    match named(list).as_slice() {
        [one] => Some(unwrapped(*one)),
        _ => None,
    }
}

fn unwrapped(node: Node<'_>) -> Node<'_> {
    if node.kind() == "parenthesized_expression"
        && let Some(inner) = named(node).into_iter().next()
    {
        return unwrapped(inner);
    }

    node
}

/// What the code around `node` does with its value.
fn context(node: Node<'_>) -> Access {
    let Some(parent) = node.parent() else {
        return Access::Borrow;
    };
    let is = |field: &str| parent.child_by_field_name(field) == Some(node);
    match parent.kind() {
        "parenthesized_expression" => context(parent),
        "expression_list" => match parent.parent() {
            Some(statement)
                if matches!(statement.kind(), "assignment_statement")
                    && statement.child_by_field_name("left") == Some(parent) =>
            {
                Access::Reach
            }
            _ => Access::Borrow,
        },
        "inc_statement" | "dec_statement" => Access::Reach,
        "unary_expression" => {
            let operator = parent.child(0).map(|o| o.kind());
            if operator == Some("&") {
                Access::Reach
            } else {
                Access::Borrow
            }
        }
        "index_expression" | "slice_expression" if is("operand") => Access::Reach,
        "selector_expression" if is("operand") => match parent.parent() {
            Some(call)
                if call.kind() == "call_expression"
                    && call.child_by_field_name("function") == Some(parent) =>
            {
                Access::Call
            }
            _ => Access::Reach,
        },
        _ => Access::Borrow,
    }
}

fn plain(ty: Node<'_>, src: &str) -> Plain {
    let data = |is: bool| if is { Plain::Data } else { Plain::No };
    match ty.kind() {
        "type_identifier" if SCALARS.contains(&text(ty, src)) => Plain::Scalar,
        "slice_type" | "array_type" => ty
            .child_by_field_name("element")
            .map_or(Plain::No, |element| data(plain(element, src) != Plain::No)),
        "struct_type" if named(ty).iter().all(|n| named(*n).is_empty()) => Plain::Nothing,
        _ => Plain::No,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STORE: &str = "package store\n\nimport \"fmt\"\n\ntype Store struct {\n\titems []int\n\tlimit int\n\tnext *Store\n}\n\n\
        func (s *Store) Items() []int { return s.items }\n\n\
        func (s *Store) SetLimit(limit int) { s.limit = limit }\n\n\
        func (s *Store) Push(v int) { s.next.Push(v) }\n\n\
        func (s *Store) Grow(v int) {\n\tif len(s.items) < s.limit {\n\t\ts.items = append(s.items, v)\n\t}\n\ts.next.items[0]++\n\tfmt.Println(s.limit)\n}\n";

    fn find(source: &str, line: &str, after: &str) -> Pos {
        let (row, text) = source
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
    fn fields_methods_and_their_shapes() {
        let facts = read(STORE).expect("parses");
        assert_eq!(facts.imports.len(), 1);
        let plain: Vec<Plain> = facts.definitions.iter().take(3).map(|d| d.plain).collect();
        assert_eq!(plain, [Plain::Data, Plain::Scalar, Plain::No]);
        let shape = |name: &str| {
            let at = find(STORE, &format!(") {name}("), ") ");
            facts
                .definitions
                .iter()
                .find(|d| d.name == at)
                .map(|d| (d.takes_self, d.shape.clone()))
        };
        let items = find(STORE, ") Items(", "return s.");
        assert_eq!(
            shape("Items"),
            Some((
                true,
                Some(Shape::Access {
                    lent: vec![items],
                    copied: vec![],
                    to_read: true
                })
            ))
        );
        assert!(matches!(
            shape("SetLimit"),
            Some((true, Some(Shape::Access { to_read: false, .. })))
        ));
        assert!(
            matches!(shape("Push"), Some((true, Some(Shape::Forward { ref fields, .. }))) if fields.len() == 1)
        );
        assert_eq!(shape("Grow"), Some((true, None)));
    }

    #[test]
    fn what_code_does_with_a_field() {
        let facts = read(STORE).expect("parses");
        let access = |line: &str, after: &str| {
            let at = find(STORE, line, after);
            facts
                .members
                .iter()
                .find(|(p, _)| *p == at)
                .map(|(_, a)| *a)
        };
        assert_eq!(access("if len(", "len(s."), Some(Access::Borrow));
        assert_eq!(access("s.items = append", "\ts."), Some(Access::Reach));
        assert_eq!(access("s.next.items[0]", "\ts."), Some(Access::Reach));
        assert_eq!(access("s.next.items[0]", "next."), Some(Access::Reach));
        assert_eq!(access("func (s *Store) Push", "{ s."), Some(Access::Call));
        assert_eq!(access("fmt.Println", "Println(s."), Some(Access::Borrow));
    }
}
