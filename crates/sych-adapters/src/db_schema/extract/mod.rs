//! Static ORM application model extractors (zero untrusted code execution) and
//! local-CLI-only external adapter protocol.

pub mod external_adapter;
pub mod go_bun;
pub mod go_ent;
pub mod go_gorm;
pub mod naming;
pub mod prisma;
pub mod py_django;
pub mod py_sqlalchemy;
pub mod rs_diesel;
pub mod rs_seaorm;
pub mod source;
pub mod ts_drizzle;

use std::collections::BTreeMap;

use sych_core::SchemaVersion;
use sych_core::db_schema::{
    DbModelManifest, DbModelTable, ModelExtractionSettings, ModelExtractorKind,
};

pub use external_adapter::run_external_adapter;
pub use source::SourceTree;

/// Runs the configured built-in static ORM extractors over `tree` and returns
/// a deterministic, deduplicated [`DbModelManifest`].
pub fn extract_models(
    tree: &SourceTree<'_>,
    settings: &ModelExtractionSettings,
) -> DbModelManifest {
    let active = resolve_extractors(&settings.extractors);
    if active.is_empty() {
        return DbModelManifest::new(None);
    }

    let mut used_extractors = Vec::new();
    let mut merged: BTreeMap<(Option<String>, String), DbModelTable> = BTreeMap::new();

    for kind in active {
        let extracted = match kind {
            ModelExtractorKind::Gorm => go_gorm::extract(tree, settings),
            ModelExtractorKind::Bun => go_bun::extract(tree, settings),
            ModelExtractorKind::Ent => go_ent::extract(tree, settings),
            ModelExtractorKind::Prisma => prisma::extract(tree, settings),
            ModelExtractorKind::Drizzle => ts_drizzle::extract(tree, settings),
            ModelExtractorKind::Sqlalchemy => py_sqlalchemy::extract(tree, settings),
            ModelExtractorKind::Django => py_django::extract(tree, settings),
            ModelExtractorKind::Diesel => rs_diesel::extract(tree, settings),
            ModelExtractorKind::Seaorm => rs_seaorm::extract(tree, settings),
            ModelExtractorKind::Auto | ModelExtractorKind::None => Vec::new(),
        };
        if !extracted.is_empty() {
            used_extractors.push(kind.as_str());
        }

        for table in extracted {
            let key = (table.schema.clone(), table.name.clone());
            merged
                .entry(key)
                .and_modify(|existing| merge_tables(existing, &table))
                .or_insert(table);
        }
    }

    let extractor = if used_extractors.is_empty() {
        None
    } else {
        Some(used_extractors.join(","))
    };

    DbModelManifest {
        schema_version: SchemaVersion,
        extractor,
        tables: merged.into_values().collect(),
        limitations: Vec::new(),
    }
}

fn resolve_extractors(configured: &[ModelExtractorKind]) -> Vec<ModelExtractorKind> {
    if configured.contains(&ModelExtractorKind::None) {
        return Vec::new();
    }

    if configured.is_empty() || configured.contains(&ModelExtractorKind::Auto) {
        return ModelExtractorKind::ALL_CONCRETE.to_vec();
    }

    let mut out = Vec::new();
    for &kind in configured {
        if !out.contains(&kind) {
            out.push(kind);
        }
    }
    out
}

fn merge_tables(dst: &mut DbModelTable, src: &DbModelTable) {
    if dst.source.is_none() {
        dst.source.clone_from(&src.source);
    }

    for col in &src.columns {
        if let Some(existing) = dst.columns.iter_mut().find(|c| c.name == col.name) {
            existing.required |= col.required;
            existing.unique |= col.unique;
            if existing.max_length.is_none() {
                existing.max_length = col.max_length;
            }

            if existing.enum_values.is_empty() {
                existing.enum_values.clone_from(&col.enum_values);
            }
        } else {
            dst.columns.push(col.clone());
        }
    }
    if dst.primary_key.is_empty() {
        dst.primary_key.clone_from(&src.primary_key);
    }

    if dst.join_table_for.is_empty() {
        dst.join_table_for.clone_from(&src.join_table_for);
    }

    for uq in &src.uniques {
        if !dst.uniques.contains(uq) {
            dst.uniques.push(uq.clone());
        }
    }
    for rel in &src.relations {
        if !dst.relations.contains(rel) {
            dst.relations.push(rel.clone());
        }
    }
}
