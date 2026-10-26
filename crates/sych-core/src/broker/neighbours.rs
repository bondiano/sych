//! Depth-1 neighbourhood of a changed construct from `SemanticFacts`: the construct itself,
//! diagnostics, callers, callees, used type definitions, implementations, writers, readers,
//! tests and references.

use super::build::{Candidate, Limits, RunState};
use super::{Priority, QueryRecord, Relation};
use crate::{
    CodeRef, Completeness, EdgeSet, PathClass, QueryStatus, RepoPath, SemanticFacts, SymbolFacts,
    SyntaxFacts, SyntaxKind, classify,
};

const MAX_DIAGNOSTICS_PER_FILE: usize = 5;

/// The link and the reason of a neighbour of `name`; a reference from a test is a test.
fn selected(relation: Relation, class: PathClass, name: &str) -> (Relation, String) {
    match (relation, class) {
        (Relation::ReferencedBy, PathClass::Test) => {
            (Relation::TestedBy, format!("test referencing `{name}`"))
        }
        (Relation::CalledBy, _) => (relation, format!("caller of `{name}`")),
        (Relation::Calls, _) => (relation, format!("called by `{name}`")),
        (Relation::UsesType, _) => (relation, format!("type used by `{name}`")),
        (Relation::ImplementedBy, _) => (relation, format!("implementation of `{name}`")),
        (Relation::ReadBy, _) => (relation, format!("reads `{name}`")),
        (Relation::WrittenBy, _) => (relation, format!("writes `{name}`")),
        _ => (relation, format!("reference to `{name}`")),
    }
}

pub(super) struct Neighbourhood<'s, 'a> {
    pub facts: &'a SemanticFacts,
    pub path: &'a RepoPath,
    pub limits: Limits,
    pub state: &'s mut RunState,
    pub requests: &'s mut u32,
    pub candidates: &'s mut Vec<Candidate>,
    pub queries: &'s mut Vec<QueryRecord>,
    pub gaps: &'s mut Vec<String>,
    /// `#[cfg(test)]` modules from syntax facts: references inside them are tests.
    pub syntax: &'a [SyntaxFacts],
}

impl Neighbourhood<'_, '_> {
    pub fn collect(&mut self) {
        let (facts, path) = (self.facts, self.path);
        for symbol in facts.symbols.iter().filter(|s| &s.definition.path == path) {
            self.symbol(symbol);
        }
        self.diagnostics();
    }

    fn symbol(&mut self, symbol: &SymbolFacts) {
        let side = self.facts.side;
        let evidence = Some(self.facts.evidence_id());
        {
            let def = &symbol.definition;
            let lines = def.end_line - def.start_line + 1;
            if lines <= self.limits.max_span_lines {
                self.push(
                    def.start_line,
                    def.end_line,
                    Priority::Required,
                    format!("enclosing construct `{}`", symbol.name),
                    None,
                );
            } else {
                self.gaps.push(format!(
                    "construct `{}` ({lines} lines) exceeds the span limit; only changed lines are included",
                    symbol.name
                ));
            }

            let edges = [
                ("find_callers", &symbol.callers, Relation::CalledBy),
                ("find_callees", &symbol.callees, Relation::Calls),
                (
                    "find_type_definitions",
                    &symbol.type_definitions,
                    Relation::UsesType,
                ),
                (
                    "find_implementations",
                    &symbol.implementations,
                    Relation::ImplementedBy,
                ),
                ("find_writers", &symbol.writers, Relation::WrittenBy),
                ("find_readers", &symbol.readers, Relation::ReadBy),
                (
                    "find_references",
                    &symbol.references,
                    Relation::ReferencedBy,
                ),
            ];
            let cost = edges
                .iter()
                .filter(|(_, e, _)| matches!(e.status, QueryStatus::Answered | QueryStatus::Failed))
                .count();
            let cost = u32::try_from(cost).unwrap_or(u32::MAX);
            let over = *self.requests + cost > self.limits.requests_per_unit
                || self.state.requests + cost > self.limits.requests_per_run;
            for (operation, edge, relation) in edges {
                self.queries.push(QueryRecord {
                    operation: operation.into(),
                    symbol: symbol.name.clone(),
                    side,
                    status: edge.status,
                    completeness: edge.completeness,
                });
                self.note_gaps(&symbol.name, operation, edge);
                if over {
                    continue;
                }

                for r in edge.refs.iter().filter(|r| *r != def) {
                    if matches!(relation, Relation::Calls | Relation::UsesType)
                        && r.end_line - r.start_line >= self.limits.max_span_lines
                    {
                        self.gaps.push(format!(
                            "definition used by `{}` at {}:{} exceeds the span limit",
                            symbol.name, r.path, r.start_line
                        ));
                        continue;
                    }

                    let (relation, reason) = selected(relation, self.class_of(r), &symbol.name);
                    self.candidates.push(Candidate {
                        side: r.side,
                        path: r.path.clone(),
                        start: r.start_line,
                        end: r.end_line,
                        // Another writer of the same data can break what the change relies on.
                        priority: if relation == Relation::WrittenBy {
                            Priority::Refuting
                        } else {
                            Priority::Supporting
                        },
                        reason,
                        evidence,
                        completeness: edge.completeness,
                        link: Some((def.path.clone(), def.start_line, relation)),
                    });
                }
            }
            if over {
                self.gaps.push(format!(
                    "SEMANTIC_REQUEST_LIMIT: neighbours of `{}` not included",
                    symbol.name
                ));
            } else {
                self.calling_constructs(symbol);

                *self.requests += cost;
                self.state.requests += cost;
            }
        }
    }

