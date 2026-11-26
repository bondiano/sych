//! Matching findings against earlier dismissals of the same change.
//!
//! Exact fingerprints keep a dismissed finding quiet when code does not move; matching by
//! rule+path or nearby lines+topic also catches reworded claims and small line shifts across
//! later revisions.

use std::collections::BTreeSet;
use std::mem;

use crate::{CodeRef, Finding, FindingCategory, FindingId, Lifecycle, ReviewResult};

/// Maximum line gap between a dismissed anchor and a new anchor on the same file for
/// topic-similarity dismissal matching.
pub const DISMISS_LINE_WINDOW: u32 = 25;

/// A finding previously dismissed on the change, with its anchor location and rationale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DismissedFinding {
    pub fingerprint: String,
    pub category: FindingCategory,
    pub rule_id: Option<String>,
    pub path: Option<String>,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub title: Option<String>,
    pub rationale: Option<String>,
}

/// Marks findings matching any entry in `dismissed`: they are published no more and block
/// nothing, even if a later revision rewords the title or shifts lines slightly.
pub fn dismiss(result: &mut ReviewResult, dismissed: &[DismissedFinding]) {
    for f in &mut result.findings {
        if dismissed.iter().any(|d| matches_dismissed(f, d)) {
            f.lifecycle = Lifecycle::Dismissed;
        }
    }
    let open: Vec<FindingId> = result
        .findings
        .iter()
        .filter(|f| f.lifecycle == Lifecycle::Open)
        .map(|f| f.id)
        .collect();
    result.blocked_by.retain(|id| open.contains(id));
}

/// Whether `finding` represents the same issue as a previously `dismissed` finding.
pub fn matches_dismissed(finding: &Finding, dismissed: &DismissedFinding) -> bool {
    if finding.fingerprint.to_string() == dismissed.fingerprint {
        return true;
    }

    if finding.category == FindingCategory::RuleViolation
        && dismissed.category == FindingCategory::RuleViolation
        && finding.rule_id.is_some()
        && finding.rule_id == dismissed.rule_id
    {
        let same_path = match dismissed.path.as_deref() {
            None => true,
            Some(path) => {
                finding.anchors.is_empty()
                    || finding.anchors.iter().any(|a| a.path.as_str() == path)
            }
        };
        if !same_path {
            return false;
        }

        return match dismissed.title.as_deref().filter(|t| !t.trim().is_empty()) {
            Some(title) => similar_topic(title, &finding.title, &finding.explanation, true),
            None => true,
        };
    }

    if finding.category != dismissed.category || finding.rule_id != dismissed.rule_id {
        return false;
    }

    let Some(path) = dismissed.path.as_deref() else {
        return false;
    };
    let same_file: Vec<&CodeRef> = finding
        .anchors
        .iter()
        .filter(|a| a.path.as_str() == path)
        .collect();
    if same_file.is_empty() {
        return false;
    }

    let Some(dismissed_title) = dismissed.title.as_deref().filter(|t| !t.trim().is_empty()) else {
        return false;
    };
    let has_lines = dismissed.start_line.is_some() && dismissed.end_line.is_some();
    if let (Some(ds), Some(de)) = (dismissed.start_line, dismissed.end_line)
        && !same_file
            .iter()
            .any(|a| line_gap(a.start_line, a.end_line, ds, de) <= DISMISS_LINE_WINDOW)
    {
        return false;
    }

    similar_topic(
        dismissed_title,
        &finding.title,
        &finding.explanation,
        has_lines,
    )
}

fn line_gap(a_start: u32, a_end: u32, b_start: u32, b_end: u32) -> u32 {
    if a_start <= b_end && b_start <= a_end {
        0
    } else if a_end < b_start {
        b_start - a_end
    } else {
        a_start - b_end
    }
}

fn similar_topic(dismissed_title: &str, title: &str, explanation: &str, near_lines: bool) -> bool {
    let dismissed_tokens = informative_tokens(dismissed_title);
    if dismissed_tokens.is_empty() {
        return false;
    }

    let candidate_title = informative_tokens(title);
    let candidate_all = informative_tokens(&format!("{title} {explanation}"));
    if candidate_all.is_empty() {
        return false;
    }

    let shared_title = count_shared(&dismissed_tokens, &candidate_title);
    let shared_all = count_shared(&dismissed_tokens, &candidate_all);
    let min_title = dismissed_tokens.len().min(candidate_title.len()).max(1);
    let dismissed_len = dismissed_tokens.len();

    if near_lines {
        if dismissed_len < 2 {
            return dismissed_title.trim().eq_ignore_ascii_case(title.trim());
        }

        (shared_title >= 2 && shared_title * 100 >= min_title * 40)
            || (shared_all >= 2 && shared_all * 100 >= dismissed_len * 50)
    } else {
        shared_title >= 3 && shared_title * 100 >= min_title * 60
    }
}

