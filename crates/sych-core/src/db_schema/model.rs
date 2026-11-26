//! Normalized database schema model and structured finding types.
//!
//! Pure data contracts shared by live database inspectors, static SQL DDL
//! reconstruction, rule checks, standalone CLI reports and review facts.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::SchemaVersion;

/// Supported relational database engines.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SqlDialect {
    Postgres,
    Mysql,
    Sqlite,
}

impl SqlDialect {
    /// All supported dialects in deterministic order.
    pub const ALL: &'static [Self] = &[Self::Postgres, Self::Mysql, Self::Sqlite];

    /// Stable lowercase dialect identifier.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Mysql => "mysql",
            Self::Sqlite => "sqlite",
        }
    }

    /// Default schema name when none is specified explicitly.
    pub const fn default_schema(self) -> &'static str {
        match self {
            Self::Postgres => "public",
            Self::Mysql => "default",
            Self::Sqlite => "main",
        }
    }
}

impl fmt::Display for SqlDialect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SqlDialect {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "postgres" | "postgresql" | "pg" => Ok(Self::Postgres),
            "mysql" | "mariadb" => Ok(Self::Mysql),
            "sqlite" | "sqlite3" => Ok(Self::Sqlite),
            other => Err(format!(
                "unsupported SQL dialect `{other}` (expected postgres, mysql or sqlite)"
            )),
        }
    }
}

/// How a database schema snapshot was obtained.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DbSchemaSource {
    /// Direct read-only catalog inspection against a live database.
    #[default]
    LiveDatabase,
    /// Schema snapshot exported in CI and imported as a bound artifact.
    CiSnapshot,
    /// Reconstructed statically from SQL schema/migration files.
    StaticSql,
}

/// One source location in application model code or a SQL migration file.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct DbSourcePosition {
    pub file: String,
    pub line: u32,
}

/// Normalized structural snapshot of a database schema: no row data, no credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbSchemaSnapshot {
    pub schema_version: SchemaVersion,
    pub dialect: SqlDialect,
    #[serde(default)]
    pub source: DbSchemaSource,
    pub schemas: Vec<DbSchema>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limitations: Vec<String>,
}

impl DbSchemaSnapshot {
    /// Creates an empty snapshot for `dialect` and `source`.
    pub fn new(dialect: SqlDialect, source: DbSchemaSource) -> Self {
        Self {
            schema_version: SchemaVersion,
            dialect,
            source,
            schemas: Vec::new(),
            limitations: Vec::new(),
        }
    }

    /// Finds a schema by name, or the only schema when `name` is empty/none.
    pub fn find_schema(&self, name: Option<&str>) -> Option<&DbSchema> {
        match name.filter(|s| !s.is_empty()) {
            None => {
                if self.schemas.len() == 1 {
                    self.schemas.first()
                } else {
                    None
                }
            }
            Some(target) => self.schemas.iter().find(|s| s.name == target),
        }
    }
}

/// One database schema / namespace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbSchema {
    pub name: String,
    #[serde(default)]
    pub tables: Vec<DbTable>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub views: Vec<DbView>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<DbTrigger>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub functions: Vec<DbFunction>,
}

impl DbSchema {
    /// Looks up a table by exact physical name.
    pub fn table(&self, name: &str) -> Option<&DbTable> {
        self.tables.iter().find(|t| t.name == name)
    }

    /// Mutable lookup by exact physical name.
    pub fn table_mut(&mut self, name: &str) -> Option<&mut DbTable> {
        self.tables.iter_mut().find(|t| t.name == name)
    }
}

/// One database table with its columns, keys, indexes and check constraints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbTable {
    pub name: String,
    #[serde(default)]
    pub columns: Vec<DbColumn>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_key: Option<DbIndex>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub indexes: Vec<DbIndex>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub foreign_keys: Vec<DbForeignKey>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<DbCheck>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<DbSourcePosition>,
}

