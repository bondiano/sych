//! The type definitions a symbol names, and its call hierarchy: callers, the constructs they
//! sit in, and callees.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::{Value, json};
use sych_core::{CodeRef, EdgeSet};

use super::lsp::{LspError, LspServer};
use super::{QUERY_TIMEOUT, Workspace};

impl Workspace<'_> {
    /// `definition` of each named type; any failure fails the whole set.
    pub(super) fn definitions(
        &self,
        server: &mut LspServer,
        uri: &str,
        names: Vec<(String, Value)>,
        texts: &mut HashMap<PathBuf, String>,
    ) -> EdgeSet {
        let mut found = Vec::new();
        for (_, at) in names {
            let params = json!({"textDocument": {"uri": uri}, "position": at});
            match server.request("textDocument/definition", &params, QUERY_TIMEOUT) {
                Ok(Value::Array(items)) => found.extend(items),
                Ok(Value::Null) => {}
                Ok(item) => found.push(item),
                Err(err) => return EdgeSet::failed(err.to_string()),
            }
        }
        self.edges(&Value::Array(found), texts)
    }

    /// `prepareCallHierarchy`, then `incomingCalls`: call sites (`fromRanges`) become caller
    /// refs and each calling item's range its enclosing construct; then `outgoingCalls`: each
    /// called item's range is a callee construct.
    pub(super) fn call_hierarchy(
        &self,
        server: &mut LspServer,
        position: &Value,
        texts: &mut HashMap<PathBuf, String>,
    ) -> (EdgeSet, Vec<CodeRef>, EdgeSet) {
        let failed = |err: LspError| {
            let failed = || EdgeSet::failed(err.to_string());
            (failed(), Vec::new(), failed())
        };
        let items =
            match server.request("textDocument/prepareCallHierarchy", position, QUERY_TIMEOUT) {
                Ok(Value::Array(items)) => items,
                Ok(_) => Vec::new(),
                Err(err) => return failed(err),
            };
        let mut sites = Vec::new();
        let mut constructs = Vec::new();
        let mut targets = Vec::new();
        for item in items {
            let params = json!({"item": item});
            match server.request("callHierarchy/incomingCalls", &params, QUERY_TIMEOUT) {
                Ok(Value::Array(calls)) => {
                    for call in calls {
                        let from = call["from"]["uri"].clone();
                        constructs.push(json!({"uri": from, "range": call["from"]["range"]}));
                        for range in call["fromRanges"].as_array().into_iter().flatten() {
                            sites.push(json!({"uri": from, "range": range}));
                        }
                    }
                }
                Ok(_) => {}
                Err(err) => return failed(err),
            }
            match server.request("callHierarchy/outgoingCalls", &params, QUERY_TIMEOUT) {
                Ok(Value::Array(calls)) => targets.extend(
                    calls
                        .iter()
                        .map(|c| json!({"uri": c["to"]["uri"], "range": c["to"]["range"]})),
                ),
                Ok(_) => {}
                Err(err) => {
                    let constructs = self.edges(&Value::Array(constructs), texts).refs;
                    let incoming = self.edges(&Value::Array(sites), texts);
                    return (incoming, constructs, EdgeSet::failed(err.to_string()));
                }
            }
        }
        let constructs = self.edges(&Value::Array(constructs), texts).refs;
        (
            self.edges(&Value::Array(sites), texts),
            constructs,
            self.edges(&Value::Array(targets), texts),
        )
    }
}