    /// Whole callers as supporting context; one too long for a span is still present
    /// through its call site.
    fn calling_constructs(&mut self, symbol: &SymbolFacts) {
        let def = &symbol.definition;
        let fits = |r: &&CodeRef| r.end_line - r.start_line < self.limits.max_span_lines;
        for r in symbol.calling_constructs.iter().filter(fits) {
            self.candidates.push(Candidate {
                side: r.side,
                path: r.path.clone(),
                start: r.start_line,
                end: r.end_line,
                priority: Priority::Supporting,
                reason: format!("construct calling `{}`", symbol.name),
                evidence: Some(self.facts.evidence_id()),
                completeness: symbol.callers.completeness,
                link: Some((def.path.clone(), def.start_line, Relation::CalledBy)),
            });
        }
    }

    /// Errors and warnings of the file as supporting context: with proc-macros off and
    /// dependencies missing many are analysis artefacts, so they never outrank the change.
    fn diagnostics(&mut self) {
        let (facts, path) = (self.facts, self.path);
        let relevant = facts.diagnostics.iter().filter(|d| {
            &d.code_ref.path == path && matches!(d.severity.as_deref(), Some("error" | "warning"))
        });
        for (i, d) in relevant.enumerate() {
            if i == MAX_DIAGNOSTICS_PER_FILE {
                self.gaps.push(format!("{path}: more than {MAX_DIAGNOSTICS_PER_FILE} diagnostics, rest in the semantic facts"));
                break;
            }

            let code = d.code.as_deref().unwrap_or("-");
            let message: String = d.message.chars().take(120).collect();
            let r = &d.code_ref;
            self.push(
                r.start_line,
                r.end_line,
                Priority::Supporting,
                format!(
                    "{} diagnostic {code} (may be an analysis artefact): {message}",
                    d.severity.as_deref().unwrap_or_default()
                ),
                None,
            );
        }
    }

    fn push(
        &mut self,
        start: u32,
        end: u32,
        priority: Priority,
        reason: String,
        link: Option<Relation>,
    ) {
        self.candidates.push(Candidate {
            side: self.facts.side,
            path: self.path.clone(),
            start,
            end,
            priority,
            reason,
            evidence: Some(self.facts.evidence_id()),
            completeness: Completeness::Partial,
            link: link.map(|rel| (self.path.clone(), start, rel)),
        });
    }

    fn class_of(&self, r: &CodeRef) -> PathClass {
        let in_test_module = self
            .syntax
            .iter()
            .filter(|d| d.side == r.side)
            .flat_map(|d| &d.facts)
            .any(|f| {
                f.kind == SyntaxKind::TestModule
                    && f.code_ref.path == r.path
                    && (f.code_ref.start_line..=f.code_ref.end_line).contains(&r.start_line)
            });
        if in_test_module {
            PathClass::Test
        } else {
            classify(r.path.as_str(), false)
        }
    }

    fn note_gaps(&mut self, symbol: &str, operation: &str, edge: &EdgeSet) {
        match edge.status {
            QueryStatus::Unsupported => self
                .gaps
                .push(format!("{operation} `{symbol}`: UNSUPPORTED")),
            QueryStatus::Failed => self.gaps.push(format!(
                "{operation} `{symbol}`: FAILED {}",
                edge.unresolved.join("; ")
            )),
            QueryStatus::Answered if !edge.unresolved.is_empty() => self.gaps.push(format!(
                "{operation} `{symbol}`: {} locations unresolved",
                edge.unresolved.len()
            )),
            _ => {}
        }
    }
}
