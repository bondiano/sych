//! Search hits need nearby guards and consequences, not just the line naming a symbol.

use super::context::side_name;
use crate::broker::{OpStatus, Operation, Ops};
use crate::{CodeRef, ReadError, SnapshotReader, SourceSide};

const RADIUS: u32 = 8;
const MAX_BYTES: usize = 4096;
/// Matches of the other side named by place; the rest are counted.
const OTHER_SIDE_SHOWN: usize = 3;
const OTHER_SIDE_LIMIT: usize = 10;

/// A small window from the same pinned blob. Shrink around long lines; if expansion fails,
/// keep the exact hit so auxiliary context never hides a search result.
pub(super) fn around(hit: &CodeRef, reader: &dyn SnapshotReader) -> CodeRef {
    let mut radius = RADIUS;
    while radius > 0 {
        let start = hit.start_line.saturating_sub(radius).max(1);
        let end = hit.end_line.saturating_add(radius);
        let reference = match reader.code_ref(hit.side, &hit.path, start, end) {
            Err(ReadError::LineRange { line_count, .. }) if line_count >= hit.end_line => {
                reader.code_ref(hit.side, &hit.path, start, end.min(line_count))
            }
            result => result,
        };
        let Ok(reference) = reference else {
            return hit.clone();
        };
        if reference.snapshot_id != hit.snapshot_id || reference.blob_oid != hit.blob_oid {
            return hit.clone();
        }

        match reader.read_span(&reference) {
            Ok(span) if span.text.len() <= MAX_BYTES => return reference,
            Ok(_) => radius /= 2,
            Err(_) => return hit.clone(),
        }
    }
    hit.clone()
}

fn described(side: SourceSide) -> String {
    let what = match side {
        SourceSide::Head => "this change",
        SourceSide::DiffBase => "before this change",
        SourceSide::TargetTip => "the target branch, without this change",
    };
    format!("{} ({what})", side_name(side))
}

/// The side a search ran on and what the same literal finds on the other side of the change,
/// so absence on one side never reads as absence on both: BASE never has what the change adds.
/// `found` is how many matches the searched side gave.
pub(super) fn other_side(ops: &Ops<'_>, side: SourceSide, query: &str, found: usize) -> String {
    let other = match side {
        SourceSide::Head => SourceSide::DiffBase,
        SourceSide::DiffBase | SourceSide::TargetTip => SourceSide::Head,
    };
    let result = ops.run(&Operation::SearchSource {
        query: query.to_owned(),
        paths: Vec::new(),
        side: other,
        limit: OTHER_SIDE_LIMIT,
    });
    let searched = format!("searched {}", described(side));
    let n = result.refs.len();
    let places: Vec<String> = result
        .refs
        .iter()
        .take(OTHER_SIDE_SHOWN)
        .map(|r| format!("{}:{}", r.path, r.start_line))
        .collect();
    let more = if n > OTHER_SIDE_SHOWN {
        format!(" and {} more", n - OTHER_SIDE_SHOWN)
    } else {
        String::new()
    };
    let noun = if n == 1 { "match" } else { "matches" };
    let count = match result.status {
        OpStatus::CompleteForQuery => format!("{n} {noun}"),
        OpStatus::Partial => format!("at least {n} {noun}"),
        status => return format!("{searched}; {} not searched ({status:?})", described(other)),
    };
    if n == 0 {
        return format!("{searched}; {} has no matches", described(other));
    }

    let meaning = match (found, other) {
        (0, SourceSide::Head) => ": the change adds them",
        (0, _) => ": the change removes them",
        _ => "",
    };
    format!(
        "{searched}; {} has {count}: {}{more}{meaning}",
        described(other),
        places.join(", ")
    )
}
