//! JSON and Markdown renderers of a saved review (spec §24.1). Repository text is data: it is
//! escaped for Markdown/HTML, and terminal control characters never reach the output raw.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sych_core::broker::EvidencePacket;
use sych_core::config::{PolicyDiff, Provenance};
use sych_core::effects::EffectLedger;
use sych_core::{
    CiFacts, CodeRef, DiffManifest, Evidence, EvidenceManifest, ExternalInputManifest, LintFacts,
    Obligation, ObligationStatus, PolicySnapshot, PolicySource, ReviewResult, SchemaVersion,
    SemanticFacts, SourceSnapshot, SyntaxFacts, UsageSummary,
};

/// Policy diagnostics of a run: effective-config provenance and the head's proposed diff.
#[derive(Serialize, Deserialize)]
pub struct PolicyReview {
    pub schema_version: SchemaVersion,
    pub provenance: Vec<Provenance>,
    pub diff: PolicyDiff,
}

/// Everything a saved run consists of; `review` and `show` render the same document.
#[derive(Serialize)]
pub struct Report {
    /// `null` once source data outlived its shorter retention; the result stays readable.
    pub snapshot: Option<SourceSnapshot>,
    pub diff_manifest: Option<DiffManifest>,
    pub policy: PolicySnapshot,
    pub policy_review: PolicyReview,
    pub external_input_manifest: ExternalInputManifest,
    pub evidence_manifest: EvidenceManifest,
    pub evidence: Vec<Evidence>,
    pub semantic_facts: Vec<SemanticFacts>,
    pub syntax_facts: Vec<SyntaxFacts>,
    pub ci_facts: Vec<CiFacts>,
    /// Effects of the changed code, matched base to head, and the bound registry.
    pub effect_ledger: EffectLedger,
    /// Diagnostics of the pinned linters on the head's changed files.
    pub lint_facts: Vec<LintFacts>,
    pub context_packets: Vec<EvidencePacket>,
    pub review_result: ReviewResult,
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum Format {
    Json,
    Markdown,
}

/// Bidi overrides and isolates can make reviewed text read differently than it runs.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// Pretty JSON with DEL, C1 controls and bidi controls `\u`-escaped; serde already escapes C0.
pub fn json<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    let text = serde_json::to_string_pretty(value)?;
    let mut out = String::with_capacity(text.len() + 1);
    for c in text.chars() {
        if c.is_control() || is_bidi_control(c) {
            // Outside strings serde emits only ASCII whitespace, which is not escaped here.
            if c.is_ascii_whitespace() {
                out.push(c);
            } else {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
        } else {
            out.push(c);
        }
    }
    out.push('\n');
    Ok(out)
}

/// Inline Markdown text: one line, no raw HTML, no active Markdown syntax, no control codes.
/// Block markers (`#`, `-`, `+`, `1.`) only act at the start, where list items place the text.
fn md(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut leading_digits = true;
    for (i, c) in text.chars().enumerate() {
        let at_start = i == 0;
        match c {
            '\n' | '\r' | '\t' => out.push(' '),
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\\' | '`' | '*' | '_' | '[' | ']' | '|' | '~' => {
                out.push('\\');
                out.push(c);
            }
            '#' | '-' | '+' if at_start => {
                out.push('\\');
                out.push(c);
            }
            '.' | ')' if leading_digits && !at_start => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_control() || is_bidi_control(c) => {
                let _ = write!(out, "U+{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
        leading_digits &= c.is_ascii_digit();
    }
    out
}

/// Wire name of a serde enum, e.g. `NEEDS_EVIDENCE`.
pub fn tag<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(s)) => s,
        _ => "?".into(),
    }
}

fn status(status: &ObligationStatus) -> String {
    match status {
        ObligationStatus::Unknown {
            reason_code,
            next_evidence_needed,
        } => format!(
            "UNKNOWN ({}); next: {}",
            md(reason_code),
            md(next_evidence_needed)
        ),
        ObligationStatus::NotApplicable { basis } => format!("NOT\\_APPLICABLE: {}", md(basis)),
        ObligationStatus::Supported => "SUPPORTED".into(),
        ObligationStatus::Violated => "VIOLATED".into(),
    }
}

fn code_ref(r: &CodeRef) -> String {
    format!(
        "{}:{}-{} ({}, blob {})",
        md(r.path.as_str()),
        r.start_line,
        r.end_line,
        tag(&r.side),
        r.blob_oid
    )
}

fn list(out: &mut String, items: impl IntoIterator<Item = String>) {
    let mut empty = true;
    for item in items {
        empty = false;
        let _ = writeln!(out, "- {item}");
    }
    if empty {
        out.push_str("- none\n");
    }

    out.push('\n');
}

fn obligation(out: &mut String, ob: &Obligation) {
    let _ = writeln!(
        out,
        "- **{}** v{} ({}, risk {}, applicability {}): {}",
        md(&ob.rule_id),
        ob.rule_version,
        if ob.required { "required" } else { "optional" },
        tag(&ob.risk_level),
        tag(&ob.applicability),
        status(&ob.status)
    );
    let _ = writeln!(out, "  - id: {}", ob.id);
    let _ = writeln!(out, "  - question: {}", md(&ob.question));
    let _ = writeln!(out, "  - acceptance: {}", md(&ob.acceptance_criterion));
    for id in &ob.evidence_ids {
        let _ = writeln!(out, "  - evidence: {id}");
    }
    for l in &ob.limitations {
        let _ = writeln!(out, "  - limitation: {}", md(l));
    }
}

fn evidence(out: &mut String, ev: &Evidence) {
    let _ = writeln!(
        out,
        "- {} {} {} {} {} by {} {}",
        ev.id,
        tag(&ev.kind),
        tag(&ev.validity),
        tag(&ev.completeness),
        tag(&ev.trust_class),
        md(&ev.producer.name),
        md(&ev.producer.version)
    );
    for r in &ev.code_refs {
        let _ = writeln!(out, "  - {}", code_ref(r));
    }
    for l in &ev.limitations {
        let _ = writeln!(out, "  - limitation: {}", md(l));
    }
}

fn policy(out: &mut String, report: &Report) {
    let p = &report.policy;
    let _ = writeln!(out, "\n## Policy\n");
    let source = match &p.source {
        PolicySource::EngineDefaults => "engine defaults".into(),
        PolicySource::TrustedRef { ref_name, commit } => {
            format!("trusted ref {} at {commit}", md(ref_name))
        }
    };
    let _ = writeln!(out, "- source: {source}");
    let _ = writeln!(out, "- policy snapshot: {}", p.id.0);
    let _ = writeln!(out, "- rules: {}, config: {}", p.rules_hash, p.config_hash);
    out.push_str("\nProvenance:\n\n");
    list(
        out,
        report.policy_review.provenance.iter().map(|v| {
            let hash = v.sha256.as_ref().map(ToString::to_string);
            format!(
                "{}: {} {}",
                tag(&v.layer),
                md(&v.origin),
                hash.as_deref().unwrap_or("")
            )
        }),
    );
    let diff = &report.policy_review.diff;
    out.push_str("Proposed by head, not applied:\n\n");
    list(
        out,
        diff.files
            .iter()
            .map(|f| format!("{} {}", tag(&f.change), md(f.path.as_str())))
            .chain(
                diff.rules
                    .iter()
                    .map(|c| format!("rule {} {}", md(&c.id), tag(&c.change))),
            )
            .chain(diff.head_errors.iter().map(|e| format!("error: {}", md(e)))),
    );
}

pub fn markdown(report: &Report) -> String {
    let r = &report.review_result;
    let mut out = String::new();
    let _ = writeln!(out, "# Sych review\n");
    let _ = writeln!(out, "- recommendation: **{}**", tag(&r.recommendation));
    let _ = writeln!(out, "- coverage: **{}**", tag(&r.coverage));
    let _ = writeln!(out, "- execution outcome: {}", tag(&r.execution_outcome));
    let _ = writeln!(
        out,
        "- run: {} (revision {}), result: {}, created {}",
        r.run_id, r.review_revision, r.result_id, r.created_at
    );
    let _ = writeln!(out, "\n## Snapshot\n");
    if let Some(s) = &report.snapshot {
        let _ = writeln!(out, "- repository: {}", md(&s.repository));
        let _ = writeln!(
            out,
            "- target: {} at {}",
            md(s.target_ref.as_deref().unwrap_or("?")),
            s.target_tip
        );
        let _ = writeln!(out, "- diff base: {} ({})", s.diff_base, tag(&s.comparison));
        let _ = writeln!(out, "- head: {}", s.head);
        let _ = writeln!(out, "- snapshot: {}", s.id.0);
    } else {
        let _ = writeln!(
            out,
            "- unavailable: the source snapshot is past its retention"
        );
    }

    policy(&mut out, report);
    let _ = writeln!(out, "## Scope (revision {})\n", r.scope_manifest.revision);
    list(
        &mut out,
        r.scope_manifest.units.iter().map(|u| {
            let paths: Vec<String> = u.paths.iter().map(|p| md(p.as_str())).collect();
            format!("{}: {}", md(&u.title), paths.join(", "))
        }),
    );
    out.push_str("## Obligations\n\n");
    for ob in &r.obligations {
        obligation(&mut out, ob);
    }
    if r.obligations.is_empty() {
        out.push_str("- none\n");
    }

    out.push_str("\n## Findings\n\n");
    for f in &r.findings {
        let _ = writeln!(
            out,
            "- **{}** {} {}: {}",
            tag(&f.severity),
            tag(&f.category),
            tag(&f.lifecycle),
            md(&f.title)
        );
        let _ = writeln!(out, "  - {}", md(&f.explanation));
        let _ = writeln!(out, "  - consequence: {}", md(&f.consequence));
        for a in &f.anchors {
            let _ = writeln!(out, "  - at {}", code_ref(a));
        }
        let _ = writeln!(out, "  - action: {}", md(&f.suggested_action));
    }
    if r.findings.is_empty() {
        out.push_str("- none\n");
    }

    out.push_str("\n## Gaps\n\n");
    list(
        &mut out,
        r.excluded_or_unreviewed
            .iter()
            .map(|g| format!("{}: {}", md(g.path.as_str()), md(&g.reason)))
            .chain(r.unresolved_questions.iter().map(|q| md(q))),
    );
    out.push_str("## Evidence\n\n");
    for ev in &report.evidence {
        evidence(&mut out, ev);
    }
    if report.evidence.is_empty() {
        out.push_str("- none\n");
    }

    usage(&mut out, &r.usage_summary);
    let adapters: Vec<String> = r.provenance.model_adapters.iter().map(|a| md(a)).collect();
    let _ = writeln!(
        out,
        "- engine {}, model adapters: {}",
        md(&r.provenance.engine_version),
        adapters.join(", ")
    );
    out
}

fn usage(out: &mut String, u: &UsageSummary) {
    out.push_str("\n## Usage\n\n");
    let _ = writeln!(
        out,
        "- model attempts: {}, tokens in/out: {}/{} (reasoning {} of output), attempts with unreported tokens: {}",
        u.model_attempts,
        u.input_tokens,
        u.output_tokens,
        u.reasoning_tokens,
        u.unknown_usage_attempts
    );
    let _ = writeln!(
        out,
        "- known cost: {} micro-USD, attempts with unknown cost: {}, list-price estimate: {} micro-USD",
        u.cost.known.0, u.cost.unknown_cost_attempts, u.cost.reference_estimate.0
    );
    for route in &u.routes {
        let _ = write!(
            out,
            "- route {}: {}, {}, {} attempts",
            md(&format!("{}:{}", route.profile, route.model)),
            tag(&route.billing),
            tag(&route.enforcement),
            route.attempts
        );
        if route.quota_attempts > 0 {
            let _ = write!(
                out,
                ", {} on included quota (remaining quota not reported)",
                route.quota_attempts
            );
        }

        if let Some(fee) = &route.fixed_fee {
            let _ = write!(
                out,
                ", fixed fee {} micro-USD for {}, {} allocated to the model pool once per month",
                fee.amount.0, fee.period, fee.allocated.0
            );
        }

        out.push('\n');
    }
}

/// One obligation with the evidence it cites and the findings it produced.
#[derive(Serialize)]
pub struct Explanation<'a> {
    pub run_id: String,
    pub obligation: &'a Obligation,
    pub evidence: Vec<&'a Evidence>,
    /// Verification packets built for this obligation.
    pub context: Vec<&'a EvidencePacket>,
    pub findings: Vec<&'a sych_core::Finding>,
}

impl<'a> Explanation<'a> {
    pub fn new(report: &'a Report, obligation: &'a Obligation) -> Self {
        let r = &report.review_result;
        Self {
            run_id: r.run_id.to_string(),
            obligation,
            evidence: report
                .evidence
                .iter()
                .filter(|e| obligation.evidence_ids.contains(&e.id))
                .collect(),
            context: report
                .context_packets
                .iter()
                .filter(|p| p.request.obligation_id == Some(obligation.id))
                .collect(),
            findings: r
                .findings
                .iter()
                .filter(|f| f.obligation_ids.contains(&obligation.id))
                .collect(),
        }
    }

    pub fn markdown(&self) -> String {
        let mut out = format!("# Obligation {}\n\n", md(&self.obligation.rule_id));
        obligation(&mut out, self.obligation);
        out.push_str("\n## Evidence\n\n");
        for ev in &self.evidence {
            evidence(&mut out, ev);
        }
        if self.evidence.is_empty() {
            out.push_str("- none cited\n");
        }

        out.push_str("\n## Context\n\n");
        for packet in &self.context {
            context(&mut out, packet);
        }
        if self.context.is_empty() {
            out.push_str("- no packet\n");
        }

        out.push_str("\n## Findings\n\n");
        list(
            &mut out,
            self.findings
                .iter()
                .map(|f| format!("{} {}: {}", f.id, tag(&f.severity), md(&f.title))),
        );
        out
    }
}

fn context(out: &mut String, packet: &EvidencePacket) {
    let u = packet.usage;
    let _ = writeln!(
        out,
        "- {} · {} tokens of {} · {} files · {} semantic items",
        tag(&packet.resolution),
        u.tokens,
        packet.request.remaining_budget,
        u.files,
        u.semantic_items
    );
    for f in &packet.fragments {
        let r = &f.code_ref;
        let reasons: Vec<String> = f.reason_selected.iter().map(|s| md(s)).collect();
        let _ = writeln!(
            out,
            "  - {} `{}:{}-{}` ({}): {}",
            tag(&f.priority),
            md(r.path.as_str()),
            r.start_line,
            r.end_line,
            tag(&r.side),
            reasons.join("; ")
        );
    }
    for e in &packet.excluded {
        let r = &e.code_ref;
        let _ = writeln!(
            out,
            "  - excluded `{}:{}-{}`: {}",
            md(r.path.as_str()),
            r.start_line,
            r.end_line,
            md(&e.reason)
        );
    }
    for gap in &packet.gaps {
        let _ = writeln!(out, "  - gap: {}", md(gap));
    }
}

#[cfg(test)]
mod tests {
    use super::{json, md};

    #[test]
    fn markdown_text_cannot_inject_html_markup_or_terminal_codes() {
        let hostile = "<img src=x>\n# [x](javascript:1) *b* \u{1b}[31m\u{9b}2J\u{202e}";
        let escaped = md(hostile);
        assert!(!escaped.contains('<') && !escaped.contains('\n'));
        assert!(!escaped.chars().any(|c| c.is_control() || c == '\u{202e}'));
        assert!(escaped.contains("\\[x\\](javascript:1)") && escaped.contains("\\*b\\*"));
        assert_eq!(md("# h"), "\\# h");
        assert_eq!(md("12. item - a-b"), "12\\. item - a-b");
    }

    #[test]
    fn json_escapes_c1_and_bidi_controls_and_stays_valid() {
        let text = json(&"a\u{1b}b\u{9b}c\u{202e}d").expect("json");
        assert_eq!(text, "\"a\\u001bb\\u009bc\\u202ed\"\n");
        let back: String = serde_json::from_str(&text).expect("valid json");
        assert_eq!(back, "a\u{1b}b\u{9b}c\u{202e}d");
    }
}
