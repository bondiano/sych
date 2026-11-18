//! The wire form of a verifier's answer for one rule's obligation.

use serde::{Deserialize, Serialize};

use crate::{EvidenceId, ObligationStatus};

/// A verifier's answer for one rule's obligation. Unknown fields are rejected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawAssessment")]
pub struct Assessment {
    pub rule_id: String,
    #[serde(flatten)]
    pub status: ObligationStatus,
    #[serde(default)]
    pub evidence_ids: Vec<EvidenceId>,
    #[serde(default)]
    pub limitations: Vec<String>,
    /// The concrete case the answer named; the reducer reads it from a `VIOLATED` one only, as
    /// the "when" of its rule violation finding.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggering_conditions: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum StatusTag {
    Supported,
    Violated,
    Unknown,
    NotApplicable,
}

/// Flat wire form; serde cannot deny unknown fields through `flatten`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAssessment {
    rule_id: String,
    status: StatusTag,
    reason_code: Option<String>,
    next_evidence_needed: Option<String>,
    basis: Option<String>,
    #[serde(default)]
    evidence_ids: Vec<EvidenceId>,
    #[serde(default)]
    limitations: Vec<String>,
    #[serde(default)]
    triggering_conditions: Vec<String>,
}

impl TryFrom<RawAssessment> for Assessment {
    type Error = &'static str;

    fn try_from(raw: RawAssessment) -> Result<Self, Self::Error> {
        let status = match (
            raw.status,
            raw.reason_code,
            raw.next_evidence_needed,
            raw.basis,
        ) {
            (StatusTag::Supported, None, None, None) => ObligationStatus::Supported,
            (StatusTag::Violated, None, None, None) => ObligationStatus::Violated,
            (StatusTag::Unknown, Some(reason_code), Some(next_evidence_needed), None) => {
                ObligationStatus::Unknown {
                    reason_code,
                    next_evidence_needed,
                }
            }
            (StatusTag::NotApplicable, None, None, Some(basis)) if !basis.trim().is_empty() => {
                ObligationStatus::NotApplicable { basis }
            }
            _ => {
                return Err("UNKNOWN needs reason_code and next_evidence_needed, \
                            NOT_APPLICABLE needs basis, other statuses take neither");
            }
        };
        Ok(Self {
            rule_id: raw.rule_id,
            status,
            evidence_ids: raw.evidence_ids,
            limitations: raw.limitations,
            triggering_conditions: raw.triggering_conditions,
        })
    }
}
