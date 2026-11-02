//! The weekly digest: the dashboard analytics of seven days in one document, as JSON or as
//! Markdown to paste or post. Built from the same counts, with the same rule that a share needs
//! five outcomes.

use std::collections::BTreeMap;
use std::fmt::Write;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::analytics::{CostBreakdown, Honesty, Latency, Quality, Spread};
use super::views::Count;
use crate::Timestamp;

/// Fewer outcomes than this say nothing about a share.
const MIN_OUTCOMES: i64 = 5;

/// `GET /v1/reports/weekly`: the seven days to `until` over the repositories the caller reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WeeklyReport {
    pub since: i64,
    pub until: i64,
    pub quality: Quality,
    pub honesty: Honesty,
    pub cost: CostBreakdown,
    pub latency: Latency,
}

fn day(at: i64) -> String {
    Timestamp::from_second(at)
        .map_or_else(|_| at.to_string(), |t| t.strftime("%Y-%m-%d").to_string())
}

fn usd(micro: i64) -> String {
    format!("${}.{:06}", micro / 1_000_000, micro % 1_000_000)
}

/// `part of total`, with the share once there are enough outcomes.
fn share(part: i64, total: i64) -> String {
    if total < MIN_OUTCOMES {
        format!("{part} of {total}")
    } else {
        format!("{part} of {total} ({}%)", (part * 100 + total / 2) / total)
    }
}

fn count(rows: &[Count], key: &str) -> i64 {
    rows.iter().find(|r| r.key == key).map_or(0, |r| r.count)
}

fn listed(rows: &[Count]) -> String {
    if rows.is_empty() {
        return "none".into();
    }

    rows.iter()
        .map(|r| format!("`{}` {}", r.key, r.count))
        .collect::<Vec<_>>()
        .join(", ")
}

fn spread(s: &Spread) -> String {
    match (s.p50_seconds, s.p90_seconds) {
        (Some(p50), Some(p90)) => format!("median {p50} s, p90 {p90} s (n = {})", s.n),
        _ => "no samples".into(),
    }
}

impl WeeklyReport {
    pub fn markdown(&self) -> String {
        let mut out = format!(
            "# Sych weekly report\n\n{} to {} (UTC)\n",
            day(self.since),
            day(self.until)
        );
        self.usefulness(&mut out);
        self.honesty(&mut out);
        self.cost(&mut out);
        let l = &self.latency;
        let _ = write!(
            out,
            "\n## Latency\n\n- To delivery: {}\n- Run duration: {}\n- Queue wait: {}\n",
            spread(&l.to_delivery),
            spread(&l.run_duration),
            spread(&l.queue_wait)
        );
        out
    }

