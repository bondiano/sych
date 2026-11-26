//! Static SQL DDL parser and schema reconstructor for PostgreSQL, MySQL and SQLite.
//!
//! Replays SQL schema and migration files in lexicographical order without a
//! live database, tracking file/line positions for inline review anchors and
//! recording any unparsed or procedural statements as explicit `limitations`.

mod ddl;
mod split;

use std::collections::BTreeMap;

use sqlparser::ast::{
    CreateFunction, CreateTrigger, CreateView, DropFunction, DropTrigger, ObjectName, Statement,
    UserDefinedTypeRepresentation,
};
use sqlparser::dialect::{Dialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect};
use sqlparser::parser::Parser;
use sych_core::SchemaVersion;
use sych_core::db_schema::{
    DbColumnType, DbFunction, DbSchema, DbSchemaSnapshot, DbSchemaSource, DbTrigger, DbView,
    SqlDialect,
};

use super::classify_sql_type;
use super::extract::SourceTree;
use split::{is_procedural_block, is_session_directive, split_sql_statements, unquote_ident};

/// One SQL schema or migration file with its repository-relative path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlFile {
    pub path: String,
    pub content: String,
}

/// A discovered group of SQL files targeting one logical dialect/directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlTarget {
    pub label: String,
    pub dialect: SqlDialect,
    pub files: Vec<SqlFile>,
}

/// Discovers SQL schema/migration targets in `tree`.
///
/// When `explicit_globs` is empty, searches common migration/schema locations
/// (`migrations/`, `db/`, `sql/`, `prisma/migrations/`, `drizzle/`, `schema*.sql`)
/// and skips rollback (`*.down.sql`), seed, test and fixture files.
pub fn discover_sql_targets(
    tree: &SourceTree<'_>,
    explicit_globs: &[String],
    dialect_hint: Option<SqlDialect>,
) -> Vec<SqlTarget> {
    let mut paths = tree.files_with_extensions(&["sql"], explicit_globs);
    if explicit_globs.is_empty() {
        paths.retain(|p| is_candidate_migration_path(p));
    }

    paths.sort();
    if paths.is_empty() {
        return Vec::new();
    }

    let mut groups: BTreeMap<(String, SqlDialect), Vec<SqlFile>> = BTreeMap::new();
    for path in paths {
        let Some(content) = tree.read_text(&path) else {
            continue;
        };
        let (label, dialect) = classify_sql_target(&path, &content, dialect_hint);
        groups
            .entry((label, dialect))
            .or_default()
            .push(SqlFile { path, content });
    }

    groups
        .into_iter()
        .map(|((label, dialect), files)| SqlTarget {
            label,
            dialect,
            files,
        })
        .collect()
}

fn is_candidate_migration_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let file = lower.rsplit('/').next().unwrap_or(&lower);
    if file.ends_with(".down.sql")
        || file == "down.sql"
        || file.starts_with("seed")
        || lower.contains("/fixtures/")
        || lower.contains("/testdata/")
        || lower.starts_with("fixtures/")
        || lower.starts_with("testdata/")
        || lower.starts_with("tests/")
        || lower.contains("/tests/")
    {
        return false;
    }

    lower.starts_with("migrations/")
        || lower.contains("/migrations/")
        || lower.starts_with("db/")
        || lower.contains("/db/")
        || lower.starts_with("sql/")
        || lower.starts_with("drizzle/")
        || lower.contains("/drizzle/")
        || file.starts_with("schema")
}

fn classify_sql_target(
    path: &str,
    content: &str,
    dialect_hint: Option<SqlDialect>,
) -> (String, SqlDialect) {
    let lower = path.to_ascii_lowercase();
    let path_dialect = if lower.contains("postgres") || lower.contains("/pg/") {
        Some(SqlDialect::Postgres)
    } else if lower.contains("mysql") || lower.contains("mariadb") {
        Some(SqlDialect::Mysql)
    } else if lower.contains("sqlite") {
        Some(SqlDialect::Sqlite)
    } else {
        None
    };

    let dialect = path_dialect
        .or(dialect_hint)
        .unwrap_or_else(|| infer_dialect_from_sql(content));
    let label = target_label_for_path(path);
    (label, dialect)
}

