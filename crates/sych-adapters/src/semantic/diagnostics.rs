//! Pull diagnostics (`textDocument/diagnostic`) of one queried file.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::{Value, json};
use sych_core::{DiagnosticFact, RepoPath};

use super::lsp::LspServer;
use super::{Edge, QUERY_TIMEOUT, Workspace};

/// Per file; the rest is reported as a limitation rather than dropped silently.
const MAX_DIAGNOSTICS: usize = 50;

fn severity(value: &Value) -> Option<String> {
    let name = match value.as_u64()? {
        1 => "error",
        2 => "warning",
        3 => "information",
        4 => "hint",
        _ => return None,
    };
    Some(name.to_owned())
}

impl Workspace<'_> {
    /// Diagnostics mapped to `CodeRef`s, plus limitations for what could not be reported.
    pub(super) fn diagnostics(
        &self,
        server: &mut LspServer,
        path: &RepoPath,
        uri: &str,
        texts: &mut HashMap<PathBuf, String>,
    ) -> Result<(Vec<DiagnosticFact>, Vec<String>), String> {
        let report = server
            .request(
                "textDocument/diagnostic",
                &json!({"textDocument": {"uri": uri}}),
                QUERY_TIMEOUT,
            )
            .map_err(|e| e.to_string())?;
        let items = report["items"].as_array().cloned().unwrap_or_default();
        let mut facts = Vec::new();
        let mut limitations = Vec::new();
        if items.len() > MAX_DIAGNOSTICS {
            limitations.push(format!(
                "{path}: {} of {} diagnostics reported",
                MAX_DIAGNOSTICS,
                items.len()
            ));
        }

        for item in items.iter().take(MAX_DIAGNOSTICS) {
            match self.locate(uri, &item["range"], texts) {
                Edge::Ref(code_ref) => facts.push(DiagnosticFact {
                    code_ref,
                    severity: severity(&item["severity"]),
                    code: match &item["code"] {
                        Value::String(s) => Some(s.clone()),
                        Value::Number(n) => Some(n.to_string()),
                        _ => None,
                    },
                    source: item["source"].as_str().map(str::to_owned),
                    // Host paths of the scratch workspace never leave the backend.
                    message: item["message"]
                        .as_str()
                        .unwrap_or_default()
                        .replace(&self.root.display().to_string(), "<snapshot>"),
                }),
                Edge::External(_) | Edge::Unresolved(_) => {
                    limitations.push(format!("{path}: unmappable diagnostic range"));
                }
            }
        }
        Ok((facts, limitations))
    }
}
