//! Writing rendered results to a file, a bundle directory or stdout.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, absolute};

use serde::Serialize;
use sych_core::{ReportView, SnapshotHeadKind};

use crate::args::{Output, ReportOutput};
use crate::config;
use crate::errors::Failure;
use crate::render::{self, Format, Report, ReportFormat};
use crate::render_sarif;
use crate::report_view;

pub mod bundle;

/// How [`emit_report`] was invoked: `review` writes `.sych-output/` on a terminal or with `--open`,
/// while `show` requires `--out-dir` to write a bundle.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EmitMode {
    Review { terminal: bool },
    Show,
}

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

pub fn validate_report_output(out: &ReportOutput, mode: EmitMode) -> anyhow::Result<()> {
    if out.out_dir.is_some() && (out.format.is_some() || out.output.is_some()) {
        return Err(Failure::invalid("--out-dir cannot be used with --format or --output").into());
    }

    if out.open && (out.format.is_some() || out.output.is_some()) {
        return Err(Failure::invalid("--open cannot be used with --format or --output").into());
    }

    if matches!(mode, EmitMode::Show) && out.open && out.out_dir.is_none() {
        return Err(Failure::invalid("`sych show --open` requires `--out-dir`").into());
    }

    if let Some(patch_path) = out.fixes_patch.as_deref()
        && let Some(report_path) = out.output.as_deref()
        && absolute(patch_path)? == absolute(report_path)?
    {
        return Err(
            Failure::invalid("--fixes-patch and --output must name different files").into(),
        );
    }

    Ok(())
}

fn build_material(
    report: &Report,
    out: &ReportOutput,
    repo: &Path,
    config: Option<&Path>,
) -> anyhow::Result<(ReportView, String)> {
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

    Ok((view, unified_diff))
}

fn terminal_summary_text(report: &Report) -> String {
    let kind = report
        .snapshot
        .as_ref()
        .map_or(SnapshotHeadKind::Committed, |s| s.head_kind);
    render::summary(&report.review_result, kind)
}

/// A saved review as `out` asks, SARIF or full bundle included.
pub fn emit_report(
    report: &Report,
    out: &ReportOutput,
    repo: &Path,
    config: Option<&Path>,
    mode: EmitMode,
) -> anyhow::Result<()> {
    validate_report_output(out, mode)?;

    let terminal = matches!(mode, EmitMode::Review { terminal: true });
    let bundle_target = if let Some(dir) = &out.out_dir {
        Some((dir.clone(), false))
    } else if matches!(mode, EmitMode::Review { .. })
        && out.format.is_none()
        && out.output.is_none()
        && (terminal || out.open)
    {
        Some((bundle::default_out_dir(repo), true))
    } else {
        None
    };

    if let Some((base_dir, write_gitignore)) = bundle_target {
        let (view, unified_diff) = build_material(report, out, repo, config)?;
        let written =
            bundle::write_bundle(report, &view, &unified_diff, &base_dir, write_gitignore)?;
        if terminal {
            let mut text = terminal_summary_text(report);
            text.push_str(&bundle::format_paths(&written));
            emit(&text, None)?;
        }

        if out.open {
            bundle::open_in_browser(&written.report_html)?;
        }

        return Ok(());
    }

    let report_path = out.output.as_deref();
    let format = out.format.unwrap_or(ReportFormat::Json);
    match format {
        ReportFormat::Markdown => {
            let (view, _) = build_material(report, out, repo, config)?;
            emit(&render::markdown(report, &view), report_path)
        }
        ReportFormat::Html => {
            let (view, _) = build_material(report, out, repo, config)?;
            let html = report_view::html::render(&view)
                .map_err(|e| Failure::no_result(format!("cannot render HTML report: {e}")))?;
            emit(&html, report_path)
        }
        ReportFormat::ReportView => {
            let (view, _) = build_material(report, out, repo, config)?;
            emit_json(&view, report_path)
        }
        ReportFormat::Json => {
            if out.fixes_patch.is_some() {
                build_material(report, out, repo, config)?;
            }

            emit_json(report, report_path)
        }
        ReportFormat::Sarif => {
            if out.fixes_patch.is_some() {
                build_material(report, out, repo, config)?;
            }

            emit_json(&render_sarif::sarif(&report.review_result), report_path)
        }
    }
}
