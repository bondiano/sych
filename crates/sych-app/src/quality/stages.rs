//! Where the review lost a labelled defect it missed, and how much the totals vary between
//! runs of a nondeterministic model.

use serde::Serialize;
use sych_core::broker::EvidencePacket;
use sych_core::{CodeRef, ObligationStatus, ReviewResult, VerificationStatus};

use super::ExpectedFinding;
use super::score::{Remark, Totals, correctness, hits, overlaps};

/// Missed labelled defects by the furthest stage a correctness candidate reached; the six
/// add up to `expected_findings - matched_expected`.
#[derive(Serialize, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Losses {
    /// No candidate reached the labelled lines, though a finished search was shown them: a model
    /// miss.
    pub(super) not_discovered: u32,
    /// No candidate reached the labelled lines and no packet showed them: the context lost them
    /// before any model could look.
    pub(super) not_shown: u32,
    /// No candidate reached the labelled lines, and every search shown them failed, was cut
    /// off or never ran: the provider or the deadline lost them, not the model.
    pub(super) discovery_failed: u32,
    /// Only candidates the verifier rejected reached them.
    pub(super) verifier_rejected: u32,
    /// A candidate left unresolved or never verified reached them, and none was confirmed.
    pub(super) verifier_unresolved: u32,
    /// A confirmed candidate reached them, yet no finding is at those lines: it was merged
    /// into an overlapping finding, or the verdict anchored it elsewhere.
    pub(super) confirmed_unmatched: u32,
}

impl Losses {
    pub(super) fn add(&mut self, other: &Self) {
        self.not_discovered += other.not_discovered;
        self.not_shown += other.not_shown;
        self.discovery_failed += other.discovery_failed;
        self.verifier_rejected += other.verifier_rejected;
        self.verifier_unresolved += other.verifier_unresolved;
        self.confirmed_unmatched += other.confirmed_unmatched;
    }
}

/// Labelled defects no correctness finding hit.
pub(super) fn missed<'a>(
    expected: &[&'a ExpectedFinding],
    findings: &[&Remark<'_>],
) -> Vec<&'a ExpectedFinding> {
    expected
        .iter()
        .copied()
        .filter(|e| hits(&[e], findings) == 0)
        .collect()
}

/// Obligations left UNKNOWN because their required context did not fit a request.
pub(super) fn context_limited(result: &ReviewResult) -> usize {
    result
        .obligations
        .iter()
        .filter(|o| {
            matches!(&o.status, ObligationStatus::Unknown { reason_code, .. }
                if reason_code == "CONTEXT_LIMIT")
        })
        .count()
}

/// Code a run showed the model: in every packet, and in the packets a search that finished
/// was shown.
#[derive(Default)]
pub(super) struct Shown<'a> {
    pub(super) all: Vec<&'a CodeRef>,
    pub(super) searched: Vec<&'a CodeRef>,
}

impl<'a> Shown<'a> {
    /// `searched`: indexes into `packets`.
    pub(super) fn of(packets: &'a [EvidencePacket], searched: &[usize]) -> Self {
        let refs = |p: &'a EvidencePacket| p.fragments.iter().map(|f| &f.code_ref);
        Self {
            all: packets.iter().flat_map(refs).collect(),
            searched: searched
                .iter()
                .filter_map(|&i| packets.get(i))
                .flat_map(refs)
                .collect(),
        }
    }

    /// Whether any packet shows the labelled defect.
    fn shown(&self, label: &ExpectedFinding) -> bool {
        at(&self.all, label)
    }

    /// The labelled defect is shown in a packet, but in none a finished search was shown.
    fn unsearched(&self, label: &ExpectedFinding) -> bool {
        self.shown(label) && !at(&self.searched, label)
    }
}

fn at(refs: &[&CodeRef], label: &ExpectedFinding) -> bool {
    label
        .evidence_locations
        .iter()
        .any(|l| refs.iter().any(|r| overlaps(r, l)))
}

/// `missed`: labelled defects no correctness finding hit; `candidates`: every candidate of
/// the run with its verification status, empty without a verifier.
pub(super) fn losses(
    missed: &[&ExpectedFinding],
    candidates: &[Remark<'_>],
    shown: &Shown<'_>,
) -> Losses {
    let mut out = Losses::default();
    for label in missed {
        let reached: Vec<VerificationStatus> = candidates
            .iter()
            .filter(|c| correctness(c.category) && hits(&[*label], &[c]) > 0)
            .map(|c| c.status)
            .collect();
        let stage = if reached.is_empty() && !shown.shown(label) {
            &mut out.not_shown
        } else if reached.is_empty() && shown.unsearched(label) {
            &mut out.discovery_failed
        } else if reached.is_empty() {
            &mut out.not_discovered
        } else if reached.contains(&VerificationStatus::Confirmed) {
            &mut out.confirmed_unmatched
        } else if reached.iter().all(|s| *s == VerificationStatus::Rejected) {
            &mut out.verifier_rejected
        } else {
            &mut out.verifier_unresolved
        };
        *stage += 1;
    }
    out
}

/// Lowest, mean and highest value of a rate over the runs that have it.
#[derive(Serialize, Debug, PartialEq)]
pub(super) struct Range {
    min: f64,
    mean: f64,
    max: f64,
}

/// How the main rates vary between the 1st, 2nd, … review of every case.
#[derive(Serialize, Default, Debug, PartialEq)]
pub(super) struct Spread {
    recall: Option<Range>,
    clean_false_finding_rate: Option<Range>,
    unknown_kept_rate: Option<Range>,
}

impl Spread {
    /// `runs`: finished totals of each run index across the suite.
    pub(super) fn of(runs: &[Totals]) -> Self {
        let range = |rate: fn(&Totals) -> Option<f64>| {
            let values: Vec<f64> = runs.iter().filter_map(rate).collect();
            (!values.is_empty()).then(|| Range {
                min: values.iter().copied().fold(f64::INFINITY, f64::min),
                mean: values.iter().sum::<f64>()
                    / f64::from(u32::try_from(values.len()).unwrap_or(u32::MAX)),
                max: values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            })
        };
        Self {
            recall: range(|t| t.recall),
            clean_false_finding_rate: range(|t| t.clean_false_finding_rate),
            unknown_kept_rate: range(|t| t.unknown_kept_rate),
        }
    }
}
