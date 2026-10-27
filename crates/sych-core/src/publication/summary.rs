//! The one advisory summary of a change (spec §18.1): what was reviewed, how complete, the
//! confirmed problems, the concrete unknowns, optional improvements and nits, cost and links.
//! A partial result never reads as "no problems".

use std::fmt::Write;

use super::{Placement, Planned};
use crate::{Coverage, FindingCategory, ObligationStatus, ReviewResult, Severity};

/// What the summary needs besides the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryContext<'a> {
    pub marker: &'a str,
    pub head_sha: &'a str,
    pub base_sha: &'a str,
    /// Link to the full result, when the server has a public base URL.
    pub result_url: Option<String>,
    pub duration_seconds: Option<i64>,
}

fn short(sha: &str) -> &str {
    &sha[..12]
}

fn location(finding: &crate::Finding) -> String {
    finding.anchors.first().map_or_else(
        || "—".to_owned(),
        |r| format!("`{}:{}`", r.path.as_str(), r.start_line),
    )
}

/// Renders the summary of `result` with the inline plan of its revision.
pub fn render_summary(result: &ReviewResult, plan: &[Planned], ctx: &SummaryContext<'_>) -> String {
    let mut out = String::new();
    header(&mut out, result, ctx);
    problems(&mut out, result, plan);
    unknowns(&mut out, result);
    optional(&mut out, result);
    footer(&mut out, result, ctx);
    out
}

fn header(out: &mut String, result: &ReviewResult, ctx: &SummaryContext<'_>) {
    let _ = writeln!(out, "<!-- {} -->", ctx.marker);
    let _ = writeln!(out, "### Sych review — advisory\n");
    let _ = writeln!(
        out,
        "Reviewed head `{}` against target `{}` · result `{}` (revision {}).",
        short(ctx.head_sha),
        short(ctx.base_sha),
        result.result_id,
        result.review_revision
    );
    let coverage = match result.coverage {
        Coverage::CompleteForScope => "complete for the reviewed scope",
        Coverage::Partial => {
            "**partial** — the gaps below were not checked; this is not \"no problems\""
        }
        Coverage::Unavailable => "**unavailable** — no substantive review was possible",
    };
    let _ = writeln!(
        out,
        "Coverage: {coverage} · recommendation `{:?}` · execution `{:?}`.",
        result.recommendation, result.execution_outcome
    );
    let profiles = &result.scope_manifest.requested_profiles;
    if !profiles.is_empty() {
        let _ = writeln!(out, "Profiles: {}.", profiles.join(", "));
    }
}

fn problems(out: &mut String, result: &ReviewResult, plan: &[Planned]) {
    let placement = |fp: &str| {
        plan.iter()
            .find(|p| p.fingerprint == fp)
            .map(|p| &p.placement)
    };
    // The plan's order: severity, then fingerprint, so the ones over the inline limit come last.
    let mut problems: Vec<_> = result
        .findings
        .iter()
        .filter(|f| {
            matches!(
                f.category,
                FindingCategory::Defect | FindingCategory::RuleViolation
            )
        })
        .collect();
    problems.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then_with(|| a.fingerprint.cmp(&b.fingerprint))
    });
    let _ = writeln!(out, "\n#### Problems ({})", problems.len());
    if problems.is_empty() {
        let none = if result.coverage == Coverage::CompleteForScope {
            "None confirmed in the reviewed scope."
        } else {
            "None confirmed in what could be checked."
        };
        let _ = writeln!(out, "{none}");
    } else {
        let _ = writeln!(
            out,
            "| Severity | Finding | Where | Shown |\n|---|---|---|---|"
        );
        for f in &problems {
            let shown = match placement(&f.fingerprint.to_string()) {
                Some(Placement::Inline { .. }) => "inline".to_owned(),
                Some(Placement::Summary { reason }) => format!("here ({reason})"),
                None => "here".to_owned(),
            };
            let rule = f
                .rule_id
                .as_deref()
                .map(|r| format!(" `{r}`"))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "| {:?} | {}{rule} | {} | {shown} |",
                f.severity,
                f.title,
                location(f)
            );
        }
    }
}

fn unknowns(out: &mut String, result: &ReviewResult) {
    let unknown: Vec<String> = result
        .obligations
        .iter()
        .filter_map(|o| match &o.status {
            ObligationStatus::Unknown {
                reason_code,
                next_evidence_needed,
            } => Some(format!(
                "{}`{}` {} — {reason_code}; needs: {next_evidence_needed}",
                if o.required { "**required** " } else { "" },
                o.rule_id,
                o.question
            )),
            _ => None,
        })
        .chain(result.unresolved_questions.iter().cloned())
        .collect();
    if !unknown.is_empty() {
        let _ = writeln!(out, "\n#### Unknown and missing checks ({})", unknown.len());
        for u in &unknown {
            let _ = writeln!(out, "- {u}");
        }
    }
}

fn optional(out: &mut String, result: &ReviewResult) {
    let optional: Vec<_> = result
        .findings
        .iter()
        .filter(|f| {
            matches!(
                f.category,
                FindingCategory::Improvement | FindingCategory::Nit
            )
        })
        .collect();
    if !optional.is_empty() {
        let _ = writeln!(
            out,
            "\n<details><summary>Improvements and nitpicks ({}) — optional</summary>\n",
            optional.len()
        );
        for f in &optional {
            let basis = f.rule_id.as_deref().unwrap_or("preference");
            let _ = writeln!(
                out,
                "- {:?} · {basis}: {} ({})",
                f.category,
                f.title,
                location(f)
            );
        }
        let _ = writeln!(out, "\n</details>");
    }
}

fn footer(out: &mut String, result: &ReviewResult, ctx: &SummaryContext<'_>) {
    let high = result
        .findings
        .iter()
        .filter(|f| f.severity >= Severity::High)
        .count();
    let cost = result.usage_summary.cost.known.0;
    let _ = write!(
        out,
        "\nCost: ${}.{:06} known",
        cost / 1_000_000,
        cost % 1_000_000
    );
    if result.usage_summary.cost.unknown_cost_attempts > 0 {
        let _ = write!(
            out,
            " + {} attempts of unknown cost",
            result.usage_summary.cost.unknown_cost_attempts
        );
    }

    let _ = write!(
        out,
        " · {} model attempts",
        result.usage_summary.model_attempts
    );
    if let Some(d) = ctx.duration_seconds {
        let _ = write!(out, " · {d} s");
    }

    let _ = writeln!(out, " · {high} high or critical.");
    if let Some(url) = &ctx.result_url {
        let _ = writeln!(out, "Full result: {url}");
    }

    let _ = writeln!(
        out,
        "\n_Advisory: Sych never approves, requests changes or gates merges._"
    );
}
