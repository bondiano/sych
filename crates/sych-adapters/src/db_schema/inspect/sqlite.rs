//! Read-only SQLite catalog inspection via `sqlite_master`, `PRAGMA` tables and
//! `sqlparser` extraction of `CHECK` constraints, partial predicates and expressions.

use std::collections::BTreeMap;
use std::str::FromStr;

use sqlparser::ast::{ColumnOption, CreateIndex, CreateTable, Statement, TableConstraint};
use sqlparser::dialect::SQLiteDialect;
use sqlparser::parser::Parser;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection, Row};
use sych_core::SchemaVersion;
use sych_core::db_schema::{
    DbCheck, DbColumn, DbForeignKey, DbIndex, DbIndexPart, DbSchema, DbSchemaSnapshot,
    DbSchemaSource, DbTable, DbTrigger, DbView, SqlDialect,
};
use tokio::time::timeout;

use super::InspectOptions;
use crate::db_schema::{DbSchemaError, classify_sql_type, redact_dsn};

fn normalize_sqlite_dsn(dsn: &str) -> String {
    let trimmed = dsn.trim();
    if trimmed.starts_with("sqlite:") || trimmed.starts_with("file:") {
        trimmed.to_owned()
    } else {
        format!("sqlite://{trimmed}")
    }
}

/// Opens a SQLite database in strict read-only mode (`create_if_missing(false)`) and
/// returns its normalized schema snapshot.
pub async fn inspect(
    dsn: &str,
    options: &InspectOptions,
) -> Result<DbSchemaSnapshot, DbSchemaError> {
    let safe_dsn = redact_dsn(dsn);
    let url = normalize_sqlite_dsn(dsn);
    let connect_opts = SqliteConnectOptions::from_str(&url)
        .map_err(|e| DbSchemaError::InvalidDsn {
            dsn: safe_dsn.clone(),
            detail: e.to_string(),
        })?
        .read_only(true)
        .create_if_missing(false);

    let secs = options.timeout.as_secs();
    timeout(options.timeout, async {
        let mut conn = connect_opts
            .connect()
            .await
            .map_err(|e| DbSchemaError::Connect {
                dsn: safe_dsn.clone(),
                detail: e.to_string(),
            })?;
        let res = read_schema(&mut conn, &safe_dsn).await;
        let _ = conn.close().await;
        res
    })
    .await
    .map_err(|_| DbSchemaError::Timeout {
        dsn: safe_dsn,
        seconds: secs,
    })?
}

