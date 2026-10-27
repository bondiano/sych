//! The one advisory summary of a change (spec §18.1): what was reviewed, how complete, the
//! confirmed problems, the concrete unknowns, optional improvements and nits, cost and links.
//! A partial result never reads as "no problems".

use std::fmt::Write;

use super::{Placement, Planned, reason_text, tr};
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
    sha.get(..12).unwrap_or(sha)
}

fn location(finding: &crate::Finding) -> String {
    finding.anchors.first().map_or_else(
        || "—".to_owned(),
        |r| format!("`{}:{}`", r.path.as_str(), r.start_line),
    )
}

/// Renders the summary of `result` with the inline plan of its revision, in the result's
/// language.
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
    let l = result.language;
    let _ = writeln!(out, "<!-- {} -->", ctx.marker);
    let _ = writeln!(
        out,
        "### {}\n",
        tr(l, "Sych review — advisory", "Ревью Sych — рекомендательное")
    );
    let _ = writeln!(
        out,
        "{} `{}` {} `{}` · {} `{}` ({} {}).",
        tr(l, "Reviewed head", "Проверен head"),
        short(ctx.head_sha),
        tr(l, "against target", "относительно target"),
        short(ctx.base_sha),
        tr(l, "result", "результат"),
        result.result_id,
        tr(l, "revision", "ревизия"),
        result.review_revision
    );
    let coverage = match result.coverage {
        Coverage::CompleteForScope => tr(
            l,
            "complete for the reviewed scope",
            "полное для проверенного объёма",
        ),
        Coverage::Partial => tr(
            l,
            "**partial** — the gaps below were not checked; this is not \"no problems\"",
            "**частичное** — пробелы ниже не проверены; это не «проблем нет»",
        ),
        Coverage::Unavailable => tr(
            l,
            "**unavailable** — no substantive review was possible",
            "**недоступно** — содержательное ревью было невозможно",
        ),
    };
    let _ = writeln!(
        out,
        "{}: {coverage} · {} `{:?}` · {} `{:?}`.",
        tr(l, "Coverage", "Покрытие"),
        tr(l, "recommendation", "рекомендация"),
        result.recommendation,
        tr(l, "execution", "выполнение"),
        result.execution_outcome
    );
    let profiles = &result.scope_manifest.requested_profiles;
    if !profiles.is_empty() {
        let _ = writeln!(
            out,
            "{}: {}.",
            tr(l, "Profiles", "Профили"),
            profiles.join(", ")
        );
    }
}

fn problems(out: &mut String, result: &ReviewResult, plan: &[Planned]) {
    let l = result.language;
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
    let _ = writeln!(
        out,
        "\n#### {} ({})",
        tr(l, "Problems", "Проблемы"),
        problems.len()
    );
    if problems.is_empty() {
        let none = if result.coverage == Coverage::CompleteForScope {
            tr(
                l,
                "None confirmed in the reviewed scope.",
                "В проверенном объёме подтверждённых проблем нет.",
            )
        } else {
            tr(
                l,
                "None confirmed in what could be checked.",
                "В том, что удалось проверить, подтверждённых проблем нет.",
            )
        };
        let _ = writeln!(out, "{none}");
    } else {
        let _ = writeln!(
            out,
            "{}\n|---|---|---|---|",
            tr(
                l,
                "| Severity | Finding | Where | Shown |",
                "| Важность | Замечание | Где | Показано |"
            )
        );
        for f in &problems {
            let shown = match placement(&f.fingerprint.to_string()) {
                Some(Placement::Inline { .. }) => tr(l, "inline", "в коде").to_owned(),
                Some(Placement::Summary { reason }) => {
                    format!("{} ({})", tr(l, "here", "здесь"), reason_text(l, reason))
                }
                None => tr(l, "here", "здесь").to_owned(),
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
    let l = result.language;
    let unknown: Vec<String> = result
        .obligations
        .iter()
        .filter_map(|o| match &o.status {
            ObligationStatus::Unknown {
                reason_code,
                next_evidence_needed,
            } => Some(format!(
                "{}`{}` {} — {reason_code}; {}: {next_evidence_needed}",
                if o.required {
                    tr(l, "**required** ", "**обязательное** ")
                } else {
                    ""
                },
                o.rule_id,
                o.question,
                tr(l, "needs", "нужно")
            )),
            _ => None,
        })
        .chain(result.unresolved_questions.iter().cloned())
        .collect();
    if !unknown.is_empty() {
        let _ = writeln!(
            out,
            "\n#### {} ({})",
            tr(
                l,
                "Unknown and missing checks",
                "Неизвестное и непроверенное"
            ),
            unknown.len()
        );
        for u in &unknown {
            let _ = writeln!(out, "- {u}");
        }
    }
}

fn optional(out: &mut String, result: &ReviewResult) {
    let l = result.language;
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
            "\n<details><summary>{} ({}) — {}</summary>\n",
            tr(l, "Improvements and nitpicks", "Улучшения и мелочи"),
            optional.len(),
            tr(l, "optional", "необязательно")
        );
        for f in &optional {
            let basis = f
                .rule_id
                .as_deref()
                .unwrap_or_else(|| tr(l, "preference", "предпочтение"));
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
    let l = result.language;
    let high = result
        .findings
        .iter()
        .filter(|f| f.severity >= Severity::High)
        .count();
    let cost = result.usage_summary.cost.known.0;
    let _ = write!(
        out,
        "\n{}: ${}.{:06} {}",
        tr(l, "Cost", "Стоимость"),
        cost / 1_000_000,
        cost % 1_000_000,
        tr(l, "known", "известная")
    );
    let unknown = result.usage_summary.cost.unknown_cost_attempts;
    if unknown > 0 {
        let _ = write!(
            out,
            " + {unknown} {}",
            tr(
                l,
                "attempts of unknown cost",
                "попыток неизвестной стоимости"
            )
        );
    }

    let _ = write!(
        out,
        " · {} {}",
        result.usage_summary.model_attempts,
        tr(l, "model attempts", "попыток модели")
    );
    if let Some(d) = ctx.duration_seconds {
        let _ = write!(out, " · {d} {}", tr(l, "s", "с"));
    }

    let _ = writeln!(
        out,
        " · {high} {}.",
        tr(l, "high or critical", "high или critical")
    );
    if let Some(url) = &ctx.result_url {
        let _ = writeln!(out, "{}: {url}", tr(l, "Full result", "Полный результат"));
    }

    let _ = writeln!(
        out,
        "\n_{}_",
        tr(
            l,
            "Advisory: Sych never approves, requests changes or gates merges.",
            "Рекомендательно: Sych никогда не одобряет, не запрашивает изменения и не блокирует \
             слияние."
        )
    );
}
