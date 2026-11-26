//! Read-only MySQL / MariaDB catalog inspection via `information_schema`.

use std::collections::BTreeMap;
use std::str::FromStr;

use sqlx::mysql::MySqlConnectOptions;
use sqlx::{ConnectOptions, Connection, Executor, MySqlConnection, Row};
use sych_core::SchemaVersion;
use sych_core::db_schema::{
    DbCheck, DbColumn, DbForeignKey, DbFunction, DbIndex, DbIndexPart, DbSchema, DbSchemaSnapshot,
    DbSchemaSource, DbTable, DbTrigger, DbView, SqlDialect,
};
use tokio::time::timeout;

use super::InspectOptions;
use crate::db_schema::{DbSchemaError, classify_sql_type, redact_dsn};

/// Connects to MySQL with `SET SESSION TRANSACTION READ ONLY` and reads `information_schema`.
pub async fn inspect(
    dsn: &str,
    options: &InspectOptions,
) -> Result<DbSchemaSnapshot, DbSchemaError> {
    let safe_dsn = redact_dsn(dsn);
    let connect_opts =
        MySqlConnectOptions::from_str(dsn.trim()).map_err(|e| DbSchemaError::InvalidDsn {
            dsn: safe_dsn.clone(),
            detail: e.to_string(),
        })?;

    let secs = options.timeout.as_secs();
    let ms = options.timeout.as_millis().min(u128::from(u32::MAX));
    timeout(options.timeout, async {
        let mut conn = connect_opts
            .connect()
            .await
            .map_err(|e| DbSchemaError::Connect {
                dsn: safe_dsn.clone(),
                detail: e.to_string(),
            })?;
        conn.execute("SET SESSION TRANSACTION READ ONLY")
            .await
            .map_err(|e| DbSchemaError::Query {
                dsn: safe_dsn.clone(),
                detail: format!("read-only session failed: {e}"),
            })?;
        // Best-effort query timeout (supported on MySQL 5.7+; ignored on MariaDB if unavailable).
        let _ = sqlx::query("SET SESSION max_execution_time = ?")
            .bind(u32::try_from(ms).unwrap_or(u32::MAX))
            .execute(&mut conn)
            .await;

        let res = read_all(&mut conn, options, &safe_dsn).await;
        let _ = conn.close().await;
        res
    })
    .await
    .map_err(|_| DbSchemaError::Timeout {
        dsn: safe_dsn,
        seconds: secs,
    })?
}

async fn read_all(
    conn: &mut MySqlConnection,
    options: &InspectOptions,
    safe_dsn: &str,
) -> Result<DbSchemaSnapshot, DbSchemaError> {
    let qerr = |e: sqlx::Error| DbSchemaError::Query {
        dsn: safe_dsn.to_owned(),
        detail: e.to_string(),
    };
    let schema_names = resolve_schemas(conn, options, safe_dsn).await?;
    let mut schemas = Vec::new();
    let mut limitations = Vec::new();

    for schema_name in schema_names {
        let mut tables: BTreeMap<String, DbTable> = BTreeMap::new();
        for row in sqlx::query(
            "SELECT TABLE_NAME FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = ? AND TABLE_TYPE = 'BASE TABLE' \
             ORDER BY TABLE_NAME",
        )
        .bind(&schema_name)
        .fetch_all(&mut *conn)
        .await
        .map_err(qerr)?
        {
            let name: String = row.try_get(0).map_err(qerr)?;
            tables.insert(
                name.clone(),
                DbTable {
                    name,
                    columns: Vec::new(),
                    primary_key: None,
                    indexes: Vec::new(),
                    foreign_keys: Vec::new(),
                    checks: Vec::new(),
                    source: None,
                },
            );
        }

        attach_columns(conn, &schema_name, &mut tables, safe_dsn).await?;
        attach_indexes(conn, &schema_name, &mut tables, safe_dsn).await?;
        attach_fks(conn, &schema_name, &mut tables, safe_dsn).await?;
        attach_checks(conn, &schema_name, &mut tables, &mut limitations).await;

        let (views, triggers, functions) = read_objects(conn, &schema_name, safe_dsn).await?;
        schemas.push(DbSchema {
            name: schema_name,
            tables: tables.into_values().collect(),
            views,
            triggers,
            functions,
        });
    }

    Ok(DbSchemaSnapshot {
        schema_version: SchemaVersion,
        dialect: SqlDialect::Mysql,
        source: DbSchemaSource::LiveDatabase,
        schemas,
        limitations,
    })
}

