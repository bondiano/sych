//! Readers and writers among a symbol's references: `documentHighlight` at each reference
//! gives its kind in that reference's document (LSP `Read` = 2, `Write` = 3).

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::{Value, json};
use sych_core::EdgeSet;

use super::lsp::LspServer;
use super::{QUERY_TIMEOUT, Workspace};

/// References classified per symbol; the rest stay unresolved with the reason.
const MAX_CLASSIFIED: usize = 12;

const READ: u64 = 2;
const WRITE: u64 = 3;

impl Workspace<'_> {
    /// `references` of the symbol at `position` and, for a data symbol (`kind` is not a
    /// function), its readers and writers; `offered` is whether the server has references and
    /// document highlights.
    pub(super) fn references(
        &self,
        server: &mut LspServer,
        position: &Value,
        offered: (bool, bool),
        kind: &str,
        texts: &mut HashMap<PathBuf, String>,
    ) -> (EdgeSet, EdgeSet, EdgeSet) {
        let (references, highlights) = offered;
        let mut params = position.clone();
        params["context"] = json!({"includeDeclaration": false});
        let raw =
            references.then(|| server.request("textDocument/references", &params, QUERY_TIMEOUT));
        let function = matches!(kind, "method" | "constructor" | "function");
        let references = match &raw {
            None => EdgeSet::unsupported(),
            Some(Ok(result)) => self.edges(result, texts),
            Some(Err(err)) => EdgeSet::failed(err.to_string()),
        };
        // Calls are neither reads nor writes; data symbols are classified reference by reference.
        let (readers, writers) = match &raw {
            _ if function => (EdgeSet::not_applicable(), EdgeSet::not_applicable()),
            Some(Ok(result)) if highlights => self.access(server, result, texts),
            Some(Ok(_)) => (EdgeSet::unsupported(), EdgeSet::unsupported()),
            _ => (references.clone(), references.clone()),
        };
        (references, readers, writers)
    }

    /// Splits the `references` answer into readers and writers; a reference the server gives
    /// no read/write kind for is unresolved in both, never counted as either.
    fn access(
        &self,
        server: &mut LspServer,
        references: &Value,
        texts: &mut HashMap<PathBuf, String>,
    ) -> (EdgeSet, EdgeSet) {
        let locations = references.as_array().map_or(&[][..], Vec::as_slice);
        let (mut read_at, mut written_at, mut unknown) = (Vec::new(), Vec::new(), Vec::new());
        for (i, location) in locations.iter().enumerate() {
            if i == MAX_CLASSIFIED {
                unknown.push(format!(
                    "{} references not classified (limit {MAX_CLASSIFIED})",
                    locations.len() - i
                ));
                break;
            }

            let start = &location["range"]["start"];
            let params = json!({"textDocument": {"uri": location["uri"]}, "position": start});
            let kind =
                match server.request("textDocument/documentHighlight", &params, QUERY_TIMEOUT) {
                    Ok(Value::Array(highlights)) => highlights
                        .iter()
                        .find(|h| h["range"]["start"] == *start)
                        .and_then(|h| h["kind"].as_u64()),
                    Ok(_) => None,
                    Err(err) => {
                        let failed = || EdgeSet::failed(err.to_string());
                        return (failed(), failed());
                    }
                };
            match kind {
                Some(READ) => read_at.push(location.clone()),
                Some(WRITE) => written_at.push(location.clone()),
                _ => unknown.push(format!(
                    "{}:{} has no read/write kind",
                    location["uri"].as_str().unwrap_or("?"),
                    start["line"].as_u64().map_or(0, |l| l + 1)
                )),
            }
        }
        let mut readers = self.edges(&Value::Array(read_at), texts);
        let mut writers = self.edges(&Value::Array(written_at), texts);
        readers.unresolved.extend(unknown.iter().cloned());
        writers.unresolved.extend(unknown);
        (readers, writers)
    }
}
