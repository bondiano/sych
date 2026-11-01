//! What a model sees of a packet: numbered source spans under local ids (`ref-N`), evidence
//! under `ev-N`, and nothing it could use to name coordinates the harness did not hand out.

use std::collections::BTreeMap;
use std::fmt::Write;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::broker::{EvidencePacket, Priority, REQUIRED_OVER_BUDGET};
use crate::model::DataBlock;
use crate::{
    BindingKind, CodeRef, Completeness, DiffManifest, Evidence, EvidenceBinding, EvidenceId,
    EvidenceKind, FileChange, Producer, ReviewLanguage, SchemaVersion, SnapshotReader, SourceSide,
    SourceSnapshot, Timestamp, TrustClass, Validity, canonical_sha256, derived_uuid,
};

/// Removed lines of one test or CI file shown verbatim; the rest is only counted.
const MAX_REMOVED_LINES: u32 = 40;
/// Generated files named in one prompt; the rest are counted.
const MAX_HIDDEN_LISTED: usize = 50;

/// `old -> new STATUS: N lines removed, M added;` of one changed file.
fn change_line(file: &FileChange) -> String {
    let mut names: Vec<&str> = file
        .old
        .iter()
        .chain(&file.new)
        .map(|s| s.path.as_str())
        .collect();
    names.dedup();
    let removed: u32 = file.hunks.iter().map(|h| h.old_lines).sum();
    let added: u32 = file.hunks.iter().map(|h| h.new_lines).sum();
    format!(
        "{} {:?}: {removed} lines removed, {added} added;",
        names.join(" -> "),
        file.status
    )
}

/// Lines inside one handed-out span; the model never writes paths or hashes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    pub ref_id: String,
    pub start_line: u32,
    pub end_line: u32,
}

/// The source spans of one packet, as read by the harness: complete for exactly those spans
/// when no required span was left out, otherwise partial.
pub fn packet_evidence(
    packet: &EvidencePacket,
    snapshot: &SourceSnapshot,
    observed_at: Timestamp,
) -> Result<Evidence, serde_json::Error> {
    let required_missing = packet
        .excluded
        .iter()
        .any(|e| e.reason == REQUIRED_OVER_BUDGET);
    let digest = canonical_sha256(packet)?;
    Ok(Evidence {
        schema_version: SchemaVersion,
        id: EvidenceId(derived_uuid(&["packet_source", &digest.to_string()])),
        kind: EvidenceKind::Source,
        source_snapshot_id: snapshot.id.clone(),
        producer: Producer {
            name: "sych-broker".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            config_hash: None,
        },
        binding: EvidenceBinding {
            kind: BindingKind::Source,
            head_oid: Some(snapshot.head.clone()),
            target_tip_oid: Some(snapshot.target_tip.clone()),
            tested_oid: None,
            environment_id: None,
        },
        validity: Validity::Valid,
        completeness: if required_missing {
            Completeness::Partial
        } else {
            Completeness::CompleteForQuery
        },
        trust_class: TrustClass::SourceObservation,
        artifact_sha256: digest,
        code_refs: packet
            .fragments
            .iter()
            .map(|f| f.code_ref.clone())
            .collect(),
        limitations: packet.gaps.clone(),
        observed_at,
    })
}

/// Local ids handed to one prompt and the spans behind them.
#[derive(Debug, Default)]
pub struct Context {
    pub blocks: Vec<DataBlock>,
    refs: BTreeMap<String, CodeRef>,
    evidence: Vec<(EvidenceId, String)>,
    /// Spans that could not be read back with their hash.
    pub limitations: Vec<String>,
    /// The language the model writes its prose in.
    pub language: ReviewLanguage,
}

fn side_name(side: SourceSide) -> &'static str {
    match side {
        SourceSide::DiffBase => "BASE",
        SourceSide::Head => "HEAD",
        SourceSide::TargetTip => "TARGET_TIP",
    }
}

impl Context {
    /// An empty context whose prompts ask for prose in `language`.
    pub fn new(language: ReviewLanguage) -> Self {
        Self {
            language,
            ..Self::default()
        }
    }

