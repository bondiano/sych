//! Typed context operations a model may request. Arguments are a closed enum
//! that rejects unknown fields; there is no shell, URL, database or credential operation.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    CodeRef, EdgeSet, Evidence, EvidenceId, QueryStatus, RepoPath, SemanticFacts, SnapshotReader,
    SourceSide, SymbolFacts, is_secret_path,
};

const MAX_LIMIT: usize = 50;
const MAX_QUERY_CHARS: usize = 200;

/// A changed construct by side, file and name, as listed in `SemanticFacts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SymbolRef {
    pub side: SourceSide,
    pub path: RepoPath,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    ReadSpan {
        code_ref: CodeRef,
    },
    ReadSymbol {
        symbol: SymbolRef,
    },
    FindDefinition {
        symbol: SymbolRef,
    },
    FindReferences {
        symbol: SymbolRef,
        limit: usize,
    },
    FindImplementations {
        symbol: SymbolRef,
        limit: usize,
    },
    FindCallers {
        symbol: SymbolRef,
        limit: usize,
    },
    FindCallees {
        symbol: SymbolRef,
        limit: usize,
    },
    /// Fixed-string search only, so CPU stays bounded.
    SearchSource {
        query: String,
        paths: Vec<RepoPath>,
        side: SourceSide,
        limit: usize,
    },
    ReadEvidence {
        evidence_id: EvidenceId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OpStatus {
    CompleteForQuery,
    Partial,
    Unsupported,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OpResult {
    pub status: OpStatus,
    pub refs: Vec<CodeRef>,
    /// Span text for `read_span`/`read_symbol`, the evidence document for `read_evidence`.
    pub text: Option<String>,
    pub limitations: Vec<String>,
}

impl OpResult {
    fn refs(status: OpStatus, refs: Vec<CodeRef>, limitations: Vec<String>) -> Self {
        Self {
            status,
            refs,
            text: None,
            limitations,
        }
    }

    fn failed(reason: String) -> Self {
        Self::refs(OpStatus::Failed, Vec::new(), vec![reason])
    }

    fn unsupported(reason: &str) -> Self {
        Self::refs(OpStatus::Unsupported, Vec::new(), vec![reason.to_owned()])
    }
}

pub struct Ops<'a> {
    pub facts: &'a [SemanticFacts],
    pub evidence: &'a [Evidence],
    pub reader: &'a dyn SnapshotReader,
    /// Generated files of the change ([`crate::DiffManifest::hidden_paths`]): never read for a
    /// model, nor found by its searches.
    pub generated: &'a [RepoPath],
}

impl Ops<'_> {
    pub fn run(&self, operation: &Operation) -> OpResult {
        match operation {
            Operation::ReadSpan { code_ref } => self.read(code_ref),
            Operation::ReadSymbol { symbol } => match self.symbol(symbol) {
                Some(s) => self.read(&s.definition),
                None => not_changed(),
            },
            Operation::FindDefinition { symbol } => match self.symbol(symbol) {
                Some(s) => OpResult::refs(
                    OpStatus::CompleteForQuery,
                    vec![s.definition.clone()],
                    Vec::new(),
                ),
                None => not_changed(),
            },
            Operation::FindReferences { symbol, limit } => {
                self.edge(symbol, *limit, |s| &s.references)
            }
            Operation::FindImplementations { symbol, limit } => {
                self.edge(symbol, *limit, |s| &s.implementations)
            }
            Operation::FindCallers { symbol, limit } => self.edge(symbol, *limit, |s| &s.callers),
            Operation::FindCallees { symbol, limit } => self.edge(symbol, *limit, |s| &s.callees),
            Operation::SearchSource {
                query,
                paths,
                side,
                limit,
            } => self.search(query, paths, *side, *limit),
            Operation::ReadEvidence { evidence_id } => {
                match self.evidence.iter().find(|e| &e.id == evidence_id) {
                    Some(e) => OpResult {
                        status: OpStatus::CompleteForQuery,
                        refs: e.code_refs.clone(),
                        text: serde_json::to_string(e).ok(),
                        limitations: e.limitations.clone(),
                    },
                    None => {
                        OpResult::failed(format!("evidence {evidence_id} is not part of this run"))
                    }
                }
            }
        }
    }

    fn read(&self, code_ref: &CodeRef) -> OpResult {
        if is_secret_path(code_ref.path.as_str()) {
            return OpResult::failed("secret path".into());
        }

        if self.generated.contains(&code_ref.path) {
            return OpResult::unsupported("generated file: not shown to reviewers");
        }

        match self.reader.read_span(code_ref) {
            Ok(span) => OpResult {
                status: OpStatus::CompleteForQuery,
                refs: vec![span.reference],
                text: Some(span.text),
                limitations: Vec::new(),
            },
            Err(err) => OpResult::failed(err.to_string()),
        }
    }

    fn symbol(&self, symbol: &SymbolRef) -> Option<&SymbolFacts> {
        self.facts
            .iter()
            .filter(|f| f.side == symbol.side)
            .flat_map(|f| &f.symbols)
            .find(|s| s.definition.path == symbol.path && s.name == symbol.name)
    }

    /// An answered edge is `PARTIAL`: an empty answer does not prove absence.
    fn edge(
        &self,
        symbol: &SymbolRef,
        limit: usize,
        pick: impl Fn(&SymbolFacts) -> &EdgeSet,
    ) -> OpResult {
        let Some(found) = self.symbol(symbol) else {
            return not_changed();
        };
        let edge = pick(found);
        let limit = limit.clamp(1, MAX_LIMIT);
        let mut limitations: Vec<String> = edge.unresolved.clone();
        if !edge.external.is_empty() {
            limitations.push(format!(
                "{} locations outside the snapshot",
                edge.external.len()
            ));
        }

        if edge.refs.len() > limit {
            limitations.push(format!("{} of {} shown", limit, edge.refs.len()));
        }

        let refs = edge.refs.iter().take(limit).cloned().collect();
        match edge.status {
            QueryStatus::Answered => OpResult::refs(OpStatus::Partial, refs, limitations),
            QueryStatus::Unsupported => OpResult::unsupported("server lacks the capability"),
            QueryStatus::NotApplicable => {
                OpResult::unsupported("not applicable to this symbol kind")
            }
            QueryStatus::Failed => OpResult::refs(OpStatus::Failed, Vec::new(), limitations),
        }
    }

    fn search(&self, query: &str, paths: &[RepoPath], side: SourceSide, limit: usize) -> OpResult {
        if query.is_empty()
            || query.chars().count() > MAX_QUERY_CHARS
            || query.contains(['\0', '\n'])
        {
            return OpResult::failed(format!(
                "query must be 1..={MAX_QUERY_CHARS} characters on one line"
            ));
        }

        let limit = limit.clamp(1, MAX_LIMIT);
        match self.reader.search(side, query, paths, limit + 1) {
            Ok(mut hits) => {
                // A literal search over the pinned tree is complete unless the reader cut it off;
                // dropping hits afterwards never makes a cut answer whole.
                let cut = hits.len() > limit;
                hits.retain(|r| !is_secret_path(r.path.as_str()));
                let found = hits.len();
                hits.retain(|r| !self.generated.contains(&r.path));
                let mut limitations = Vec::new();
                if hits.len() < found {
                    limitations.push(format!(
                        "{} hits in generated files are not shown",
                        found - hits.len()
                    ));
                }

                if cut {
                    hits.truncate(limit);
                    limitations.push(format!("first {limit} hits"));
                    OpResult::refs(OpStatus::Partial, hits, limitations)
                } else {
                    OpResult::refs(OpStatus::CompleteForQuery, hits, limitations)
                }
            }
            Err(err) => OpResult::failed(err.to_string()),
        }
    }
}

fn not_changed() -> OpResult {
    OpResult::unsupported("not a changed construct; live semantic queries are not available yet")
}
