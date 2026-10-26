//! Server locations to exact `CodeRef`s of the materialized side.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::{env, fs};

use reqwest::Url;
use serde_json::Value;
use sych_core::{CodeRef, Completeness, EdgeSet, QueryStatus, RepoPath, line_range, sha256};

use super::Workspace;
use super::symbols::lines;

pub(super) enum Edge {
    Ref(CodeRef),
    External(String),
    Unresolved(String),
}

impl Workspace<'_> {
    /// Maps `Location | Location[] | LocationLink[] | null` to an answered, partial edge set.
    pub(super) fn edges(&self, result: &Value, texts: &mut HashMap<PathBuf, String>) -> EdgeSet {
        let locations = match result {
            Value::Array(items) => items.clone(),
            Value::Null => Vec::new(),
            single => vec![single.clone()],
        };
        let mut set = EdgeSet {
            status: QueryStatus::Answered,
            completeness: Completeness::Partial,
            refs: Vec::new(),
            external: Vec::new(),
            unresolved: Vec::new(),
        };
        for location in locations {
            let uri = location["uri"].as_str().or(location["targetUri"].as_str());
            let range = location.get("range").or(location.get("targetRange"));
            let (Some(uri), Some(range)) = (uri, range) else {
                set.unresolved
                    .push(format!("malformed location {location}"));
                continue;
            };
            match self.locate(uri, range, texts) {
                Edge::Ref(code_ref) if !set.refs.contains(&code_ref) => set.refs.push(code_ref),
                Edge::Ref(_) => {}
                Edge::External(place) => set.external.push(place),
                Edge::Unresolved(why) => set.unresolved.push(why),
            }
        }
        set
    }

    pub(super) fn locate(
        &self,
        uri: &str,
        range: &Value,
        texts: &mut HashMap<PathBuf, String>,
    ) -> Edge {
        let Some(path) = Url::parse(uri).ok().and_then(|u| u.to_file_path().ok()) else {
            return Edge::Unresolved(format!("non-file location {uri}"));
        };
        let Ok(relative) = path.strip_prefix(self.root) else {
            return Edge::External(shorten_home(&path));
        };
        let Some(repo_path) = relative.to_str().and_then(|p| p.parse::<RepoPath>().ok()) else {
            return Edge::Unresolved(format!("unmappable path {}", relative.display()));
        };
        let Some(oid) = self.materialized.blobs.get(&repo_path) else {
            return Edge::Unresolved(format!("{repo_path} is not in the materialized tree"));
        };
        let (start, end) = lines(range);
        let text = texts
            .entry(path.clone())
            .or_insert_with(|| fs::read_to_string(&path).unwrap_or_default());
        let Some(span) = line_range(text, start, end) else {
            return Edge::Unresolved(format!("{repo_path}:{start}-{end} is outside the blob"));
        };
        Edge::Ref(CodeRef {
            snapshot_id: self.snapshot.clone(),
            side: self.side,
            path: repo_path,
            blob_oid: oid.clone(),
            start_line: start,
            end_line: end,
            content_sha256: sha256(span.as_bytes()),
        })
    }
}

fn shorten_home(path: &Path) -> String {
    match env::var_os("HOME").map(PathBuf::from) {
        Some(home) if path.starts_with(&home) => {
            format!("~/{}", path.strip_prefix(&home).unwrap_or(path).display())
        }
        _ => path.display().to_string(),
    }
}
