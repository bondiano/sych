//! Structural index comparison independent of index names.

use std::fmt::Write as _;

use crate::db_schema::model::{DbIndex, DbIndexPart, DbTable};

/// What an index can serve, independent of its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct IndexShape {
    /// Lowercase access method; empty string means default B-tree.
    pub method: String,
    /// Normalized partial predicate; empty string when total.
    pub predicate: String,
    /// Non-key covering INCLUDE columns joined by comma.
    pub include: String,
    /// One canonical key per index key part.
    pub parts: Vec<String>,
}

impl IndexShape {
    /// Builds the canonical shape of an index or primary key.
    pub fn of(idx: &DbIndex) -> Self {
        let mut method = idx.method.trim().to_ascii_lowercase();
        if method == "btree" {
            method.clear();
        }

        let predicate = idx
            .predicate
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .to_owned();
        let include = idx.include.join(",");
        let parts = idx.parts.iter().map(part_key).collect();
        Self {
            method,
            predicate,
            include,
            parts,
        }
    }

    /// Primary key viewed as an index shape, or `None` when the table has none.
    pub fn primary_key(table: &DbTable) -> Option<Self> {
        let pk = table.primary_key.as_ref()?;
        if pk.parts.is_empty() {
            return None;
        }

        Some(Self::of(pk))
    }

    /// Only B-tree indexes have ordered prefix lookup semantics.
    pub fn is_btree(&self) -> bool {
        self.method.is_empty()
    }

    /// Reports whether `self.parts` are the leading key parts of `other.parts`.
    pub fn is_prefix_of(&self, other: &Self) -> bool {
        self.parts.len() <= other.parts.len()
            && self
                .parts
                .iter()
                .zip(&other.parts)
                .all(|(left, right)| left == right)
    }

    /// Exact functional equivalence between two index shapes.
    pub fn same_as(&self, other: &Self) -> bool {
        self.method == other.method
            && self.predicate == other.predicate
            && self.include == other.include
            && self.parts.len() == other.parts.len()
            && self.is_prefix_of(other)
    }
}

fn part_key(part: &DbIndexPart) -> String {
    let mut out = String::new();
    if let Some(col) = &part.column {
        out.push_str("c:");
        out.push_str(col);
    } else {
        out.push_str("x:");
        out.push_str(part.expression.as_deref().unwrap_or(""));
    }

    if part.desc {
        out.push_str(" desc");
    }

    if let Some(opclass) = &part.opclass {
        let _ = write!(out, " opclass:{opclass}");
    }

    if let Some(prefix) = part.prefix_length {
        let _ = write!(out, " prefix:{prefix}");
    }

    out
}

/// Column names (or expressions) an index is keyed on, for diagnostic messages.
pub(super) fn index_columns(idx: &DbIndex) -> Vec<&str> {
    idx.parts.iter().map(DbIndexPart::display_name).collect()
}
