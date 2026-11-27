//! Writing the full report bundle (`report.html`, `fix-brief.md`, `review.json`, `review.sarif`
//! and optional `fixes.patch`) into `<DIR>/<run_id>/` and opening `report.html` when requested.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf, absolute};
use std::process::{Command, Stdio};

use sych_core::ReportView;
use sych_core::handoff::bundle_brief;

use super::{emit, emit_json};
use crate::errors::Failure;
use crate::handoff;
use crate::render::Report;
use crate::render_sarif;
use crate::report_view;

pub const DEFAULT_OUT_DIR: &str = ".sych-output";

pub struct WrittenBundle {
    pub report_html: PathBuf,
    pub fix_brief: PathBuf,
    pub review_json: PathBuf,
    pub review_sarif: PathBuf,
    pub fixes_patch: Option<PathBuf>,
}

pub fn default_out_dir(repo: &Path) -> PathBuf {
    if repo.as_os_str().is_empty() || repo == Path::new(".") {
        PathBuf::from(DEFAULT_OUT_DIR)
    } else {
        repo.join(DEFAULT_OUT_DIR)
    }
}

fn ensure_default_gitignore(root: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(root)
        .map_err(|e| Failure::no_result(format!("cannot create {}: {e}", root.display())))?;
    let gitignore = root.join(".gitignore");
    if !gitignore.exists() {
        fs::write(&gitignore, "*\n").map_err(|e| {
            Failure::no_result(format!("cannot write {}: {e}", gitignore.display()))
        })?;
    }

    Ok(())
}

pub fn write_bundle(
    report: &Report,
    view: &ReportView,
    unified_diff: &str,
    base_dir: &Path,
    write_gitignore: bool,
) -> anyhow::Result<WrittenBundle> {
    if write_gitignore {
        ensure_default_gitignore(base_dir)?;
    }

    let run_dir = base_dir.join(report.review_result.run_id.to_string());
    fs::create_dir_all(&run_dir)
        .map_err(|e| Failure::no_result(format!("cannot create {}: {e}", run_dir.display())))?;

    let report_html = run_dir.join("report.html");
    let html = report_view::html::render(view)
        .map_err(|e| Failure::no_result(format!("cannot render HTML report: {e}")))?;
    emit(&html, Some(&report_html))?;

    let fix_brief = run_dir.join("fix-brief.md");
    let brief = handoff::markdown(&bundle_brief(&report.review_result, view));
    emit(&brief, Some(&fix_brief))?;

    let review_json = run_dir.join("review.json");
    emit_json(report, Some(&review_json))?;

    let review_sarif = run_dir.join("review.sarif");
    emit_json(
        &render_sarif::sarif(&report.review_result),
        Some(&review_sarif),
    )?;

    let patch_path = run_dir.join("fixes.patch");
    let fixes_patch = if unified_diff.is_empty() {
        let _ = fs::remove_file(&patch_path);
        None
    } else {
        emit(unified_diff, Some(&patch_path))?;
        Some(patch_path)
    };

    Ok(WrittenBundle {
        report_html,
        fix_brief,
        review_json,
        review_sarif,
        fixes_patch,
    })
}

fn clean_display(path: &Path) -> String {
    path.display()
        .to_string()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

pub fn format_paths(bundle: &WrittenBundle) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Report: {}", clean_display(&bundle.report_html));
    let _ = writeln!(out, "Fix brief: {}", clean_display(&bundle.fix_brief));
    let _ = writeln!(out, "JSON: {}", clean_display(&bundle.review_json));
    let _ = writeln!(out, "SARIF: {}", clean_display(&bundle.review_sarif));
    if let Some(patch) = &bundle.fixes_patch {
        let _ = writeln!(out, "Fixes patch: {}", clean_display(patch));
    }

    out
}

pub fn open_in_browser(path: &Path) -> anyhow::Result<()> {
    let abs = absolute(path)
        .map_err(|e| Failure::no_result(format!("cannot resolve {}: {e}", path.display())))?;
    let mut command = opener(&abs);
    let output = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| Failure::no_result(format!("cannot open {}: {e}", abs.display())))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = detail.trim();
        let reason = if detail.is_empty() {
            format!("opener exited with {}", output.status)
        } else {
            detail.to_owned()
        };
        return Err(Failure::no_result(format!("cannot open {}: {reason}", abs.display())).into());
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn opener(abs: &Path) -> Command {
    let mut cmd = Command::new("open");
    cmd.arg(abs);
    cmd
}

#[cfg(target_os = "windows")]
fn opener(abs: &Path) -> Command {
    let mut cmd = Command::new("rundll32");
    cmd.arg("url.dll,FileProtocolHandler").arg(abs);
    cmd
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn opener(abs: &Path) -> Command {
    let mut cmd = Command::new("xdg-open");
    cmd.arg(abs);
    cmd
}
