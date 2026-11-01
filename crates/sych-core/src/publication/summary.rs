//! The one advisory summary of a change: what was reviewed, how complete, the
//! confirmed problems, the concrete unknowns, optional improvements and nits, cost and links.
//! A partial result never reads as "no problems".

use std::fmt::Write;

use super::body::{agent_prompt, badge, category, details, lines, task};
use super::{Placement, Planned, reason_text, tr};
use crate::{
    Coverage, Finding, FindingCategory, GENERATED_IN_CHANGE, ObligationStatus, ReviewResult,
    Severity, SourceSide,
};

/// What the summary needs besides the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryContext<'a> {
    pub marker: &'a str,
    /// `owner/name` on GitHub; findings outside the inline comments link their code with it.
    pub repo: Option<&'a str>,
    pub head_sha: &'a str,
    pub base_sha: &'a str,
    /// Link to the full result, when the server has a public base URL.
    pub result_url: Option<String>,
    pub duration_seconds: Option<i64>,
}

/// GitHub refuses a comment over 65 536 characters; the summary stays below with room to spare.
const MAX_BODY: usize = 60_000;

/// How much of each section a summary shows: everything, or headings only when the full text
/// would be over [`MAX_BODY`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Detail {
    Full,
    Brief,
}

fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

/// Renders the summary of `result` with the inline plan of its revision, in the result's
/// language.
pub fn render_summary(result: &ReviewResult, plan: &[Planned], ctx: &SummaryContext<'_>) -> String {
    let full = render(result, plan, ctx, Detail::Full);
    if full.len() <= MAX_BODY {
        return full;
    }

    render(result, plan, ctx, Detail::Brief)
}

fn render(
    result: &ReviewResult,
    plan: &[Planned],
    ctx: &SummaryContext<'_>,
    detail: Detail,
) -> String {
    // The plan's order: severity, then fingerprint, so the ones over the inline limit come last.
    let mut problems: Vec<&Finding> = result
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
    let mut out = String::new();
    header(&mut out, result, ctx, &problems);
    for (i, f) in problems.iter().enumerate() {
        problem(&mut out, result, plan, ctx, (i + 1, f), detail);
    }

    unknowns(&mut out, result, detail);
    if detail == Detail::Full {
        optional(&mut out, result);
        if !problems.is_empty() {
            let l = result.language;
            let tasks: Vec<String> = problems
                .iter()
                .map(|f| format!("- {}", task(f, l)))
                .collect();
            agent_prompt(
                &mut out,
                tr(
                    l,
                    "Prompt for all findings for AI agents",
                    "Промпт для AI-агента по всем замечаниям",
                ),
                &tasks.join("\n"),
                l,
            );
        }
    }

    footer(&mut out, result, ctx);
    out
}

fn header(
    out: &mut String,
    result: &ReviewResult,
    ctx: &SummaryContext<'_>,
    problems: &[&Finding],
) {
    let l = result.language;
    let _ = writeln!(out, "<!-- {} -->", ctx.marker);
    let _ = writeln!(out, "### {}\n", tr(l, "Sych review", "Ревью Sych"));
    let high = problems
        .iter()
        .filter(|f| f.severity >= Severity::High)
        .count();
    let complete = result.coverage == Coverage::CompleteForScope;
    let alert = if high > 0 {
        "CAUTION"
    } else if !problems.is_empty() || !complete {
        "WARNING"
    } else {
        "TIP"
    };
    let found = match (problems.len(), high) {
        (0, _) if complete => tr(
            l,
            "None confirmed in the reviewed scope.",
            "В проверенном объёме подтверждённых проблем нет.",
        )
        .to_owned(),
        (0, _) => tr(
            l,
            "None confirmed in what could be checked.",
            "В том, что удалось проверить, подтверждённых проблем нет.",
        )
        .to_owned(),
        (n, 0) => format!("{}: {n}.", tr(l, "Problems", "Проблемы")),
        (n, high) => format!(
            "{}: {n}, {high} {}.",
            tr(l, "Problems", "Проблемы"),
            tr(l, "high or critical", "high или critical")
        ),
    };
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
        "> [!{alert}]\n> **{found}** {}: {coverage}.",
        tr(l, "Coverage", "Покрытие")
    );
}

/// A link GitHub renders as the code of the finding's first anchor, on the side it names.
fn permalink(finding: &Finding, ctx: &SummaryContext<'_>) -> Option<String> {
    let r = finding.anchors.first()?;
    let sha = match r.side {
        SourceSide::Head => ctx.head_sha,
        SourceSide::DiffBase => ctx.base_sha,
        SourceSide::TargetTip => return None,
    };
    let range = if r.start_line < r.end_line {
        format!("L{}-L{}", r.start_line, r.end_line)
    } else {
        format!("L{}", r.start_line)
    };
    Some(format!(
        "https://github.com/{}/blob/{sha}/{}#{range}",
        ctx.repo?,
        r.path.as_str()
    ))
}

