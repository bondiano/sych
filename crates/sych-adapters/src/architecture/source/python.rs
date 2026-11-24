//! Python facts from tree-sitter. A method's first parameter is its `self`; a special method
//! (`__eq__`, `__iter__`) implements a protocol, as a Rust trait's method does. Values are
//! shared, so a use of an attribute as a whole borrows it. What `getattr`, `setattr` and
//! `__dict__` reach no index resolves.

use tree_sitter::Node;

use super::{Access, Definition, Plain, Pos, Shape, SourceFacts, Span};
use crate::syntax::tree::{named, parse, text};

const SCALARS: [&str; 7] = ["bool", "bytes", "complex", "float", "int", "str", "None"];
const HOLDS_DATA: [&str; 9] = [
    "list",
    "List",
    "tuple",
    "Tuple",
    "frozenset",
    "Optional",
    "Sequence",
    "set",
    "Set",
];
/// Special methods that build the object rather than implement a protocol.
const BUILDERS: [&str; 4] = ["__init__", "__new__", "__post_init__", "__init_subclass__"];
const DYNAMIC: &str =
    "getattr, setattr and __dict__ reach attributes by name, which the index does not see";

pub(super) fn read(text: &str) -> Option<SourceFacts> {
    let tree = parse(text, &tree_sitter_python::LANGUAGE.into()).ok()?;
    let mut facts = SourceFacts::default();
    walk(tree.root_node(), text, &mut facts, None);
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

/// `receiver`: inside a method, its first parameter's name.
fn walk(node: Node<'_>, src: &str, facts: &mut SourceFacts, receiver: Option<&str>) {
    match node.kind() {
        "import_statement" | "import_from_statement" | "future_import_statement" => {
            facts.imports.push(span(node));
            return;
        }
        "function_definition" => {
            let in_class = node
                .parent()
                .and_then(|p| {
                    if p.kind() == "decorated_definition" {
                        p.parent()
                    } else {
                        Some(p)
                    }
                })
                .is_some_and(|block| {
                    block
                        .parent()
                        .is_some_and(|c| c.kind() == "class_definition")
                });
            let first = function(node, src, facts, in_class);
            if let Some(body) = node.child_by_field_name("body") {
                walk(body, src, facts, first);
            }

            return;
        }
        "expression_statement" if receiver.is_none() || in_class_body(node) => {
            class_attribute(node, src, facts);
        }
        "assignment" | "augmented_assignment" => {
            if let (Some(receiver), Some(left)) = (receiver, node.child_by_field_name("left"))
                && let Some(field) = own_field(left, src, receiver)
            {
                let mut definition = Definition::new(field, span(node));
                definition.plain = node
                    .child_by_field_name("type")
                    .map(|ty| annotated(ty, src))
                    .or_else(|| node.child_by_field_name("right").map(valued))
                    .unwrap_or(Plain::No);
                facts.definitions.push(definition);
            }
        }
        "attribute" => {
            if let Some(name) = node.child_by_field_name("attribute") {
                let access = match node.parent() {
                    Some(call)
                        if call.kind() == "call"
                            && call.child_by_field_name("function") == Some(node) =>
                    {
                        context(call)
                    }
                    _ => context(node),
                };
                facts.members.push((pos(name), access));
            }
        }
        "identifier"
            if matches!(text(node, src), "getattr" | "setattr" | "__dict__")
                && !facts.unknowns.contains(&DYNAMIC) =>
        {
            facts.unknowns.push(DYNAMIC);
        }
        _ => {}
    }
    for child in named(node) {
        walk(child, src, facts, receiver);
    }
}

fn in_class_body(node: Node<'_>) -> bool {
    node.parent().is_some_and(|block| {
        block
            .parent()
            .is_some_and(|c| c.kind() == "class_definition")
    })
}

/// `limit: int = 3` or `limit = 3` in a class body: a field.
fn class_attribute(node: Node<'_>, src: &str, facts: &mut SourceFacts) {
    if !in_class_body(node) {
        return;
    }

    let Some(assignment) = named(node)
        .into_iter()
        .next()
        .filter(|n| n.kind() == "assignment")
    else {
        return;
    };
    let Some(left) = assignment
        .child_by_field_name("left")
        .filter(|l| l.kind() == "identifier")
    else {
        return;
    };
    let mut definition = Definition::new(pos(left), span(node));
    definition.plain = assignment
        .child_by_field_name("type")
        .map(|ty| annotated(ty, src))
        .or_else(|| assignment.child_by_field_name("right").map(valued))
        .unwrap_or(Plain::No);
    facts.definitions.push(definition);
}

/// Records the function and returns the name of its receiver, if it is a method.
fn function<'s>(
    node: Node<'_>,
    src: &'s str,
    facts: &mut SourceFacts,
    in_class: bool,
) -> Option<&'s str> {
    let name = node.child_by_field_name("name")?;
    let extent = node
        .parent()
        .filter(|p| p.kind() == "decorated_definition")
        .unwrap_or(node);
    let decorators: Vec<&str> = if extent == node {
        Vec::new()
    } else {
        named(extent)
            .into_iter()
            .filter(|d| d.kind() == "decorator")
            .map(|d| text(d, src).trim_start_matches('@').trim())
            .collect()
    };
    let params: Vec<&str> = node
        .child_by_field_name("parameters")
        .map(|list| {
            named(list)
                .into_iter()
                .filter_map(|p| match p.kind() {
                    "identifier" => Some(text(p, src)),
                    "typed_parameter" | "default_parameter" | "typed_default_parameter" => p
                        .child_by_field_name("name")
                        .or_else(|| named(p).into_iter().next())
                        .map(|n| text(n, src)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let fn_name = text(name, src);
    let receiver = (in_class && !decorators.contains(&"staticmethod"))
        .then(|| params.first().copied())
        .flatten();
    let mut definition = Definition::new(pos(name), span(extent));
    definition.takes_self = receiver.is_some() && !decorators.contains(&"classmethod");
    definition.trait_impl = in_class
        && fn_name.starts_with("__")
        && fn_name.ends_with("__")
        && !BUILDERS.contains(&fn_name);
    definition.shape = node
        .child_by_field_name("body")
        .and_then(sole)
        .and_then(|stmt| shape(stmt, src, receiver, params.get(1..).unwrap_or_default()));
    facts.definitions.push(definition);
    receiver
}

/// The one statement of a body, past its docstring.
fn sole(body: Node<'_>) -> Option<Node<'_>> {
    let statements: Vec<Node<'_>> = named(body)
        .into_iter()
        .filter(|n| n.kind() != "comment")
        .collect();
    match statements.as_slice() {
        [one] => Some(*one),
        [doc, one]
            if doc.kind() == "expression_statement"
                && named(*doc).first().is_some_and(|s| s.kind() == "string") =>
        {
            Some(*one)
        }
        _ => None,
    }
}

fn shape(stmt: Node<'_>, src: &str, receiver: Option<&str>, params: &[&str]) -> Option<Shape> {
    let expr = match stmt.kind() {
        "return_statement" | "expression_statement" => *named(stmt).first()?,
        _ => return None,
    };
    if let Some(receiver) = receiver {
        if expr.kind() == "assignment" {
            let field = own_field(expr.child_by_field_name("left")?, src, receiver)?;
            let value = expr.child_by_field_name("right")?;
            return (value.kind() == "identifier" && params.contains(&text(value, src))).then(
                || Shape::Access {
                    lent: vec![field],
                    copied: Vec::new(),
                    to_read: false,
                },
            );
        }

        if let Some(field) = own_field(expr, src, receiver) {
            return Some(Shape::Access {
                lent: vec![field],
                copied: Vec::new(),
                to_read: true,
            });
        }
    }

    forward(expr, src, receiver, params)
}

fn forward(expr: Node<'_>, src: &str, receiver: Option<&str>, params: &[&str]) -> Option<Shape> {
    if expr.kind() != "call" {
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

    let function = expr.child_by_field_name("function")?;
    match function.kind() {
        "identifier" => Some(Shape::Forward {
            callee: pos(function),
            fields: Vec::new(),
        }),
        "attribute" => {
            let callee = function.child_by_field_name("attribute")?;
            let mut fields = Vec::new();
            let mut at = function.child_by_field_name("object")?;
            while at.kind() == "attribute" {
                fields.push(pos(at.child_by_field_name("attribute")?));
                at = at.child_by_field_name("object")?;
            }
            (at.kind() == "identifier" && Some(text(at, src)) == receiver).then_some(
                Shape::Forward {
                    callee: pos(callee),
                    fields,
                },
            )
        }
        _ => None,
    }
}

/// `self.f` for the receiver `self`: the position of `f`.
fn own_field(expr: Node<'_>, src: &str, receiver: &str) -> Option<Pos> {
    if expr.kind() != "attribute" {
        return None;
    }

    let object = expr.child_by_field_name("object")?;
    (object.kind() == "identifier" && text(object, src) == receiver)
        .then(|| expr.child_by_field_name("attribute").map(pos))
        .flatten()
}

/// What the code around `node` does with its value.
fn context(node: Node<'_>) -> Access {
    let Some(parent) = node.parent() else {
        return Access::Borrow;
    };
    let is = |field: &str| parent.child_by_field_name(field) == Some(node);
    match parent.kind() {
        "parenthesized_expression" | "pattern_list" | "tuple_pattern" => context(parent),
        "assignment" | "augmented_assignment" if is("left") => Access::Reach,
        "delete_statement" => Access::Reach,
        "subscript" if is("value") => Access::Reach,
        "attribute" if is("object") => match parent.parent() {
            Some(call)
                if call.kind() == "call"
                    && call.child_by_field_name("function") == Some(parent) =>
            {
                Access::Call
            }
            _ => Access::Reach,
        },
        _ => Access::Borrow,
    }
}

fn annotated(ty: Node<'_>, src: &str) -> Plain {
    let written = text(ty, src).replace(' ', "");
    plain_type(&written)
}

fn plain_type(written: &str) -> Plain {
    if SCALARS.contains(&written) {
        return Plain::Scalar;
    }

    let Some((holder, inner)) = written.strip_suffix(']').and_then(|w| w.split_once('[')) else {
        return if written.split('|').all(|part| SCALARS.contains(&part)) {
            Plain::Scalar
        } else {
            Plain::No
        };
    };
    let all = inner
        .split(',')
        .all(|part| part == "..." || plain_type(part) != Plain::No);
    if HOLDS_DATA.contains(&holder) && all {
        Plain::Data
    } else {
        Plain::No
    }
}

/// What a value written in the code holds: a literal is plain.
fn valued(value: Node<'_>) -> Plain {
    match value.kind() {
        "integer" | "float" | "true" | "false" | "none" | "string" | "concatenated_string" => {
            Plain::Scalar
        }
        "tuple" | "list" if !named(value).is_empty() => {
            if named(value).iter().all(|v| valued(*v) != Plain::No) {
                Plain::Data
            } else {
                Plain::No
            }
        }
        _ => Plain::No,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STORE: &str = "from a import b\n\nclass Store:\n    limit: int = 3\n\n    \
        def __init__(self, cap):\n        self.items = []\n        self.cap = cap\n        self.name: str = 'x'\n\n    \
        @property\n    def all(self):\n        \"\"\"Docs.\"\"\"\n        return self.items\n\n    \
        def set_cap(self, cap):\n        self.cap = cap\n\n    \
        def push(self, v):\n        self.items.append(v)\n\n    \
        def __len__(self):\n        return len(self.items)\n\n    \
        @staticmethod\n    def make():\n        return Store(1)\n\n\
        def run(s):\n    s.items.append(1)\n    s.items[0] = 2\n    n = len(s.items)\n    del s.cap\n";

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
    fn fields_methods_and_their_shapes() {
        let facts = read(STORE).expect("parses");
        assert_eq!(facts.imports.len(), 1);
        let plain = |line: &str, after: &str| {
            let at = find(line, after);
            facts
                .definitions
                .iter()
                .find(|d| d.name == at)
                .map(|d| d.plain)
        };
        assert_eq!(plain("limit: int", "    "), Some(Plain::Scalar));
        assert_eq!(plain("self.items = []", "self."), Some(Plain::No));
        assert_eq!(plain("self.name: str", "self."), Some(Plain::Scalar));
        let definition = |line: &str| {
            let at = find(line, "def ");
            facts
                .definitions
                .iter()
                .find(|d| d.name == at)
                .cloned()
                .expect("a function")
        };
        let all = definition("def all(");
        assert!(all.takes_self);
        assert_eq!(
            all.shape,
            Some(Shape::Access {
                lent: vec![find("return self.items", "self.")],
                copied: vec![],
                to_read: true
            })
        );
        assert!(matches!(
            definition("def set_cap(").shape,
            Some(Shape::Access { to_read: false, .. })
        ));
        // A call on a field passing the parameters on; whether it wraps anything depends on
        // what the index resolves `append` to.
        assert!(
            matches!(definition("def push(").shape, Some(Shape::Forward { ref fields, .. }) if fields.len() == 1)
        );
        assert!(definition("def __len__(").trait_impl);
        assert!(!definition("def __init__(").trait_impl);
        assert!(!definition("def make(").takes_self);
        assert!(!definition("def run(").takes_self);
    }

    #[test]
    fn what_code_does_with_an_attribute() {
        let facts = read(STORE).expect("parses");
        let access = |line: &str, after: &str| {
            let at = find(line, after);
            facts
                .members
                .iter()
                .find(|(p, _)| *p == at)
                .map(|(_, a)| *a)
        };
        assert_eq!(access("s.items.append(1)", "s."), Some(Access::Call));
        assert_eq!(access("s.items[0] = 2", "s."), Some(Access::Reach));
        assert_eq!(access("n = len(", "len(s."), Some(Access::Borrow));
        assert_eq!(access("del s.cap", "s."), Some(Access::Reach));
        assert_eq!(access("s.items.append(1)", "items."), Some(Access::Borrow));
    }
}
