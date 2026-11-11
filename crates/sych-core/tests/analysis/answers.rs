//! Answers to context requests: what the model is told about an answer it sees only in part.

use sych_core::analysis::{AskOperation, Context, ContextAsk};
use sych_core::broker::{OpStatus, Ops};
use sych_core::{SnapshotReader, SourceSide};

use super::Reader;

#[test]
fn a_search_hit_shows_the_guard_and_return_on_neighbouring_lines() {
    let reader = Reader::new();
    let ops = Ops {
        generated: &[],
        facts: &[],
        evidence: &[],
        reader: &reader,
    };
    let ask = ContextAsk {
        question: "does total guard missing items?".into(),
        operation: AskOperation::SearchSource {
            side: SourceSide::DiffBase,
            query: "function total".into(),
        },
    };
    let mut context = Context::default();
    let answered = ask.answer(&ops, &mut context, 3);
    assert_eq!(answered.refs, 1);
    let span = context.span("ref-1").expect("search span");
    let text = reader.read_span(span).expect("hash checked").text;
    assert!(text.contains("if (!items)"), "{text}");
    assert!(text.contains("return 0;"), "{text}");
    assert_eq!(span.side, SourceSide::DiffBase);
    assert_eq!((span.start_line, span.end_line), (1, 6));
}

#[test]
fn read_lines_stays_exact_instead_of_expanding() {
    let reader = Reader::new();
    let ops = Ops {
        generated: &[],
        facts: &[],
        evidence: &[],
        reader: &reader,
    };
    let ask = ContextAsk {
        question: "one line".into(),
        operation: AskOperation::ReadLines {
            side: SourceSide::DiffBase,
            path: super::path("src/order.ts"),
            start_line: 2,
            end_line: 2,
        },
    };
    let mut context = Context::default();
    ask.answer(&ops, &mut context, 3);
    let span = context.span("ref-1").expect("exact span");
    assert_eq!((span.start_line, span.end_line), (2, 2));
}

#[test]
fn an_answer_cut_to_fewer_refs_says_it_is_partial() {
    let reader = Reader::new();
    let ask = ContextAsk {
        question: "who totals?".into(),
        operation: AskOperation::SearchSource {
            side: SourceSide::Head,
            query: "total".into(),
        },
    };
    let ops = Ops {
        generated: &[],
        facts: &[],
        evidence: &[],
        reader: &reader,
    };
    let mut whole = Context::default();
    let all = ask.answer(&ops, &mut whole, 10);
    assert!(all.refs > 1, "{all:?}");
    let mut cut = Context::default();
    let answered = ask.answer(&ops, &mut cut, 1);
    assert_eq!(answered.refs, 1);
    assert_ne!(answered.status, Some(OpStatus::CompleteForQuery));
    let shown = format!("1 of {} matches shown", all.refs);
    assert!(answered.limitations.contains(&shown), "{answered:?}");
    let block = cut.blocks.last().expect("status block");
    assert!(block.content.contains(&shown), "{}", block.content);
}

#[test]
fn a_whole_search_answer_says_it_never_matched_paths() {
    let reader = Reader::new();
    let ops = Ops {
        generated: &[],
        facts: &[],
        evidence: &[],
        reader: &reader,
    };
    let search = |query: &str| ContextAsk {
        question: "is it there?".into(),
        operation: AskOperation::SearchSource {
            side: SourceSide::Head,
            query: query.into(),
        },
    };
    for query in ["total", "no-such-text-anywhere"] {
        let mut context = Context::default();
        let answered = search(query).answer(&ops, &mut context, 10);
        assert_eq!(
            answered.status,
            Some(OpStatus::CompleteForQuery),
            "{answered:?}"
        );
        let block = context.blocks.last().expect("status block");
        assert!(
            block
                .content
                .contains("in file contents (paths are not searched)"),
            "{query}: {}",
            block.content
        );
    }
}

#[test]
fn a_search_says_where_it_looked_and_what_the_other_side_has() {
    let reader = Reader::new();
    let ops = Ops {
        generated: &[],
        facts: &[],
        evidence: &[],
        reader: &reader,
    };
    let answer = |side, query: &str| {
        let ask = ContextAsk {
            question: "is it used?".into(),
            operation: AskOperation::SearchSource {
                side,
                query: query.into(),
            },
        };
        let mut context = Context::default();
        ask.answer(&ops, &mut context, 10);
        context.blocks.last().expect("status block").content.clone()
    };
    // Only the change adds it: a BASE search alone would read as "unused" (#160).
    let added = answer(SourceSide::DiffBase, "items!.");
    assert!(added.starts_with("no matches"), "{added}");
    assert!(
        added.contains(
            "searched BASE (before this change); HEAD (this change) has 1 match: \
             src/order.ts:2: the change adds them"
        ),
        "{added}"
    );
    let removed = answer(SourceSide::Head, "if (!items)");
    assert!(
        removed.contains(
            "BASE (before this change) has 1 match: src/order.ts:2: the change removes them"
        ),
        "{removed}"
    );
    let kept = answer(SourceSide::Head, "export function total");
    assert!(
        kept.contains(
            "searched HEAD (this change); BASE (before this change) has 1 match: src/order.ts:1"
        ) && !kept.contains("the change"),
        "{kept}"
    );
    let nowhere = answer(SourceSide::TargetTip, "no-such-text");
    assert!(
        nowhere.contains("searched TARGET_TIP (the target branch, without this change); HEAD (this change) has no matches"),
        "{nowhere}"
    );
}
