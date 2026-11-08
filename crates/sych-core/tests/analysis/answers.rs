//! Answers to context requests: what the model is told about an answer it sees only in part.

use sych_core::SourceSide;
use sych_core::analysis::{AskOperation, Context, ContextAsk};
use sych_core::broker::{OpStatus, Ops};

use super::Reader;

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
