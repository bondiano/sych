//! LSP session start: capabilities Sych asks for, producer and position encoding it records.

use std::path::Path;
use std::process;

use reqwest::Url;
use serde_json::{Value, json};
use sych_core::{Producer, SemanticFacts, canonical_sha256};

use super::READY_TIMEOUT;
use super::lsp::{LspError, LspServer};
use super::profiles::Profile;

/// `initialize`/`initialized`, recording producer and encoding; returns server capabilities.
pub(super) fn initialize(
    server: &mut LspServer,
    root: &Path,
    profile: &Profile,
    facts: &mut SemanticFacts,
) -> Result<Value, LspError> {
    let root_uri = file_uri(root);
    let mut init = server.request(
        "initialize",
        &json!({
            "processId": process::id(),
            "rootUri": root_uri,
            "workspaceFolders": [{"uri": root_uri, "name": "snapshot"}],
            "initializationOptions": profile.init_options,
            "capabilities": {
                "general": {"positionEncodings": ["utf-8", "utf-16"]},
                "textDocument": {
                    "documentSymbol": {"hierarchicalDocumentSymbolSupport": true},
                    "references": {}, "implementation": {"linkSupport": true},
                    "definition": {"linkSupport": true},
                    "callHierarchy": {}
                },
                "window": {"workDoneProgress": true},
                "experimental": {"serverStatusNotification": true}
            }
        }),
        READY_TIMEOUT,
    )?;
    let capabilities = init["capabilities"].take();
    facts.position_encoding = Some(
        capabilities["positionEncoding"]
            .as_str()
            .unwrap_or("utf-16")
            .to_owned(),
    );
    let info = &init["serverInfo"];
    facts.producer = Some(Producer {
        name: info["name"]
            .as_str()
            .unwrap_or(&profile.tool.name)
            .to_owned(),
        version: server_version(info["version"].as_str())
            .unwrap_or_else(|| profile.tool.version.clone()),
        config_hash: canonical_sha256(&json!({
            "tool": [&profile.tool.name, &profile.tool.version],
            "args": profile.args,
            "init": profile.init_options,
        }))
        .ok(),
    });
    server.notify("initialized", &json!({}))?;
    if profile.wait_quiescent {
        let status = server.wait_notification("experimental/serverStatus", READY_TIMEOUT, |p| {
            p["quiescent"] == true
        })?;
        let message = format!(
            "server health {}: {}",
            status["health"],
            status["message"]
                .as_str()
                .unwrap_or_default()
                .replace(&root.display().to_string(), "<snapshot>")
        );
        // Without a loaded workspace every answer would be an empty, misleading success.
        if status["health"] == "error" {
            return Err(LspError::Response {
                method: "experimental/serverStatus".into(),
                message,
            });
        }

        if status["health"] != "ok" {
            facts.limitations.push(message);
        }
    }

    Ok(capabilities)
}

/// The server's version; gopls reports its whole build info as JSON, whose `Version` it is.
fn server_version(reported: Option<&str>) -> Option<String> {
    let reported = reported?;
    if reported.starts_with('{') {
        let info: Value = serde_json::from_str(reported).ok()?;
        return info["Version"].as_str().map(str::to_owned);
    }

    Some(reported.to_owned())
}

pub(super) fn file_uri(path: &Path) -> String {
    Url::from_file_path(path).map_or_else(|()| String::new(), String::from)
}