async fn read_schema(
    conn: &mut sqlx::SqliteConnection,
    safe_dsn: &str,
) -> Result<DbSchemaSnapshot, DbSchemaError> {
    let qerr = |e: sqlx::Error| DbSchemaError::Query {
        dsn: safe_dsn.to_owned(),
        detail: e.to_string(),
    };
    let rows = sqlx::query(
        "SELECT type, name, tbl_name, sql FROM sqlite_master \
         WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(qerr)?;

    let mut table_sql: BTreeMap<String, String> = BTreeMap::new();
    let mut index_sql: BTreeMap<String, String> = BTreeMap::new();
    let mut views = Vec::new();
    let mut triggers = Vec::new();

    for row in rows {
        let kind: String = row.try_get(0).map_err(qerr)?;
        let name: String = row.try_get(1).map_err(qerr)?;
        let tbl_name: String = row.try_get(2).map_err(qerr)?;
        let sql: Option<String> = row.try_get(3).map_err(qerr)?;
        match kind.as_str() {
            "table" => {
                table_sql.insert(name, sql.unwrap_or_default());
            }
            "index" => {
                if let Some(s) = sql {
                    index_sql.insert(name, s);
                }
            }
            "view" => views.push(DbView {
                name,
                extension_owned: false,
            }),
            "trigger" => triggers.push(DbTrigger {
                table: tbl_name,
                name,
                extension_owned: false,
            }),
            _ => {}
        }
    }

    let mut tables = Vec::new();
    let mut limitations = Vec::new();
    for (name, create_sql) in &table_sql {
        let tbl = read_table(
            conn,
            name,
            create_sql,
            &index_sql,
            safe_dsn,
            &mut limitations,
        )
        .await?;
        tables.push(tbl);
    }

    Ok(DbSchemaSnapshot {
        schema_version: SchemaVersion,
        dialect: SqlDialect::Sqlite,
        source: DbSchemaSource::LiveDatabase,
        schemas: vec![DbSchema {
            name: SqlDialect::Sqlite.default_schema().to_owned(),
            tables,
            views,
            triggers,
            functions: Vec::new(),
        }],
        limitations,
    })
}

async fn read_table(
    conn: &mut sqlx::SqliteConnection,
    table_name: &str,
    create_sql: &str,
    index_sql: &BTreeMap<String, String>,
    safe_dsn: &str,
    limitations: &mut Vec<String>,
) -> Result<DbTable, DbSchemaError> {
    let qerr = |e: sqlx::Error| DbSchemaError::Query {
        dsn: safe_dsn.to_owned(),
        detail: e.to_string(),
    };
    let col_rows =
        sqlx::query(r#"SELECT name, type, "notnull", pk FROM pragma_table_info(?) ORDER BY cid"#)
            .bind(table_name)
            .fetch_all(&mut *conn)
            .await
            .map_err(qerr)?;

    let mut columns = Vec::new();
    let mut pk_cols: Vec<(i64, String)> = Vec::new();
    for row in col_rows {
        let name: String = row.try_get("name").map_err(qerr)?;
        let raw_type: String = row.try_get("type").map_err(qerr)?;
        let notnull: i64 = row.try_get("notnull").map_err(qerr)?;
        let pk_pos: i64 = row.try_get("pk").map_err(qerr)?;
        if pk_pos > 0 {
            pk_cols.push((pk_pos, name.clone()));
        }

        columns.push(DbColumn {
            name,
            data_type: classify_sql_type(SqlDialect::Sqlite, &raw_type, None),
            nullable: notnull == 0 && pk_pos == 0,
            source: None,
        });
    }
    pk_cols.sort_by_key(|(pos, _)| *pos);
    let primary_key = (!pk_cols.is_empty()).then(|| DbIndex {
        name: format!("{table_name}_pkey"),
        unique: true,
        method: String::new(),
        predicate: None,
        include: Vec::new(),
        parts: pk_cols
            .into_iter()
            .map(|(_, c)| DbIndexPart {
                column: Some(c),
                expression: None,
                desc: false,
                opclass: None,
                prefix_length: None,
            })
            .collect(),
        source: None,
    });

    let indexes = read_indexes(conn, table_name, index_sql, safe_dsn).await?;
    let (checks, fk_names) = parse_sqlite_table_sql(create_sql, table_name, limitations);
    let foreign_keys = read_foreign_keys(conn, table_name, &fk_names, safe_dsn).await?;

    Ok(DbTable {
        name: table_name.to_owned(),
        columns,
        primary_key,
        indexes,
        foreign_keys,
        checks,
        source: None,
    })
}

async fn read_indexes(
    conn: &mut sqlx::SqliteConnection,
    table_name: &str,
    index_sql: &BTreeMap<String, String>,
    safe_dsn: &str,
) -> Result<Vec<DbIndex>, DbSchemaError> {
    let qerr = |e: sqlx::Error| DbSchemaError::Query {
        dsn: safe_dsn.to_owned(),
        detail: e.to_string(),
    };
    let idx_rows =
        sqlx::query(r#"SELECT name, "unique", origin FROM pragma_index_list(?) ORDER BY seq"#)
            .bind(table_name)
            .fetch_all(&mut *conn)
            .await
            .map_err(qerr)?;

    let mut out = Vec::new();
    for row in idx_rows {
        let name: String = row.try_get("name").map_err(qerr)?;
        let unique: i64 = row.try_get("unique").map_err(qerr)?;
        let origin: String = row.try_get("origin").map_err(qerr)?;
        if origin == "pk" {
            continue;
        }

        let parsed_idx = index_sql
            .get(&name)
            .and_then(|sql| parse_sqlite_index_sql(sql));
        let xrows = sqlx::query(
            r#"SELECT cid, name, "desc", coll, "key" FROM pragma_index_xinfo(?) ORDER BY seqno"#,
        )
        .bind(&name)
        .fetch_all(&mut *conn)
        .await
        .map_err(qerr)?;

        let mut parts = Vec::new();
        for (i, xrow) in xrows.iter().enumerate() {
            let key: i64 = xrow.try_get("key").map_err(qerr)?;
            if key == 0 {
                continue;
            }

            let cid: i64 = xrow.try_get("cid").map_err(qerr)?;
            let col_name: Option<String> = xrow.try_get("name").map_err(qerr)?;
            let desc: i64 = xrow.try_get("desc").map_err(qerr)?;
            let coll: Option<String> = xrow.try_get("coll").map_err(qerr)?;
            let opclass = coll.filter(|c| !c.eq_ignore_ascii_case("BINARY"));
            if cid == -2 || col_name.is_none() {
                let expr = parsed_idx
                    .as_ref()
                    .and_then(|(_, exprs)| exprs.get(i).cloned())
                    .unwrap_or_else(|| "<expression>".to_owned());
                parts.push(DbIndexPart {
                    column: None,
                    expression: Some(expr),
                    desc: desc != 0,
                    opclass,
                    prefix_length: None,
                });
            } else {
                parts.push(DbIndexPart {
                    column: col_name,
                    expression: None,
                    desc: desc != 0,
                    opclass,
                    prefix_length: None,
                });
            }
        }
        out.push(DbIndex {
            name,
            unique: unique != 0,
            method: String::new(),
            predicate: parsed_idx.and_then(|(pred, _)| pred),
            include: Vec::new(),
            parts,
            source: None,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

type FkGroupMap = BTreeMap<i64, Vec<(i64, String, String, Option<String>)>>;
type FkNameMap = BTreeMap<(Vec<String>, String), String>;

async fn read_foreign_keys(
    conn: &mut sqlx::SqliteConnection,
    table_name: &str,
    fk_names: &FkNameMap,
    safe_dsn: &str,
) -> Result<Vec<DbForeignKey>, DbSchemaError> {
    let qerr = |e: sqlx::Error| DbSchemaError::Query {
        dsn: safe_dsn.to_owned(),
        detail: e.to_string(),
    };
    let rows = sqlx::query(
        r#"SELECT id, seq, "table", "from", "to" FROM pragma_foreign_key_list(?) ORDER BY id, seq"#,
    )
    .bind(table_name)
    .fetch_all(&mut *conn)
    .await
    .map_err(qerr)?;

    let mut groups: FkGroupMap = BTreeMap::new();
    for row in rows {
        let id: i64 = row.try_get("id").map_err(qerr)?;
        let seq: i64 = row.try_get("seq").map_err(qerr)?;
        let ref_table: String = row.try_get("table").map_err(qerr)?;
        let from: String = row.try_get("from").map_err(qerr)?;
        let to: Option<String> = row.try_get("to").map_err(qerr)?;
        groups
            .entry(id)
            .or_default()
            .push((seq, ref_table, from, to));
    }

    let mut out = Vec::new();
    for (_, mut items) in groups {
        items.sort_by_key(|(seq, _, _, _)| *seq);
        let ref_table = items[0].1.clone();
        let columns: Vec<String> = items.iter().map(|(_, _, from, _)| from.clone()).collect();
        let ref_columns: Vec<String> = items
            .iter()
            .filter_map(|(_, _, _, to)| to.clone())
            .collect();
        let name = fk_names
            .get(&(columns.clone(), ref_table.clone()))
            .cloned()
            .unwrap_or_else(|| format!("fk_{table_name}_{}", columns.join("_")));
        out.push(DbForeignKey {
            name,
            columns,
            ref_schema: Some(SqlDialect::Sqlite.default_schema().to_owned()),
            ref_table,
            ref_columns,
            source: None,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn parse_sqlite_table_sql(
    sql: &str,
    table_name: &str,
    limitations: &mut Vec<String>,
) -> (Vec<DbCheck>, FkNameMap) {
    let mut checks = Vec::new();
    let mut fk_names = BTreeMap::new();
    if sql.trim().is_empty() {
        return (checks, fk_names);
    }

    let Ok(stmts) = Parser::parse_sql(&SQLiteDialect {}, sql) else {
        limitations.push(format!(
            "could not parse SQLite CREATE TABLE SQL for `{table_name}`; CHECK constraints may be incomplete"
        ));
        return (checks, fk_names);
    };
    for stmt in stmts {
        if let Statement::CreateTable(CreateTable {
            columns,
            constraints,
            ..
        }) = stmt
        {
            for col in columns {
                for opt in col.options {
                    match opt.option {
                        ColumnOption::Check(chk) => checks.push(DbCheck {
                            name: opt.name.or(chk.name).map(|i| i.value),
                            expression: chk.expr.to_string(),
                        }),
                        ColumnOption::ForeignKey(fk) => {
                            if let Some(name) = opt.name.or(fk.name) {
                                let ref_tbl = fk
                                    .foreign_table
                                    .0
                                    .last()
                                    .map(|p| {
                                        p.to_string().trim_matches(['"', '`', '[', ']']).to_owned()
                                    })
                                    .unwrap_or_default();
                                fk_names
                                    .insert((vec![col.name.value.clone()], ref_tbl), name.value);
                            }
                        }
                        _ => {}
                    }
                }
            }
            for constraint in constraints {
                match constraint {
                    TableConstraint::Check(chk) => checks.push(DbCheck {
                        name: chk.name.map(|i| i.value),
                        expression: chk.expr.to_string(),
                    }),
                    TableConstraint::ForeignKey(fk) => {
                        if let Some(name) = fk.name {
                            let cols: Vec<String> =
                                fk.columns.into_iter().map(|i| i.value).collect();
                            let ref_tbl = fk
                                .foreign_table
                                .0
                                .last()
                                .map(|p| {
                                    p.to_string().trim_matches(['"', '`', '[', ']']).to_owned()
                                })
                                .unwrap_or_default();
                            fk_names.insert((cols, ref_tbl), name.value);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    (checks, fk_names)
}

fn parse_sqlite_index_sql(sql: &str) -> Option<(Option<String>, Vec<String>)> {
    let stmts = Parser::parse_sql(&SQLiteDialect {}, sql).ok()?;
    for stmt in stmts {
        if let Statement::CreateIndex(CreateIndex {
            predicate, columns, ..
        }) = stmt
        {
            let pred = predicate.map(|p| p.to_string());
            let exprs = columns
                .into_iter()
                .map(|c| c.column.expr.to_string())
                .collect();
            return Some((pred, exprs));
        }
    }
    None
}