    /// Every fragment of `packet` as one numbered block; `source` is its packet evidence.
    pub fn add_packet(
        &mut self,
        packet: &EvidencePacket,
        source: &Evidence,
        reader: &dyn SnapshotReader,
    ) {
        self.cite(source.id, "source spans shown in this prompt");
        for fragment in &packet.fragments {
            let priority = match fragment.priority {
                Priority::Required => "required",
                Priority::Refuting => "may refute",
                Priority::Supporting => "supporting",
            };
            let reason = format!("{priority}: {}", fragment.reason_selected.join("; "));
            for id in &fragment.evidence_ids {
                self.cite(*id, "derived facts behind the selection, partial");
            }

            self.add_span(&fragment.code_ref, &reason, reader);
        }
        if !packet.gaps.is_empty() {
            self.blocks.push(DataBlock {
                label: "known gaps of this context".into(),
                content: packet.gaps.join("\n"),
            });
        }
    }

    /// Every hunk of the changed files that the spans show, so a model can tell a shown hunk
    /// is its file's whole change: lines no hunk lists are unchanged. Generated files the change
    /// also touches are named without content, so their absence is never read as "not changed".
    pub fn add_changes(&mut self, manifest: &DiffManifest) {
        let mut content = String::new();
        for file in &manifest.files {
            let sides: Vec<_> = file.old.iter().chain(&file.new).map(|s| &s.path).collect();
            if !self.refs.values().any(|r| sides.contains(&&r.path)) {
                continue;
            }

            content.push_str(&change_line(file));
            for h in &file.hunks {
                let _ = write!(
                    content,
                    " -{},{} +{},{}",
                    h.old_start, h.old_lines, h.new_start, h.new_lines
                );
            }
            content.push('\n');
        }
        if !content.is_empty() {
            self.blocks.push(DataBlock {
                label: "every hunk of the shown changed files (BASE -start,count +start,count); \
                        other lines of these files are unchanged"
                    .into(),
                content,
            });
        }

        let hidden: Vec<&FileChange> = manifest.files.iter().filter(|f| f.hidden()).collect();
        if hidden.is_empty() {
            return;
        }

        let mut content = String::new();
        for file in hidden.iter().take(MAX_HIDDEN_LISTED) {
            content.push_str(&change_line(file));
            content.push('\n');
        }
        if hidden.len() > MAX_HIDDEN_LISTED {
            let _ = writeln!(content, "{} more", hidden.len() - MAX_HIDDEN_LISTED);
        }

        self.blocks.push(DataBlock {
            label: "generated files this change also changes; never shown or reviewed: do not \
                    report them as missing, stale or not regenerated"
                .into(),
            content,
        });
    }

    /// The lines every changed test or CI file loses, shown or not: a removed assertion or
    /// gate is visible verbatim, and a file without removals says so.
    pub fn add_test_removals(&mut self, manifest: &DiffManifest, reader: &dyn SnapshotReader) {
        let mut content = String::new();
        for file in &manifest.files {
            let sides = file.old.iter().chain(&file.new);
            let Some(path) = sides.map(|s| &s.path).find(|p| crate::is_test_path(p)) else {
                continue;
            };

            let removed: u32 = file.hunks.iter().map(|h| h.old_lines).sum();
            let _ = writeln!(content, "{path}: {removed} lines removed");
            let Some(old) = &file.old else { continue };
            let mut shown = 0;
            for h in file.hunks.iter().filter(|h| h.old_lines > 0) {
                let end = h.old_start + h.old_lines - 1;
                let take = h.old_lines.min(MAX_REMOVED_LINES - shown);
                if take == 0 {
                    break;
                }

                match reader
                    .code_ref(
                        SourceSide::DiffBase,
                        &old.path,
                        h.old_start,
                        h.old_start + take - 1,
                    )
                    .and_then(|r| reader.read_span(&r))
                {
                    Ok(span) => {
                        for line in span.text.lines() {
                            let _ = writeln!(content, "- {line}");
                        }
                    }
                    Err(err) => {
                        let _ =
                            writeln!(content, "  lines {}-{end} unreadable: {err}", h.old_start);
                    }
                }
                shown += take;
            }
            if removed > shown {
                let _ = writeln!(
                    content,
                    "  {} more removed lines not shown",
                    removed - shown
                );
            }
        }
        if !content.is_empty() {
            self.blocks.push(DataBlock {
                label: "removed lines of every changed test and CI file (BASE)".into(),
                content,
            });
        }
    }

