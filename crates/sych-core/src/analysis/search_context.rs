//! Search hits need nearby guards and consequences, not just the line naming a symbol.

use crate::{CodeRef, ReadError, SnapshotReader};

const RADIUS: u32 = 8;
const MAX_BYTES: usize = 4096;

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
