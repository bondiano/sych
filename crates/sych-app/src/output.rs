//! Writing rendered results to a file or stdout.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use serde::Serialize;

use crate::args::Output;
use crate::errors::Failure;
use crate::render::{self, Format};

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
