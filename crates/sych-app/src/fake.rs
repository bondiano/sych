//! Fake model adapter fixtures (`--fake-responses`, spec §24.5): scripted evidence and
//! assessments for acceptance tests. The harness resolves code coordinates and binds the
//! evidence to the pinned snapshot; the fixture never supplies ids or hashes.

use std::fs;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sych_adapters::git::LocalSnapshot;
use sych_core::{
    Assessment, BindingKind, Completeness, Evidence, EvidenceBinding, EvidenceId, EvidenceKind,
    Producer, RepoPath, SchemaVersion, SourceSide, Timestamp, TrustClass, Validity,
    canonical_sha256, derived_uuid,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FakeResponses {
    #[expect(dead_code, reason = "validated on decode")]
    schema_version: SchemaVersion,
    /// Simulated model latency; lets acceptance tests cancel a run in flight.
    #[serde(default)]
    delay_ms: u64,
    #[serde(default)]
    evidence: Vec<FakeEvidence>,
    #[serde(default)]
    assessments: Vec<FakeAssessment>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FakeEvidence {
    /// Fixture-local name that assessments cite.
    key: String,
    kind: EvidenceKind,
    completeness: Completeness,
    trust_class: TrustClass,
    #[serde(default = "valid")]
    validity: Validity,
    #[serde(default)]
    code_refs: Vec<FakeCodeRef>,
    #[serde(default)]
    limitations: Vec<String>,
}

fn valid() -> Validity {
    Validity::Valid
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FakeCodeRef {
    #[serde(default = "head")]
    side: SourceSide,
    path: RepoPath,
    start_line: u32,
    end_line: u32,
}

fn head() -> SourceSide {
    SourceSide::Head
}

/// The assessment wire form with fixture keys in `evidence` instead of evidence ids.
#[derive(Deserialize)]
struct FakeAssessment {
    #[serde(default)]
    evidence: Vec<String>,
    #[serde(flatten)]
    rest: Map<String, Value>,
}

/// What the fake adapter answers for one run.
#[derive(Default)]
pub struct Scripted {
    pub delay: Duration,
    pub evidence: Vec<Evidence>,
    pub assessments: Vec<Assessment>,
}

impl FakeResponses {
    pub fn read(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_yaml_ng::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Binds the fixture to the snapshot. Unknown keys, paths or lines are input errors.
    pub fn script(self, snap: &LocalSnapshot, now: Timestamp) -> Result<Scripted, String> {
        let snapshot = &snap.snapshot;
        let id_of = |key: &str| {
            EvidenceId(derived_uuid(&[
                "fake_evidence",
                &snapshot.id.0.to_string(),
                key,
            ]))
        };
        let mut evidence = Vec::with_capacity(self.evidence.len());
        for fake in &self.evidence {
            if evidence.iter().any(|e: &Evidence| e.id == id_of(&fake.key)) {
                return Err(format!("evidence key `{}` is repeated", fake.key));
            }

            let code_refs = fake
                .code_refs
                .iter()
                .map(|r| {
                    snap.code_ref(r.side, &r.path, r.start_line, r.end_line)
                        .map_err(|e| format!("evidence `{}`: {e}", fake.key))
                })
                .collect::<Result<_, _>>()?;
            evidence.push(Evidence {
                schema_version: SchemaVersion,
                id: id_of(&fake.key),
                kind: fake.kind,
                source_snapshot_id: snapshot.id.clone(),
                producer: Producer {
                    name: "fake".into(),
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
                validity: fake.validity,
                completeness: fake.completeness,
                trust_class: fake.trust_class,
                artifact_sha256: canonical_sha256(fake).map_err(|e| e.to_string())?,
                code_refs,
                limitations: fake.limitations.clone(),
                observed_at: now,
            });
        }
        let assessments = self
            .assessments
            .into_iter()
            .enumerate()
            .map(|(i, fake)| {
                let mut wire = fake.rest;
                let mut ids = Vec::with_capacity(fake.evidence.len());
                for key in &fake.evidence {
                    let id = id_of(key);
                    if !evidence.iter().any(|e| e.id == id) {
                        return Err(format!("assessments[{i}]: unknown evidence key `{key}`"));
                    }

                    ids.push(Value::String(id.to_string()));
                }
                wire.insert("evidence_ids".into(), Value::Array(ids));
                serde_json::from_value(Value::Object(wire))
                    .map_err(|e| format!("assessments[{i}]: {e}"))
            })
            .collect::<Result<_, _>>()?;
        Ok(Scripted {
            delay: Duration::from_millis(self.delay_ms),
            evidence,
            assessments,
        })
    }
}
