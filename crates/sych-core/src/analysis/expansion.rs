//! A further discovery round over one batch: the answers to the last round's context requests
//! are among the spans now, and the hypotheses already proposed are listed, so the round adds
//! new ones instead of restating them. Their claims are model text, so they go in a data block.

use super::Context;
use super::prompts::discovery;
use crate::model::{DataBlock, Prompt};
use crate::{Candidate, PlannedObligation};

pub const EXPANSION: &str = "discovery_expansion@1";

pub fn expansion(
    context: &Context,
    obligations: &[&PlannedObligation],
    unit: &str,
    proposed: &[Candidate],
) -> Prompt {
    let mut prompt = discovery(context, obligations, unit);
    prompt.template = EXPANSION.into();
    prompt.rules.push(
        "This is a further round of the same review: the answers to your earlier context \
         requests are now among the spans. Propose only hypotheses the list of proposed ones \
         does not hold; restating one in other words is not new. Ask for more context only for \
         a question the answers raised."
            .into(),
    );
    let listed: Vec<String> = proposed
        .iter()
        .map(|c| {
            let at: Vec<String> = c
                .anchors
                .iter()
                .map(|a| format!("{}:{}-{}", a.path, a.start_line, a.end_line))
                .collect();
            format!("- {} ({})", c.claim, at.join(", "))
        })
        .collect();
    prompt.data.insert(
        1,
        DataBlock {
            label: "hypotheses already proposed in this review; do not repeat them".into(),
            content: if listed.is_empty() {
                "none".into()
            } else {
                listed.join("\n")
            },
        },
    );
    prompt
}