fn target_label_for_path(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 1 {
        return path.to_owned();
    }

    // e.g. `prisma/migrations/20260101_init/migration.sql` -> `prisma/migrations`
    if parts.len() >= 3 && parts[parts.len() - 1].eq_ignore_ascii_case("migration.sql") {
        return parts[..parts.len() - 2].join("/");
    }

    parts[..parts.len() - 1].join("/")
}

/// Infers SQL dialect from syntax cues when neither path nor config names one.
pub fn infer_dialect_from_sql(sql: &str) -> SqlDialect {
    let upper = sql.to_ascii_uppercase();
    if upper.contains("AUTOINCREMENT")
        || upper.contains("WITHOUT ROWID")
        || upper.contains("PRAGMA ")
    {
        return SqlDialect::Sqlite;
    }

    if upper.contains("AUTO_INCREMENT")
        || upper.contains("ENGINE=")
        || upper.contains("ENGINE =")
        || upper.contains("TINYINT(1)")
        || upper.contains("UNSIGNED")
    {
        return SqlDialect::Mysql;
    }

    SqlDialect::Postgres
}

/// Reconstructs a normalized [`DbSchemaSnapshot`] by replaying `files` in order.
pub fn parse_sql_files(dialect: SqlDialect, files: &[SqlFile]) -> DbSchemaSnapshot {
    let mut state = SchemaState::new(dialect);
    let parser_dialect: Box<dyn Dialect> = match dialect {
        SqlDialect::Postgres => Box::new(PostgreSqlDialect {}),
        SqlDialect::Mysql => Box::new(MySqlDialect {}),
        SqlDialect::Sqlite => Box::new(SQLiteDialect {}),
    };

    for file in files {
        let chunks = split_sql_statements(&file.content);
        for (start_line, chunk) in chunks {
            let trimmed = chunk.trim();
            if trimmed.is_empty() || is_session_directive(dialect, trimmed) {
                continue;
            }

            if is_procedural_block(trimmed) {
                state.limitations.push(format!(
                    "{}:{start_line}: skipped procedural SQL block; schema changes inside it are not reconstructed statically",
                    file.path
                ));
                continue;
            }

            match Parser::parse_sql(parser_dialect.as_ref(), trimmed) {
                Ok(stmts) => {
                    for stmt in stmts {
                        state.apply_statement(&file.path, start_line, stmt);
                    }
                }
                Err(err) => {
                    let preview: String = trimmed
                        .lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(80)
                        .collect();
                    state.limitations.push(format!(
                        "{}:{start_line}: could not parse SQL statement (`{preview}`): {err}",
                        file.path
                    ));
                }
            }
        }
    }

    state.finish()
}

pub(super) struct SchemaState {
    pub dialect: SqlDialect,
    pub default_schema: String,
    pub schemas: BTreeMap<String, DbSchema>,
    pub enums: BTreeMap<(String, String), Vec<String>>,
    pub limitations: Vec<String>,
}

impl SchemaState {
    fn new(dialect: SqlDialect) -> Self {
        Self {
            dialect,
            default_schema: dialect.default_schema().to_owned(),
            schemas: BTreeMap::new(),
            enums: BTreeMap::new(),
            limitations: Vec::new(),
        }
    }

    pub(super) fn schema_mut(&mut self, name: &str) -> &mut DbSchema {
        self.schemas
            .entry(name.to_owned())
            .or_insert_with(|| DbSchema {
                name: name.to_owned(),
                tables: Vec::new(),
                views: Vec::new(),
                triggers: Vec::new(),
                functions: Vec::new(),
            })
    }

    pub(super) fn split_name(&self, obj: &ObjectName) -> (String, String) {
        let parts: Vec<String> = obj
            .0
            .iter()
            .map(|p| unquote_ident(&p.to_string()))
            .collect();
        match parts.as_slice() {
            [.., schema, name] => (schema.clone(), name.clone()),
            [name] => (self.default_schema.clone(), name.clone()),
            [] => (self.default_schema.clone(), String::new()),
        }
    }

