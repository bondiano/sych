//! Which cited lines count as shown code.

use sych_core::SourceSide;

use super::{Reader, anchor, context_of};

/// A model that names the neighbouring span of lines it was shown still cites shown code;
/// lines no span of that file and side shows stay rejected.
#[test]
fn lines_shown_by_another_span_of_the_file_resolve_through_it() {
    let reader = Reader::new();
    let (context, _) = context_of(
        &reader,
        &[
            reader.at(SourceSide::Head, "src/order.ts", 1, 1),
            reader.at(SourceSide::Head, "src/order.ts", 3, 3),
            reader.at(SourceSide::DiffBase, "src/order.ts", 1, 6),
        ],
    );
    assert_eq!(
        context.anchor(&anchor("ref-1", 3, 3), &reader),
        Ok(reader.at(SourceSide::Head, "src/order.ts", 3, 3))
    );
    for (at, problem) in [
        (anchor("ref-1", 2, 2), "lines 2-2 are outside ref-1 (1-1)"),
        (anchor("ref-1", 1, 3), "lines 1-3 are outside ref-1 (1-1)"),
        (anchor("ref-1", 5, 5), "lines 5-5 are outside ref-1 (1-1)"),
    ] {
        assert_eq!(context.anchor(&at, &reader), Err(problem.to_owned()));
    }
}
