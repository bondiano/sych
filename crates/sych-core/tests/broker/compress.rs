//! Compression over budget: a supporting construct shrinks to its signature, and packets
//! that would overrun the run's input cap shrink the request budgets.

use sych_core::broker::{Limits, SIGNATURE_ONLY};

use super::{Reader, facts, limits, run};

/// A supporting construct that no longer fits keeps its first line: the model still sees the
/// signature it calls, and the full span is listed as `SIGNATURE_ONLY`.
#[test]
fn supporting_construct_over_budget_keeps_its_signature() {
    let reader = Reader::new();
    let (_, roomy) = run(limits(), &[facts(&reader)]);
    let construct = roomy[0].fragments[0].tokens;
    let tight = Limits {
        unit_tokens: construct + 40,
        request_tokens: construct + 40,
        ..limits()
    };
    let (_, packets) = run(tight, &[facts(&reader)]);
    let discovery = &packets[0];
    let signature = discovery
        .fragments
        .iter()
        .find(|f| f.transformation == "signature")
        .expect("a signature fragment");
    assert_eq!(signature.code_ref.start_line, signature.code_ref.end_line);
    assert!(discovery.excluded.iter().any(|e| e.reason == SIGNATURE_ONLY
        && e.code_ref.start_line == signature.code_ref.start_line
        && e.code_ref.end_line > e.code_ref.start_line));
}

#[test]
fn packets_over_the_run_cap_shrink_the_request_budgets() {
    let reader = Reader::new();
    let (_, packets) = run(limits(), &[facts(&reader)]);
    let shown: u32 = packets.iter().map(|p| p.usage.tokens).sum();
    assert_eq!(limits().fit_run(&packets, 2 * shown), None);

    let fitted = limits().fit_run(&packets, shown).expect("over the cap");
    assert_eq!(fitted.request_tokens, limits().request_tokens / 2);
    assert_eq!(fitted.unit_tokens, limits().unit_tokens / 2);
    let floor = limits().fit_run(&packets, 1).expect("over the cap");
    assert_eq!(floor.request_tokens, limits().request_tokens / 4);
}
