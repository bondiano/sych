//! Writing rendered results to a file or stdout.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, absolute};

use serde::Serialize;
use sych_core::SnapshotHeadKind;

use crate::args::{Output, ReportOutput};
use crate::config;
use crate::errors::Failure;
use crate::render::{self, Format, Report, ReportFormat};
use crate::render_sarif;
use crate::report_view;

/// Writes `text` to `output` (creating its directory) or stdout.
pub fn emit(text: &str, output: Option<&Path>) -> anyhow::Result<()> {
    let written = match output {
        Some(path) => path
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(path, text)),
        None => io::stdout().lock().write_all(text.as_bytes()),
    };
    Ok(written.map_err(|e| Failure::no_result(format!("cannot write result: {e}")))?)
}

/// Pretty JSON of `value` written like [`emit`].
pub fn emit_json<T: Serialize>(value: &T, output: Option<&Path>) -> anyhow::Result<()> {
    let text =
        render::json(value).map_err(|e| Failure::no_result(format!("cannot render: {e}")))?;
    emit(&text, output)
}

/// `value` as JSON or `markdown` as Markdown, as `out` asks.
pub fn emit_as<T: Serialize>(
    value: &T,
    markdown: impl FnOnce() -> String,
    out: &Output,
) -> anyhow::Result<()> {
    match out.format {
        Format::Json => emit_json(value, out.output.as_deref()),
        Format::Markdown => emit(&markdown(), out.output.as_deref()),
    }
}

/// A saved review as `out` asks, SARIF included.
pub fn emit_report(
    report: &Report,
    out: &ReportOutput,
    repo: &Path,
    config: Option<&Path>,
    terminal_summary: bool,
) -> anyhow::Result<()> {
    let report_path = out.output.as_deref();
    if let Some(patch_path) = out.fixes_patch.as_deref()
        && let Some(report_path) = report_path
        && absolute(patch_path)? == absolute(report_path)?
    {
        return Err(
            Failure::invalid("--fixes-patch and --output must name different files").into(),
        );
    }

    let material = || -> anyhow::Result<_> {
        let redactor = config::server(config)?
            .redaction
            .redactor()
            .map_err(Failure::invalid)?;
        let (view, unified_diff) = report_view::build(report, repo, &redactor);
        if let Some(path) = out.fixes_patch.as_deref() {
            if unified_diff.is_empty() {
                eprintln!("No applicable suggested fixes in this run.");
            }

            emit(&unified_diff, Some(path))?;
        }

        Ok(view)
    };
    let format = out.format.unwrap_or(ReportFormat::Json);
    match format {
        ReportFormat::Markdown => emit(&render::markdown(report, &material()?), report_path),
        ReportFormat::Html => {
            let html = report_view::html::render(&material()?)
                .map_err(|e| Failure::no_result(format!("cannot render HTML report: {e}")))?;
            emit(&html, report_path)
        }
        ReportFormat::ReportView => emit_json(&material()?, report_path),
        ReportFormat::Json | ReportFormat::Sarif => {
            if out.fixes_patch.is_some() {
                material()?;
            }

            if terminal_summary {
                let kind = report
                    .snapshot
                    .as_ref()
                    .map_or(SnapshotHeadKind::Committed, |s| s.head_kind);
                emit(&render::summary(&report.review_result, kind), None)
            } else if matches!(format, ReportFormat::Json) {
                emit_json(report, report_path)
            } else {
                emit_json(&render_sarif::sarif(&report.review_result), report_path)
            }
        }
    }
}