fn count_shared(a: &BTreeSet<String>, b: &BTreeSet<String>) -> usize {
    a.iter()
        .filter(|ta| b.iter().any(|tb| tokens_match(ta, tb)))
        .count()
}

fn tokens_match(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }

    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    a_chars.len() >= 5 && b_chars.len() >= 5 && a_chars[..5] == b_chars[..5]
}

fn informative_tokens(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for raw in text.split(|c: char| !c.is_alphanumeric() && c != '_') {
        if raw.is_empty() {
            continue;
        }

        for part in split_identifier(raw) {
            let lowered = part.to_lowercase();
            if lowered.chars().count() >= 3 && !is_stop_word(&lowered) {
                out.insert(lowered);
            }
        }
    }
    out
}

fn split_identifier(raw: &str) -> Vec<String> {
    let mut parts = Vec::new();
    for piece in raw.split('_').filter(|s| !s.is_empty()) {
        let mut current = String::new();
        let mut prev_lower = false;
        for ch in piece.chars() {
            if ch.is_uppercase() && prev_lower && !current.is_empty() {
                parts.push(mem::take(&mut current));
            }

            prev_lower = ch.is_lowercase();
            current.push(ch);
        }
        if !current.is_empty() {
            parts.push(current);
        }
    }
    parts
}

