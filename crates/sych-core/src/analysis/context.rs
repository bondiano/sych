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
    EvidenceKind, Producer, ReviewLanguage, SchemaVersion, SnapshotReader, SourceSide,
    SourceSnapshot, Timestamp, TrustClass, Validity, canonical_sha256, derived_uuid,
};

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
    /// is its file's whole change: lines no hunk lists are unchanged.
    pub fn add_changes(&mut self, manifest: &DiffManifest) {
        let mut content = String::new();
        for file in &manifest.files {
            let sides: Vec<_> = file.old.iter().chain(&file.new).map(|s| &s.path).collect();
            if !self.refs.values().any(|r| sides.contains(&&r.path)) {
                continue;
            }

            let mut names: Vec<&str> = sides.iter().map(|p| p.as_str()).collect();
            names.dedup();
            let removed: u32 = file.hunks.iter().map(|h| h.old_lines).sum();
            let added: u32 = file.hunks.iter().map(|h| h.new_lines).sum();
            let _ = write!(
                content,
                "{} {:?}: {removed} lines removed, {added} added;",
                names.join(" -> "),
                file.status
            );
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

    /// The exact `CodeRef` of lines inside a handed-out span, hashed by the reader.
    pub fn anchor(&self, anchor: &Anchor, reader: &dyn SnapshotReader) -> Result<CodeRef, String> {
        let span = self
            .refs
            .get(&anchor.ref_id)
            .ok_or_else(|| format!("unknown ref id `{}`", anchor.ref_id))?;
        if anchor.start_line > anchor.end_line
            || anchor.start_line < span.start_line
            || anchor.end_line > span.end_line
        {
            return Err(format!(
                "lines {}-{} are outside {} ({}-{})",
                anchor.start_line, anchor.end_line, anchor.ref_id, span.start_line, span.end_line
            ));
        }

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