fn problem(
    out: &mut String,
    result: &ReviewResult,
    plan: &[Planned],
    ctx: &SummaryContext<'_>,
    (n, f): (usize, &Finding),
    detail: Detail,
) {
    let l = result.language;
    let _ = writeln!(
        out,
        "\n#### {n}. {} · {}: {}\n",
        badge(f.severity),
        category(l, f.category),
        f.title
    );
    let mut meta: Vec<String> = f.rule_id.iter().map(|r| format!("`{r}`")).collect();
    meta.extend(lines(f).map(|at| format!("`{at}`")));
    let fingerprint = f.fingerprint.to_string();
    let placement = plan
        .iter()
        .find(|p| p.fingerprint == fingerprint)
        .map(|p| &p.placement);
    let inline = matches!(placement, Some(Placement::Inline { .. }));
    meta.push(match placement {
        Some(Placement::Inline { .. }) => tr(l, "↳ shown inline", "↳ показано в коде").to_owned(),
        Some(Placement::Summary { reason }) => format!(
            "{}: {}",
            tr(l, "not inline", "не в коде"),
            reason_text(l, reason)
        ),
        None => tr(l, "not inline", "не в коде").to_owned(),
    });
    let _ = writeln!(out, "{}", meta.join(" · "));
    if inline || detail == Detail::Brief {
        return;
    }

    if let Some(link) = permalink(f, ctx) {
        let _ = writeln!(out, "\n{link}");
    }

    out.push('\n');
    details(out, f, l, false);
}

fn unknowns(out: &mut String, result: &ReviewResult, detail: Detail) {
    let l = result.language;
    let (required, other): (Vec<_>, Vec<_>) = result
        .obligations
        .iter()
        .filter_map(|o| match &o.status {
            ObligationStatus::Unknown {
                reason_code,
                next_evidence_needed,
            } => Some((
                o.required,
                format!(
                    "{}`{}` {} — {reason_code}; {}: {next_evidence_needed}",
                    if o.required {
                        tr(l, "**required** ", "**обязательное** ")
                    } else {
                        ""
                    },
                    o.rule_id,
                    o.question,
                    tr(l, "needs", "нужно")
                ),
            )),
            _ => None,
        })
        .partition(|(required, _)| *required);
    let other: Vec<String> = other
        .into_iter()
        .map(|(_, text)| text)
        .chain(result.unresolved_questions.iter().cloned())
        .collect();
    // Only the change's own marker kept these from the reviewer: a reader must look at them.
    let hidden: Vec<String> = result
        .excluded_or_unreviewed
        .iter()
        .filter(|g| g.reason == GENERATED_IN_CHANGE)
        .map(|g| {
            format!(
                "{} `{}` — {}",
                tr(l, "**hidden by this change**", "**скрыт этим изменением**"),
                g.path,
                tr(
                    l,
                    "its own content marks it generated; the reviewer did not see it, check it by hand",
                    "его собственное содержимое объявляет его сгенерированным; ревьюер его не видел, проверьте вручную"
                )
            )
        })
        .collect();
    if hidden.is_empty() && required.is_empty() && other.is_empty() {
        return;
    }

    let _ = writeln!(
        out,
        "\n#### {} ({})\n",
        tr(
            l,
            "Unknown and missing checks",
            "Неизвестное и непроверенное"
        ),
        hidden.len() + required.len() + other.len()
    );
    for h in &hidden {
        let _ = writeln!(out, "- {h}");
    }

    for (_, u) in &required {
        let _ = writeln!(out, "- {u}");
    }

    if other.is_empty() {
        return;
    }

    if detail == Detail::Brief {
        let _ = writeln!(
            out,
            "\n_{} {}._",
            other.len(),
            tr(
                l,
                "more not shown: the full summary is over GitHub's size limit",
                "не показано: полная сводка больше лимита GitHub"
            )
        );
        return;
    }

    let _ = writeln!(
        out,
        "\n<details><summary>{} ({})</summary>\n",
        tr(
            l,
            "Other checks and open questions",
            "Прочие проверки и открытые вопросы"
        ),
        other.len()
    );
    for u in &other {
        let _ = writeln!(out, "- {u}");
    }

    let _ = writeln!(out, "\n</details>");
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
            let at = lines(f).map(|at| format!(" (`{at}`)")).unwrap_or_default();
            let _ = writeln!(
                out,
                "- {} · {basis}: {}{at}",
                category(l, f.category),
                f.title
            );
        }
        let _ = writeln!(out, "\n</details>");
    }
}

fn footer(out: &mut String, result: &ReviewResult, ctx: &SummaryContext<'_>) {
    let l = result.language;
    let _ = write!(
        out,
        "\n---\n<sub>{} `{}` · {} `{}` · {} `{}` ({} {}) · {} `{:?}` · {} `{:?}`",
        tr(l, "Head", "Head"),
        short(ctx.head_sha),
        tr(l, "target", "target"),
        short(ctx.base_sha),
        tr(l, "result", "результат"),
        result.result_id,
        tr(l, "revision", "ревизия"),
        result.review_revision,
        tr(l, "recommendation", "рекомендация"),
        result.recommendation,
        tr(l, "execution", "выполнение"),
        result.execution_outcome
    );
    let profiles = &result.scope_manifest.requested_profiles;
    if !profiles.is_empty() {
        let _ = write!(
            out,
            " · {}: {}",
            tr(l, "profiles", "профили"),
            profiles.join(", ")
        );
    }

    let cost = result.usage_summary.cost.known.0;
    let _ = write!(
        out,
        "<br>\n{}: ${}.{:06} {}",
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

    if let Some(url) = &ctx.result_url {
        let _ = write!(
            out,
            " · [{}]({url})",
            tr(l, "full result", "полный результат")
        );
    }

    let _ = writeln!(
        out,
        "<br>\n{}</sub>",
        tr(
            l,
            "Advisory: Sych never approves, requests changes or gates merges.",
            "Рекомендательно: Sych никогда не одобряет, не запрашивает изменения и не блокирует \
             слияние."
        )
    );
}