    fn apply_statement(&mut self, file: &str, base_line: u32, stmt: Statement) {
        match stmt {
            Statement::CreateType {
                name,
                representation: Some(UserDefinedTypeRepresentation::Enum { labels }),
            } => {
                let (schema, ty) = self.split_name(&name);
                let vals = labels.into_iter().map(|i| i.value).collect();
                self.enums.insert((schema, ty), vals);
            }
            Statement::CreateTable(ct) => self.apply_create_table(file, base_line, ct),
            Statement::AlterTable(at) => self.apply_alter_table(file, base_line, at),
            Statement::CreateIndex(ci) => self.apply_create_index(file, base_line, ci),
            Statement::CreateView(CreateView { name, .. }) => {
                let (schema, view) = self.split_name(&name);
                let s = self.schema_mut(&schema);
                if !s.views.iter().any(|v| v.name == view) {
                    s.views.push(DbView {
                        name: view,
                        extension_owned: false,
                    });
                }
            }
            Statement::CreateTrigger(CreateTrigger {
                name, table_name, ..
            }) => {
                let (schema, trig) = self.split_name(&name);
                let (_, tbl) = self.split_name(&table_name);
                let s = self.schema_mut(&schema);
                if !s.triggers.iter().any(|t| t.name == trig && t.table == tbl) {
                    s.triggers.push(DbTrigger {
                        table: tbl,
                        name: trig,
                        extension_owned: false,
                    });
                }
            }
            Statement::CreateFunction(CreateFunction { name, .. }) => {
                let (schema, func) = self.split_name(&name);
                let s = self.schema_mut(&schema);
                if !s.functions.iter().any(|f| f.name == func) {
                    s.functions.push(DbFunction {
                        name: func,
                        extension_owned: false,
                    });
                }
            }
            Statement::Drop {
                object_type, names, ..
            } => self.apply_drop(object_type, names),
            Statement::DropTrigger(DropTrigger { trigger_name, .. }) => {
                let (_, name) = self.split_name(&trigger_name);
                for s in self.schemas.values_mut() {
                    s.triggers.retain(|t| t.name != name);
                }
            }
            Statement::DropFunction(DropFunction { func_desc, .. }) => {
                for desc in func_desc {
                    let (schema_name, name) = self.split_name(&desc.name);
                    if let Some(s) = self.schemas.get_mut(&schema_name) {
                        s.functions.retain(|f| f.name != name);
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn resolve_column_type(&self, schema_name: &str, raw_type: &str) -> DbColumnType {
        let trimmed = raw_type.trim();
        let (enum_schema, enum_name) = match trimmed.split_once('.') {
            Some((s, n)) => (unquote_ident(s), unquote_ident(n)),
            None => (schema_name.to_owned(), unquote_ident(trimmed)),
        };
        let enum_info = self
            .enums
            .get(&(enum_schema.clone(), enum_name.clone()))
            .or_else(|| {
                self.enums
                    .get(&(self.default_schema.clone(), enum_name.clone()))
            })
            .map(|vals| (Some(format!("{enum_schema}.{enum_name}")), vals.clone()));

        classify_sql_type(self.dialect, raw_type, enum_info)
    }

    fn finish(mut self) -> DbSchemaSnapshot {
        for s in self.schemas.values_mut() {
            s.tables.sort_by(|a, b| a.name.cmp(&b.name));
            for t in &mut s.tables {
                t.indexes.sort_by(|a, b| a.name.cmp(&b.name));
                t.foreign_keys.sort_by(|a, b| a.name.cmp(&b.name));
            }
            s.views.sort_by(|a, b| a.name.cmp(&b.name));
            s.triggers
                .sort_by(|a, b| a.table.cmp(&b.table).then_with(|| a.name.cmp(&b.name)));
            s.functions.sort_by(|a, b| a.name.cmp(&b.name));
        }
        DbSchemaSnapshot {
            schema_version: SchemaVersion,
            dialect: self.dialect,
            source: DbSchemaSource::StaticSql,
            schemas: self.schemas.into_values().collect(),
            limitations: self.limitations,
        }
    }
}