    fn usefulness(&self, out: &mut String) {
        let q = &self.quality;
        let settled: i64 = q.settled.iter().map(|c| c.count).sum();
        let judged: i64 = q.judgements.iter().map(|j| j.count).sum();
        let false_positive = |rows: &mut dyn Iterator<Item = &super::analytics::Judged>| {
            rows.filter(|j| j.judgement == "FALSE_POSITIVE")
                .map(|j| j.count)
                .sum::<i64>()
        };
        let merged = if q.merged_with_open.is_empty() {
            "none".to_owned()
        } else {
            q.merged_with_open
                .iter()
                .map(|m| {
                    format!(
                        "{} with {}",
                        m.changes,
                        m.severity.as_deref().unwrap_or("unknown")
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        let _ = write!(
            out,
            "\n## Usefulness\n\n- Addressed: {} findings of merged or closed pull requests\n\
             - False positives: {} judgements\n- Merged with open findings: {merged}\n",
            share(count(&q.settled, "RESOLVED"), settled),
            share(false_positive(&mut q.judgements.iter()), judged),
        );
        let mut rules: BTreeMap<&str, (i64, i64)> = BTreeMap::new();
        for j in &q.judgements {
            if let Some(rule) = j.rule_id.as_deref() {
                let (fp, total) = rules.entry(rule).or_default();
                *total += j.count;
                *fp += if j.judgement == "FALSE_POSITIVE" {
                    j.count
                } else {
                    0
                };
            }
        }
        let mut noisy: Vec<_> = rules
            .into_iter()
            .filter(|(_, (fp, total))| *fp > 0 && *total >= MIN_OUTCOMES)
            .collect();
        noisy.sort_by(|a, b| (b.1.0 * a.1.1).cmp(&(a.1.0 * b.1.1)));
        for (rule, (fp, total)) in noisy.iter().take(5) {
            let _ = writeln!(
                out,
                "- Noisy rule `{rule}`: false positive {}",
                share(*fp, *total)
            );
        }
    }

    fn honesty(&self, out: &mut String) {
        let h = &self.honesty;
        let runs: i64 = h.coverage_by_day.iter().map(|d| d.runs).sum();
        let partial: i64 = h.coverage_by_day.iter().map(|d| d.partial).sum();
        let _ = write!(
            out,
            "\n## Honesty\n\n- Incomplete coverage: {} finished runs\n- Outcomes: {}\n\
             - Gaps by reason: {}\n- Unknown obligations by reason: {}\n",
            share(partial, runs),
            listed(&h.outcomes),
            listed(&h.gaps),
            listed(&h.unknown)
        );
        if h.unrecorded_runs > 0 {
            let _ = writeln!(
                out,
                "- {} runs completed before reasons were recorded are not in these lists",
                h.unrecorded_runs
            );
        }
    }

    fn cost(&self, out: &mut String) {
        let c = &self.cost;
        let known: i64 = c.by_purpose.iter().map(|r| r.cost_known_micro_usd).sum();
        let unknown: i64 = c.by_purpose.iter().map(|r| r.unknown_cost_attempts).sum();
        let _ = write!(
            out,
            "\n## Cost\n\n- Known: {}, plus {unknown} calls of unknown cost\n\
             - On superseded revisions: {}\n",
            usd(known),
            usd(c.superseded_micro_usd)
        );
        if c.useful_findings > 0 {
            let _ = writeln!(
                out,
                "- Per useful finding: {} ({} findings)",
                usd(known / c.useful_findings),
                c.useful_findings
            );
        }

        if let Some(b) = &c.budget {
            let cap = b.month_limit_micro_usd.map_or_else(|| "no cap".into(), usd);
            let _ = writeln!(
                out,
                "- Tenant budget this month: {} of {cap}, forecast {}",
                usd(b.month_spent_micro_usd),
                usd(b.month_forecast_micro_usd)
            );
        }

        for p in c.top_changes.iter().take(3) {
            let number = p.number.map_or_else(|| "?".into(), |n| n.to_string());
            let _ = writeln!(
                out,
                "- {}#{number}: {}",
                p.repository,
                usd(p.cost_known_micro_usd)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::analytics::{CostBreakdown, Honesty, Judged, Latency, Quality, Spread};
    use super::super::views::Count;
    use super::WeeklyReport;

    fn judged(rule: &str, judgement: &str, count: i64) -> Judged {
        Judged {
            repository: "o/r".into(),
            rule_id: Some(rule.into()),
            category: "BUG".into(),
            severity: None,
            judgement: judgement.into(),
            count,
        }
    }

    #[test]
    fn the_digest_shows_counts_and_a_share_only_from_five_outcomes() {
        let none = Spread::of(Vec::new());
        let report = WeeklyReport {
            since: 1_793_577_600,
            until: 1_794_182_400,
            quality: Quality {
                since: 0,
                judgements: vec![
                    judged("R1", "FALSE_POSITIVE", 3),
                    judged("R1", "USEFUL", 2),
                    judged("R2", "USEFUL", 1),
                ],
                settled: vec![Count {
                    key: "RESOLVED".into(),
                    count: 2,
                }],
                merged_with_open: Vec::new(),
            },
            honesty: Honesty {
                since: 0,
                outcomes: Vec::new(),
                gaps: Vec::new(),
                unknown: Vec::new(),
                unrecorded_runs: 2,
                coverage_by_day: Vec::new(),
            },
            cost: CostBreakdown {
                since: 0,
                by_repository: Vec::new(),
                by_model: Vec::new(),
                by_purpose: Vec::new(),
                top_changes: Vec::new(),
                superseded_micro_usd: 1_500_000,
                useful_findings: 0,
                budget: None,
            },
            latency: Latency {
                since: 0,
                to_delivery: Spread::of(vec![30, 90]),
                run_duration: none.clone(),
                queue_wait: none,
            },
        };
        let text = report.markdown();
        for line in [
            "2026-11-02 to 2026-11-09 (UTC)",
            "- Addressed: 2 of 2 findings",
            "- False positives: 3 of 6 (50%) judgements",
            "- Noisy rule `R1`: false positive 3 of 5 (60%)",
            "- 2 runs completed before reasons were recorded",
            "- On superseded revisions: $1.500000",
            "- To delivery: median 30 s, p90 90 s (n = 2)",
            "- Run duration: no samples",
        ] {
            assert!(text.contains(line), "{line:?} in\n{text}");
        }
        assert!(
            !text.contains("R2"),
            "a rule nobody called wrong is not noisy"
        );
    }
}