fn is_stop_word(word: &str) -> bool {
    matches!(
        word,
        "the"
            | "and"
            | "for"
            | "with"
            | "not"
            | "are"
            | "was"
            | "were"
            | "that"
            | "this"
            | "from"
            | "into"
            | "without"
            | "when"
            | "only"
            | "has"
            | "have"
            | "does"
            | "can"
            | "all"
            | "any"
            | "its"
            | "still"
            | "при"
            | "без"
            | "для"
            | "или"
            | "что"
            | "как"
            | "это"
            | "нет"
            | "все"
            | "уже"
            | "если"
            | "только"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Coverage, EvidenceManifestId, ExecutionOutcome, FindingId, GitHashAlgorithm, GitObjectId,
        PolicySnapshotId, Provenance, Recommendation, ResultId, ReviewLanguage, RunId,
        SchemaVersion, ScopeManifest, Severity, SourceSide, SourceSnapshotId, Support, Timestamp,
        UsageSummary, sha256,
    };

    fn sample_finding(
        category: FindingCategory,
        rule_id: Option<&str>,
        path: &str,
        lines: (u32, u32),
        title: &str,
        explanation: &str,
    ) -> Finding {
        Finding {
            id: FindingId::new_v7(),
            category,
            severity: Severity::Medium,
            support: Support::SemanticArgument,
            title: title.into(),
            explanation: explanation.into(),
            triggering_conditions: Vec::new(),
            consequence: "consequence".into(),
            anchors: vec![CodeRef {
                snapshot_id: SourceSnapshotId(sha256(b"snap")),
                side: SourceSide::Head,
                path: path.parse().expect("path"),
                blob_oid: GitObjectId::new(GitHashAlgorithm::Sha1, &"a".repeat(40)).expect("oid"),
                start_line: lines.0,
                end_line: lines.1,
                content_sha256: sha256(b"code"),
            }],
            evidence_ids: Vec::new(),
            rule_id: rule_id.map(str::to_owned),
            obligation_ids: Vec::new(),
            suggested_action: "fix".into(),
            validation_expectation: "check".into(),
            fingerprint: sha256(format!("{title}:{}:{}", lines.0, lines.1).as_bytes()),
            predecessor_finding_id: None,
            lifecycle: Lifecycle::Open,
            scope: "u1".into(),
            limitations: Vec::new(),
            suggested_change: None,
            also: Vec::new(),
        }
    }

    fn empty_result(findings: Vec<Finding>) -> ReviewResult {
        ReviewResult {
            schema_version: SchemaVersion,
            result_id: ResultId::new_v7(),
            run_id: RunId::new_v7(),
            review_revision: 1,
            parent_result_id: None,
            source_snapshot_id: SourceSnapshotId(sha256(b"snap")),
            policy_snapshot_id: PolicySnapshotId(sha256(b"pol")),
            evidence_manifest_id: EvidenceManifestId::new_v7(),
            scope_manifest: ScopeManifest {
                revision: 1,
                requested_profiles: Vec::new(),
                units: Vec::new(),
                excluded: Vec::new(),
                required_obligation_ids: Vec::new(),
            },
            execution_outcome: ExecutionOutcome::PlanExecuted,
            coverage: Coverage::CompleteForScope,
            recommendation: Recommendation::AttentionRequired,
            blocked_by: findings.iter().map(|f| f.id).collect(),
            findings,
            obligations: Vec::new(),
            unresolved_questions: Vec::new(),
            excluded_or_unreviewed: Vec::new(),
            rejected_candidates: Vec::new(),
            unresolved_candidates: Vec::new(),
            usage_summary: UsageSummary::default(),
            created_at: Timestamp::now(),
            provenance: Provenance {
                engine_version: "test".into(),
                model_adapters: Vec::new(),
            },
            language: ReviewLanguage::Ru,
        }
    }

    #[test]
    fn reworded_and_line_shifted_defect_stays_dismissed() {
        let dismissed = DismissedFinding {
            fingerprint: sha256(b"rev4").to_string(),
            category: FindingCategory::Defect,
            rule_id: None,
            path: Some("crates/sych-adapters/src/db_schema/collect.rs".into()),
            start_line: Some(118),
            end_line: Some(121),
            title: Some("ORM-only репозиторий без SQL-таргетов пропускается".into()),
            rationale: Some("intentional design".into()),
        };
        let shifted = sample_finding(
            FindingCategory::Defect,
            None,
            "crates/sych-adapters/src/db_schema/collect.rs",
            (123, 125),
            "В ORM-only репозитории без SQL-таргета проверка не запускается",
            "Без SQL-таргетов collect возвращает пустой список.",
        );
        let unrelated = sample_finding(
            FindingCategory::Defect,
            None,
            "crates/sych-adapters/src/db_schema/collect.rs",
            (124, 126),
            "Утечка файловых дескрипторов при чтении конфигурации",
            "Файл остаётся открытым при ошибке чтения.",
        );
        let far = sample_finding(
            FindingCategory::Defect,
            None,
            "crates/sych-adapters/src/db_schema/collect.rs",
            (220, 222),
            "В ORM-only репозитории без SQL-таргета проверка не запускается",
            "Далеко от исходных строк.",
        );
        let mut result = empty_result(vec![shifted, unrelated, far]);
        dismiss(&mut result, &[dismissed]);
        assert_eq!(result.findings[0].lifecycle, Lifecycle::Dismissed);
        assert_eq!(result.findings[1].lifecycle, Lifecycle::Open);
        assert_eq!(result.findings[2].lifecycle, Lifecycle::Open);
        assert_eq!(result.blocked_by.len(), 2);
    }

    #[test]
    fn rule_violation_matches_by_rule_and_path_even_if_fingerprint_differs() {
        let dismissed = DismissedFinding {
            fingerprint: sha256(b"old-ob-id").to_string(),
            category: FindingCategory::RuleViolation,
            rule_id: Some("TEST-INTEGRITY".into()),
            path: Some("crates/sych-adapters/tests/db_schema.rs".into()),
            start_line: Some(10),
            end_line: Some(12),
            title: Some("TEST-INTEGRITY violated: Important checks are not weakened".into()),
            rationale: Some("new opt-in integration tests".into()),
        };
        let same_file = sample_finding(
            FindingCategory::RuleViolation,
            Some("TEST-INTEGRITY"),
            "crates/sych-adapters/tests/db_schema.rs",
            (20, 22),
            "TEST-INTEGRITY violated: Important checks are not weakened",
            "Test skipped.",
        );
        let other_file = sample_finding(
            FindingCategory::RuleViolation,
            Some("TEST-INTEGRITY"),
            "crates/sych-adapters/tests/other.rs",
            (10, 12),
            "TEST-INTEGRITY violated: Important checks are not weakened",
            "Test skipped.",
        );
        assert!(matches_dismissed(&same_file, &dismissed));
        assert!(!matches_dismissed(&other_file, &dismissed));
    }
}