    /// One span under a new `ref-N`, its lines numbered as in the file.
    pub fn add_span(&mut self, code_ref: &CodeRef, reason: &str, reader: &dyn SnapshotReader) {
        let span = match reader.read_span(code_ref) {
            Ok(span) => span,
            Err(err) => {
                self.limitations.push(format!(
                    "{}:{}-{} not shown: {err}",
                    code_ref.path, code_ref.start_line, code_ref.end_line
                ));
                return;
            }
        };
        let id = format!("ref-{}", self.refs.len() + 1);
        let mut content = String::with_capacity(span.text.len() + 8 * 64);
        for (line, text) in (code_ref.start_line..).zip(span.text.lines()) {
            let _ = writeln!(content, "{line:>5} | {text}");
        }
        self.blocks.push(DataBlock {
            label: format!(
                "{id} {} {}:{}-{} ({reason})",
                side_name(code_ref.side),
                code_ref.path,
                code_ref.start_line,
                code_ref.end_line
            ),
            content,
        });
        self.refs.insert(id, code_ref.clone());
    }

    fn cite(&mut self, id: EvidenceId, what: &str) {
        if !self.evidence.iter().any(|(e, _)| *e == id) {
            self.evidence.push((id, what.to_owned()));
        }
    }

    /// The evidence index for the task text; ids and descriptions are harness-made.
    pub fn evidence_index(&self) -> String {
        let mut out = String::new();
        for (i, (_, what)) in self.evidence.iter().enumerate() {
            let _ = writeln!(out, "- ev-{}: {what}", i + 1);
        }
        out
    }

    pub fn evidence_id(&self, local: &str) -> Option<EvidenceId> {
        let n: usize = local.strip_prefix("ev-")?.parse().ok()?;
        self.evidence.get(n.checked_sub(1)?).map(|(id, _)| *id)
    }

    /// Local evidence ids, each mapped or rejected by name.
    pub fn evidence_ids(&self, locals: &[String], problems: &mut Vec<String>) -> Vec<EvidenceId> {
        let mut out = Vec::new();
        for local in locals {
            match self.evidence_id(local) {
                Some(id) if !out.contains(&id) => out.push(id),
                Some(_) => {}
                None => problems.push(format!("unknown evidence id `{local}`")),
            }
        }
        out
    }

    pub fn span(&self, ref_id: &str) -> Option<&CodeRef> {
        self.refs.get(ref_id)
    }

    /// The exact `CodeRef` of lines inside a handed-out span, hashed by the reader. Lines that
    /// lie outside the named span but inside another shown span of the same file and side were
    /// still shown, so they resolve through that span.
    pub fn anchor(&self, anchor: &Anchor, reader: &dyn SnapshotReader) -> Result<CodeRef, String> {
        let named = self
            .refs
            .get(&anchor.ref_id)
            .ok_or_else(|| format!("unknown ref id `{}`", anchor.ref_id))?;
        let shows = |s: &CodeRef| {
            anchor.start_line <= anchor.end_line
                && s.start_line <= anchor.start_line
                && anchor.end_line <= s.end_line
        };
        let span = if shows(named) {
            named
        } else {
            self.refs
                .values()
                .find(|s| s.side == named.side && s.path == named.path && shows(s))
                .ok_or_else(|| {
                    format!(
                        "lines {}-{} are outside {} ({}-{})",
                        anchor.start_line,
                        anchor.end_line,
                        anchor.ref_id,
                        named.start_line,
                        named.end_line
                    )
                })?
        };

        reader
            .code_ref(span.side, &span.path, anchor.start_line, anchor.end_line)
            .map_err(|e| e.to_string())
    }

    /// Anchors that resolve, with a problem named for each that does not.
    pub fn anchors(
        &self,
        anchors: &[Anchor],
        reader: &dyn SnapshotReader,
        problems: &mut Vec<String>,
    ) -> Vec<CodeRef> {
        let mut out = Vec::new();
        for anchor in anchors {
            match self.anchor(anchor, reader) {
                Ok(r) if !out.contains(&r) => out.push(r),
                Ok(_) => {}
                Err(problem) => problems.push(problem),
            }
        }
        out
    }
}