impl DbTable {
    /// Looks up a column by exact physical name.
    pub fn column(&self, name: &str) -> Option<&DbColumn> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// Names of primary key columns, in key order.
    pub fn primary_key_columns(&self) -> Vec<&str> {
        self.primary_key
            .as_ref()
            .map(|pk| {
                pk.parts
                    .iter()
                    .filter_map(|p| p.column.as_deref())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Normalized SQL column type carrying the exact properties the rules inspect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DbColumnType {
    /// PostgreSQL `boolean`, MySQL `tinyint(1)` / `boolean`, SQLite declared `BOOL`/`BOOLEAN`.
    Boolean,
    /// Integer column; `bits` is `Some(8|16|24|32|64)` on PG/MySQL, `None` on SQLite.
    Integer {
        raw: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bits: Option<u8>,
    },
    /// Character/string column (`varchar`, `char`, `text`, etc.).
    String {
        raw: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        char_max_len: Option<u32>,
        /// True for length-limited SQL character types (`varchar`, `char`, `bpchar`, ...).
        #[serde(default)]
        is_char_type: bool,
    },
    /// Native database enum type (PostgreSQL `CREATE TYPE ... AS ENUM`, MySQL `ENUM(...)`).
    Enum {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        type_name: Option<String>,
        values: Vec<String>,
    },
    /// Any other SQL type.
    Other { raw: String },
}

/// One column of a database table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbColumn {
    pub name: String,
    pub data_type: DbColumnType,
    pub nullable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<DbSourcePosition>,
}

/// One key part of a database index or primary key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbIndexPart {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub desc: bool,
    /// Operator class or collation modifier that changes search semantics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opclass: Option<String>,
    /// MySQL prefix sub-part length (`col(20)`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_length: Option<u32>,
}

impl DbIndexPart {
    /// Column name or expression text of this index part.
    pub fn display_name(&self) -> &str {
        self.column
            .as_deref()
            .or(self.expression.as_deref())
            .unwrap_or("")
    }
}

/// One database index or primary key constraint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbIndex {
    pub name: String,
    #[serde(default)]
    pub unique: bool,
    /// Access method in lowercase; empty or `"btree"` means default B-tree.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub method: String,
    /// Partial index `WHERE` predicate; `None` when total.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicate: Option<String>,
    /// Non-key covering `INCLUDE` columns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    pub parts: Vec<DbIndexPart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<DbSourcePosition>,
}

impl DbIndex {
    /// Whether this index is partial (restricted by a `WHERE` predicate).
    pub fn is_partial(&self) -> bool {
        self.predicate
            .as_deref()
            .is_some_and(|p| !p.trim().is_empty())
    }
}

/// One foreign key constraint on a table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbForeignKey {
    pub name: String,
    pub columns: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_schema: Option<String>,
    pub ref_table: String,
    pub ref_columns: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<DbSourcePosition>,
}

/// One table or column `CHECK` constraint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbCheck {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub expression: String,
}

/// A database view or materialized view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbView {
    pub name: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub extension_owned: bool,
}

/// A user-defined trigger on a table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbTrigger {
    pub table: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub extension_owned: bool,
}

/// A user-defined database function or stored procedure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbFunction {
    pub name: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub extension_owned: bool,
}

/// Database object a finding addresses.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbSchemaObject {
    pub schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreign_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
}

impl DbSchemaObject {
    /// Formats the object in the exact exclusion syntax (`schema.table[.kind:NAME]`).
    pub fn format_exclusion(&self) -> String {
        if let Some(view) = &self.view {
            return format!("{}.view:{view}", self.schema);
        }

        if let Some(func) = &self.function {
            return format!("{}.function:{func}", self.schema);
        }

        let mut out = format!("{}.{}", self.schema, self.table.as_deref().unwrap_or(""));
        if let Some(col) = &self.column {
            out.push_str(".column:");
            out.push_str(col);
        } else if let Some(idx) = &self.index {
            out.push_str(".index:");
            out.push_str(idx);
        } else if let Some(fk) = &self.foreign_key {
            out.push_str(".foreign_key:");
            out.push_str(fk);
        } else if let Some(trg) = &self.trigger {
            out.push_str(".trigger:");
            out.push_str(trg);
        }

        out
    }

    /// Formats the object as a compact dotted display path (`schema.table.column`).
    pub fn display_path(&self) -> String {
        let mut parts = vec![self.schema.as_str()];
        for part in [
            self.table.as_deref(),
            self.column.as_deref(),
            self.index.as_deref(),
            self.foreign_key.as_deref(),
            self.trigger.as_deref(),
            self.view.as_deref(),
            self.function.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if !part.is_empty() {
                parts.push(part);
            }
        }
        parts.join(".")
    }

    /// Most specific logical kind for SARIF `logicalLocations`.
    pub fn sarif_kind(&self) -> &'static str {
        if self.function.is_some() {
            "function"
        } else if self.view.is_some() {
            "view"
        } else if self.trigger.is_some() {
            "trigger"
        } else if self.foreign_key.is_some() {
            "foreign key"
        } else if self.index.is_some() {
            "index"
        } else if self.column.is_some() {
            "column"
        } else {
            "table"
        }
    }

    fn sort_key(&self) -> [&str; 8] {
        [
            self.schema.as_str(),
            self.table.as_deref().unwrap_or(""),
            self.column.as_deref().unwrap_or(""),
            self.index.as_deref().unwrap_or(""),
            self.foreign_key.as_deref().unwrap_or(""),
            self.trigger.as_deref().unwrap_or(""),
            self.view.as_deref().unwrap_or(""),
            self.function.as_deref().unwrap_or(""),
        ]
    }
}

impl Ord for DbSchemaObject {
    fn cmp(&self, other: &Self) -> Ordering {
        self.sort_key().cmp(&other.sort_key())
    }
}

impl PartialOrd for DbSchemaObject {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// One structured schema finding emitted by a deterministic rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbSchemaFinding {
    pub rule: String,
    pub object: DbSchemaObject,
    pub message: String,
    pub fix: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<DbSourcePosition>,
}

/// Sorts findings deterministically by rule, object and message.
pub fn sort_findings(findings: &mut [DbSchemaFinding]) {
    findings.sort_by(|a, b| {
        a.rule
            .cmp(&b.rule)
            .then_with(|| a.object.cmp(&b.object))
            .then_with(|| a.message.cmp(&b.message))
    });
}
