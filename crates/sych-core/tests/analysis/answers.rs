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

fn ask_for(query: &str) -> ContextAsk {
    ContextAsk {
        question: "where?".into(),
        operation: AskOperation::SearchSource {
            side: SourceSide::Head,
            query: query.into(),
        },
    }
}

#[test]
fn a_hit_is_shown_with_the_lines_around_it_as_far_as_the_file_goes() {
    let reader = Reader::new();
    let ops = Ops {
        generated: &[],
        facts: &[],
        evidence: &[],
        reader: &reader,
    };
    let mut context = Context::default();
    let answered = ask_for("total").answer(&ops, &mut context, 1);
    assert_eq!(answered.refs, 1);
    let block = context
        .blocks
        .iter()
        .find(|b| b.label.starts_with("ref-1 "))
        .expect("the hit's span");
    let first = block.content.lines().next().expect("a line");
    assert!(first.trim_start().starts_with("1 |"), "{}", block.content);
    assert!(block.content.lines().count() > 1, "{}", block.content);
}
