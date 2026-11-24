use super::*;

fn facts(text: &str) -> SourceFacts {
    read(text).expect("parses")
}

fn at(line: u32, column: u32) -> Pos {
    Pos { line, column }
}

fn access(facts: &SourceFacts, line: u32, column: u32) -> Option<Access> {
    facts
        .members
        .iter()
        .find(|(pos, _)| *pos == at(line, column))
        .map(|(_, access)| *access)
}

/// The column of the name `after` ends with, on the line of `source` that holds `line`.
fn column_of(source: &str, line: &str, after: &str) -> (u32, u32) {
    let (row, text) = source
        .lines()
        .enumerate()
        .find(|(_, l)| l.contains(line))
        .expect("a line");
    let at = text.find(after).expect("the text") + after.len();
    (
        u32::try_from(row).expect("small"),
        u32::try_from(at).expect("small"),
    )
}

#[test]
fn what_code_does_with_a_field_it_names() {
    let source = "fn f(q: &mut Q) {\n    let a = q.head + 1;\n    q.head = 0;\n    \
                  let b = &q.items;\n    q.items.push(1);\n    let c = q.name.clone();\n    \
                  let d = &mut q.items;\n    let e = q.items[0];\n    println!(\"{}\", q.size);\n}\n";
    let f = facts(source);
    for (line, after, expected) in [
        ("let a", "q.", Access::Copy),
        ("q.head = 0", "q.", Access::Reach),
        ("let b", "&q.", Access::Borrow),
        ("push(1)", "q.", Access::Call),
        ("let c", "q.", Access::Copy),
        ("let d", "mut q.", Access::Reach),
        ("let e", "q.", Access::Reach),
        ("println", "q.", Access::Copy),
    ] {
        let (row, column) = column_of(source, line, after);
        assert_eq!(access(&f, row, column), Some(expected), "{line}");
    }
}

#[test]
fn getters_setters_and_forwarders_are_shapes_and_working_code_is_not() {
    let source = "struct Q { head: usize, items: Vec<u32>, tag: PhantomData<u8>, inner: Inner }\n\
                  impl Q {\n    fn head(&self) -> usize { self.head }\n    \
                  fn items(&self) -> &Vec<u32> { &self.items }\n    \
                  fn items_mut(&mut self) -> &mut Vec<u32> { &mut self.items }\n    \
                  fn set_head(&mut self, head: usize) { self.head = head; }\n    \
                  fn push(&mut self, v: u32) { self.inner.push(v) }\n    \
                  fn len(&self) -> usize { self.items.len() + 1 }\n}\n\
                  impl Display for Q { fn fmt(&self, f: &mut F) -> R { todo!() } }\n";
    let f = facts(source);
    let shape = |name: &str| {
        let (row, column) = column_of(source, &format!("fn {name}("), "fn ");
        f.definitions
            .iter()
            .find(|d| d.name == at(row, column))
            .and_then(|d| d.shape.clone())
    };
    let (row, column) = column_of(source, "fn head(", "self.");
    assert_eq!(
        shape("head"),
        Some(Shape::Access {
            lent: vec![],
            copied: vec![at(row, column)],
            to_read: true
        })
    );
    assert!(
        matches!(shape("items"), Some(Shape::Access { to_read: true, ref lent, .. }) if lent.len() == 1)
    );
    assert!(matches!(
        shape("items_mut"),
        Some(Shape::Access { to_read: false, .. })
    ));
    assert!(matches!(
        shape("set_head"),
        Some(Shape::Access { to_read: false, .. })
    ));
    assert!(matches!(shape("push"), Some(Shape::Forward { ref fields, .. }) if fields.len() == 1));
    assert_eq!(shape("len"), None);
    let plain: Vec<Plain> = f.definitions.iter().take(4).map(|d| d.plain).collect();
    assert_eq!(
        plain,
        [Plain::Scalar, Plain::Data, Plain::Nothing, Plain::No]
    );
    assert!(f.definitions.iter().any(|d| d.trait_impl));
}

#[test]
fn test_code_imports_and_unexpanded_macros_are_found() {
    let source = "use crate::a::B;\n#[derive(Debug, serde::Serialize)]\nstruct S;\n\
                  #[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n";
    let f = facts(source);
    assert_eq!(f.imports.len(), 1);
    assert_eq!(f.tests.first().map(|s| s.start.line), Some(3));
    assert_eq!(f.unknowns, [MACROS]);
    assert!(read("fn (").is_none());
}