async fn resolve_schemas(
    conn: &mut MySqlConnection,
    options: &InspectOptions,
    safe_dsn: &str,
) -> Result<Vec<String>, DbSchemaError> {
    let qerr = |e: sqlx::Error| DbSchemaError::Query {
        dsn: safe_dsn.to_owned(),
        detail: e.to_string(),
    };
    if !options.schemas.is_empty() {
        return Ok(options.schemas.clone());
    }

    let current: Option<String> = sqlx::query_scalar("SELECT DATABASE()")
        .fetch_one(&mut *conn)
        .await
        .map_err(qerr)?;
    if let Some(db) = current.filter(|s| !s.is_empty()) {
        return Ok(vec![db]);
    }

    sqlx::query_scalar(
        "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA \
         WHERE SCHEMA_NAME NOT IN ('information_schema', 'mysql', 'performance_schema', 'sys') \
         ORDER BY SCHEMA_NAME",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(qerr)
}

async fn attach_columns(
    conn: &mut MySqlConnection,
    schema: &str,
    tables: &mut BTreeMap<String, DbTable>,
    safe_dsn: &str,
) -> Result<(), DbSchemaError> {
    let qerr = |e: sqlx::Error| DbSchemaError::Query {
        dsn: safe_dsn.to_owned(),
        detail: e.to_string(),
    };
    for row in sqlx::query(
        "SELECT TABLE_NAME, COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE \
         FROM information_schema.COLUMNS \
         WHERE TABLE_SCHEMA = ? \
         ORDER BY TABLE_NAME, ORDINAL_POSITION",
    )
    .bind(schema)
    .fetch_all(&mut *conn)
    .await
    .map_err(qerr)?
    {
        let table_name: String = row.try_get(0).map_err(qerr)?;
        let col_name: String = row.try_get(1).map_err(qerr)?;
        let col_type: String = row.try_get(2).map_err(qerr)?;
        let is_nullable: String = row.try_get(3).map_err(qerr)?;
        if let Some(table) = tables.get_mut(&table_name) {
            table.columns.push(DbColumn {
                name: col_name,
                data_type: classify_sql_type(SqlDialect::Mysql, &col_type, None),
                nullable: is_nullable.eq_ignore_ascii_case("YES"),
                source: None,
            });
        }
    }
    Ok(())
}

async fn attach_indexes(
    conn: &mut MySqlConnection,
    schema: &str,
    tables: &mut BTreeMap<String, DbTable>,
    safe_dsn: &str,
) -> Result<(), DbSchemaError> {
    let qerr = |e: sqlx::Error| DbSchemaError::Query {
        dsn: safe_dsn.to_owned(),
        detail: e.to_string(),
    };
    // MySQL 8+ exposes EXPRESSION; try with EXPRESSION first and fall back if older server.
    let rows = sqlx::query(
        "SELECT TABLE_NAME, INDEX_NAME, NON_UNIQUE, SEQ_IN_INDEX, COLUMN_NAME, \
                SUB_PART, COLLATION, INDEX_TYPE, EXPRESSION \
         FROM information_schema.STATISTICS \
         WHERE TABLE_SCHEMA = ? \
         ORDER BY TABLE_NAME, INDEX_NAME, SEQ_IN_INDEX",
    )
    .bind(schema)
    .fetch_all(&mut *conn)
    .await;

    let mut grouped: BTreeMap<(String, String), (bool, String, Vec<DbIndexPart>)> = BTreeMap::new();
    match rows {
        Ok(rows) => {
            for row in rows {
                let tbl: String = row.try_get(0).map_err(qerr)?;
                let idx: String = row.try_get(1).map_err(qerr)?;
                let non_unique: i64 = row.try_get(2).map_err(qerr)?;
                let col: Option<String> = row.try_get(4).map_err(qerr)?;
                let sub_part: Option<i64> = row.try_get(5).map_err(qerr)?;
                let collation: Option<String> = row.try_get(6).map_err(qerr)?;
                let method: String = row.try_get(7).map_err(qerr)?;
                let expr: Option<String> = row.try_get(8).map_err(qerr)?;
                let entry = grouped
                    .entry((tbl, idx))
                    .or_insert_with(|| (non_unique == 0, method.to_ascii_lowercase(), Vec::new()));
                entry.2.push(DbIndexPart {
                    column: col,
                    expression: expr,
                    desc: collation.as_deref() == Some("D"),
                    opclass: None,
                    prefix_length: sub_part.and_then(|n| u32::try_from(n).ok()),
                });
            }
        }
        Err(_) => {
            for row in sqlx::query(
                "SELECT TABLE_NAME, INDEX_NAME, NON_UNIQUE, SEQ_IN_INDEX, COLUMN_NAME, \
                        SUB_PART, COLLATION, INDEX_TYPE \
                 FROM information_schema.STATISTICS \
                 WHERE TABLE_SCHEMA = ? \
                 ORDER BY TABLE_NAME, INDEX_NAME, SEQ_IN_INDEX",
            )
            .bind(schema)
            .fetch_all(&mut *conn)
            .await
            .map_err(qerr)?
            {
                let tbl: String = row.try_get(0).map_err(qerr)?;
                let idx: String = row.try_get(1).map_err(qerr)?;
                let non_unique: i64 = row.try_get(2).map_err(qerr)?;
                let col: Option<String> = row.try_get(4).map_err(qerr)?;
                let sub_part: Option<i64> = row.try_get(5).map_err(qerr)?;
                let collation: Option<String> = row.try_get(6).map_err(qerr)?;
                let method: String = row.try_get(7).map_err(qerr)?;
                let entry = grouped
                    .entry((tbl, idx))
                    .or_insert_with(|| (non_unique == 0, method.to_ascii_lowercase(), Vec::new()));
                entry.2.push(DbIndexPart {
                    column: col,
                    expression: None,
                    desc: collation.as_deref() == Some("D"),
                    opclass: None,
                    prefix_length: sub_part.and_then(|n| u32::try_from(n).ok()),
                });
            }
        }
    }

    for ((tbl, name), (unique, method, parts)) in grouped {
        if let Some(table) = tables.get_mut(&tbl) {
            let idx = DbIndex {
                name: name.clone(),
                unique,
                method,
                predicate: None,
                include: Vec::new(),
                parts,
                source: None,
            };
            if name == "PRIMARY" {
                table.primary_key = Some(idx);
            } else {
                table.indexes.push(idx);
            }
        }
    }
    Ok(())
}

type GroupedFks = BTreeMap<(String, String), (String, String, Vec<String>, Vec<String>)>;

async fn attach_fks(
    conn: &mut MySqlConnection,
    schema: &str,
    tables: &mut BTreeMap<String, DbTable>,
    safe_dsn: &str,
) -> Result<(), DbSchemaError> {
    let qerr = |e: sqlx::Error| DbSchemaError::Query {
        dsn: safe_dsn.to_owned(),
        detail: e.to_string(),
    };
    let mut grouped: GroupedFks = BTreeMap::new();
    for row in sqlx::query(
        "SELECT TABLE_NAME, CONSTRAINT_NAME, COLUMN_NAME, \
                REFERENCED_TABLE_SCHEMA, REFERENCED_TABLE_NAME, REFERENCED_COLUMN_NAME \
         FROM information_schema.KEY_COLUMN_USAGE \
         WHERE TABLE_SCHEMA = ? AND REFERENCED_TABLE_NAME IS NOT NULL \
         ORDER BY TABLE_NAME, CONSTRAINT_NAME, ORDINAL_POSITION",
    )
    .bind(schema)
    .fetch_all(&mut *conn)
    .await
    .map_err(qerr)?
    {
        let tbl: String = row.try_get(0).map_err(qerr)?;
        let name: String = row.try_get(1).map_err(qerr)?;
        let col: String = row.try_get(2).map_err(qerr)?;
        let ref_schema: String = row.try_get(3).map_err(qerr)?;
        let ref_table: String = row.try_get(4).map_err(qerr)?;
        let ref_col: String = row.try_get(5).map_err(qerr)?;
        let entry = grouped
            .entry((tbl, name))
            .or_insert_with(|| (ref_schema, ref_table, Vec::new(), Vec::new()));
        entry.2.push(col);
        entry.3.push(ref_col);
    }

    for ((tbl, name), (ref_schema, ref_table, columns, ref_columns)) in grouped {
        if let Some(table) = tables.get_mut(&tbl) {
            table.foreign_keys.push(DbForeignKey {
                name,
                columns,
                ref_schema: Some(ref_schema),
                ref_table,
                ref_columns,
                source: None,
            });
        }
    }
    Ok(())
}

async fn attach_checks(
    conn: &mut MySqlConnection,
    schema: &str,
    tables: &mut BTreeMap<String, DbTable>,
    limitations: &mut Vec<String>,
) {
    let query = sqlx::query(
        "SELECT tc.TABLE_NAME, cc.CONSTRAINT_NAME, cc.CHECK_CLAUSE \
         FROM information_schema.CHECK_CONSTRAINTS cc \
         JOIN information_schema.TABLE_CONSTRAINTS tc \
           ON tc.CONSTRAINT_SCHEMA = cc.CONSTRAINT_SCHEMA \
          AND tc.CONSTRAINT_NAME = cc.CONSTRAINT_NAME \
         WHERE cc.CONSTRAINT_SCHEMA = ? \
         ORDER BY tc.TABLE_NAME, cc.CONSTRAINT_NAME",
    )
    .bind(schema)
    .fetch_all(&mut *conn)
    .await;

    match query {
        Ok(rows) => {
            for row in rows {
                if let (Ok(tbl), Ok(name), Ok(clause)) = (
                    row.try_get::<String, _>(0),
                    row.try_get::<String, _>(1),
                    row.try_get::<String, _>(2),
                ) && let Some(table) = tables.get_mut(&tbl)
                {
                    table.checks.push(DbCheck {
                        name: Some(name),
                        expression: clause,
                    });
                }
            }
        }
        Err(_) => limitations.push(format!(
            "MySQL schema `{schema}`: information_schema.CHECK_CONSTRAINTS is unavailable on this server version"
        )),
    }
}

async fn read_objects(
    conn: &mut MySqlConnection,
    schema: &str,
    safe_dsn: &str,
) -> Result<(Vec<DbView>, Vec<DbTrigger>, Vec<DbFunction>), DbSchemaError> {
    let qerr = |e: sqlx::Error| DbSchemaError::Query {
        dsn: safe_dsn.to_owned(),
        detail: e.to_string(),
    };
    let views = sqlx::query_scalar::<_, String>(
        "SELECT TABLE_NAME FROM information_schema.VIEWS \
         WHERE TABLE_SCHEMA = ? ORDER BY TABLE_NAME",
    )
    .bind(schema)
    .fetch_all(&mut *conn)
    .await
    .map_err(qerr)?
    .into_iter()
    .map(|name| DbView {
        name,
        extension_owned: false,
    })
    .collect();

    let triggers = sqlx::query(
        "SELECT EVENT_OBJECT_TABLE, TRIGGER_NAME FROM information_schema.TRIGGERS \
         WHERE TRIGGER_SCHEMA = ? ORDER BY EVENT_OBJECT_TABLE, TRIGGER_NAME",
    )
    .bind(schema)
    .fetch_all(&mut *conn)
    .await
    .map_err(qerr)?
    .into_iter()
    .map(|row| -> Result<DbTrigger, DbSchemaError> {
        Ok(DbTrigger {
            table: row.try_get(0).map_err(qerr)?,
            name: row.try_get(1).map_err(qerr)?,
            extension_owned: false,
        })
    })
    .collect::<Result<Vec<_>, _>>()?;

    let functions = sqlx::query_scalar::<_, String>(
        "SELECT ROUTINE_NAME FROM information_schema.ROUTINES \
         WHERE ROUTINE_SCHEMA = ? ORDER BY ROUTINE_NAME",
    )
    .bind(schema)
    .fetch_all(&mut *conn)
    .await
    .map_err(qerr)?
    .into_iter()
    .map(|name| DbFunction {
        name,
        extension_owned: false,
    })
    .collect();

    Ok((views, triggers, functions))
}
