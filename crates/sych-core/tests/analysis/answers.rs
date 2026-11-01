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
