//! A changed line no discovery packet shows keeps coverage partial: a review cannot call
//! itself complete over code no call saw.

use sych_core::PathGap;
use sych_core::broker::{EvidencePacket, UNSHOWN_CHANGE, unshown_changes};

use super::{Reader, facts, limits, manifest, path, run};

#[test]
fn every_added_line_a_discovery_packet_misses_is_a_gap() {
    let reader = Reader::new();
    let (mut plan, mut packets) = run(limits(), &[facts(&reader)]);
    let change = manifest();
    let none: [String; 0] = [];
    assert_eq!(
        unshown_changes(&change, &plan, &packets),
        none,
        "the hunk src/store.rs:6-7 is shown"
    );

    // Each discovery fragment of the hunk keeps only its first line, as if the rest were cut.
    let discovery = |p: &&mut EvidencePacket| p.request.obligation_id.is_none();
    for f in packets
        .iter_mut()
        .filter(discovery)
        .flat_map(|p| &mut p.fragments)
    {
        if f.code_ref.path == path("src/store.rs") && f.code_ref.start_line <= 6 {
            f.code_ref.end_line = f.code_ref.end_line.min(6);
        }
    }
    assert_eq!(
        unshown_changes(&change, &plan, &packets),
        [format!("{UNSHOWN_CHANGE}: src/store.rs:7-7")]
    );

    for p in packets.iter_mut().filter(discovery) {
        p.fragments
            .retain(|f| f.code_ref.path != path("src/store.rs"));
    }
    assert_eq!(
        unshown_changes(&change, &plan, &packets),
        [format!("{UNSHOWN_CHANGE}: src/store.rs:6-7")]
    );

    // A generated or already unreviewed file is the plan's to report.
    let mut generated = change.clone();
    generated.files[0].generated = true;
    assert_eq!(unshown_changes(&generated, &plan, &packets), none);
    let gap = PathGap {
        path: path("src/store.rs"),
        reason: "LIMIT_MAX_SOURCE_FILES".into(),
    };
    let mut excluded = plan.clone();
    excluded.scope.excluded.push(gap.clone());
    assert_eq!(unshown_changes(&change, &excluded, &packets), none);
    plan.unreviewed.push(gap);
    assert_eq!(unshown_changes(&change, &plan, &packets), none);
}
