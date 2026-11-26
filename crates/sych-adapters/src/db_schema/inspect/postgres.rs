//! Read-only PostgreSQL catalog inspection via `pg_catalog`.

use std::collections::BTreeMap;
use std::str::FromStr;

use sqlx::postgres::PgConnectOptions;
use sqlx::{ConnectOptions, Connection, PgConnection, Row};
use sych_core::SchemaVersion;
use sych_core::db_schema::{
    DbCheck, DbColumn, DbForeignKey, DbFunction, DbIndex, DbIndexPart, DbSchema, DbSchemaSnapshot,
    DbSchemaSource, DbTable, DbTrigger, DbView, SqlDialect,
};
use tokio::time::timeout;

use super::InspectOptions;
use crate::db_schema::{DbSchemaError, classify_sql_type, redact_dsn};

const SCHEMAS_SQL: &str = r"
SELECT nspname
FROM pg_catalog.pg_namespace
WHERE nspname NOT IN ('pg_catalog', 'information_schema', 'pg_toast')
  AND nspname NOT LIKE 'pg_temp_%'
  AND nspname NOT LIKE 'pg_toast_temp_%'
ORDER BY nspname
";

const ENUMS_SQL: &str = r"
SELECT t.oid::bigint, n.nspname || '.' || t.typname, e.enumlabel
FROM pg_catalog.pg_type t
JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace
JOIN pg_catalog.pg_enum e ON e.enumtypid = t.oid
WHERE t.typtype = 'e'
ORDER BY t.oid, e.enumsortorder
";

const TABLES_SQL: &str = r"
SELECT c.oid::bigint, n.nspname, c.relname
FROM pg_catalog.pg_class c
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
WHERE c.relkind IN ('r', 'p')
  AND n.nspname = ANY($1)
ORDER BY n.nspname, c.relname
";

const COLUMNS_SQL: &str = r"
SELECT c.oid::bigint, a.attname, pg_catalog.format_type(a.atttypid, a.atttypmod),
       NOT a.attnotnull, a.atttypid::bigint
FROM pg_catalog.pg_attribute a
JOIN pg_catalog.pg_class c ON c.oid = a.attrelid
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
WHERE c.relkind IN ('r', 'p')
  AND n.nspname = ANY($1)
  AND a.attnum > 0
  AND NOT a.attisdropped
ORDER BY c.oid, a.attnum
";

const INDEXES_SQL: &str = r"
SELECT
    i.indrelid::bigint,
    ic.relname,
    i.indisprimary,
    i.indisunique,
    am.amname,
    pg_catalog.pg_get_expr(i.indpred, i.indrelid),
    i.indnkeyatts::int,
    i.indnatts::int,
    ARRAY(
        SELECT CASE WHEN k = 0 THEN '' ELSE COALESCE(a.attname, '') END
        FROM unnest(i.indkey::int2[]) WITH ORDINALITY AS u(k, ord)
        LEFT JOIN pg_catalog.pg_attribute a
            ON a.attrelid = i.indrelid AND a.attnum = u.k
        ORDER BY u.ord
    ),
    ARRAY(
        SELECT pg_catalog.pg_get_indexdef(i.indexrelid, g, true)
        FROM generate_series(1, i.indnatts) AS g
    ),
    ARRAY(
        SELECT (o & 1) <> 0
        FROM unnest(i.indoption::int2[]) WITH ORDINALITY AS u(o, ord)
        ORDER BY u.ord
    ),
    ARRAY(
        SELECT CASE WHEN oc.opcdefault THEN '' ELSE oc.opcname END
        FROM unnest(i.indclass::oid[]) WITH ORDINALITY AS u(cid, ord)
        LEFT JOIN pg_catalog.pg_opclass oc ON oc.oid = u.cid
        ORDER BY u.ord
    )
FROM pg_catalog.pg_index i
JOIN pg_catalog.pg_class tc ON tc.oid = i.indrelid
JOIN pg_catalog.pg_namespace n ON n.oid = tc.relnamespace
JOIN pg_catalog.pg_class ic ON ic.oid = i.indexrelid
JOIN pg_catalog.pg_am am ON am.oid = ic.relam
WHERE tc.relkind IN ('r', 'p')
  AND n.nspname = ANY($1)
  AND i.indisvalid
  AND i.indisready
ORDER BY i.indrelid, ic.relname
";

const FKS_SQL: &str = r"
SELECT
    con.conrelid::bigint,
    con.conname,
    rn.nspname,
    rc.relname,
    ARRAY(
        SELECT a.attname
        FROM unnest(con.conkey) WITH ORDINALITY AS u(attnum, ord)
        JOIN pg_catalog.pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = u.attnum
        ORDER BY u.ord
    ),
    ARRAY(
        SELECT a.attname
        FROM unnest(con.confkey) WITH ORDINALITY AS u(attnum, ord)
        JOIN pg_catalog.pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = u.attnum
        ORDER BY u.ord
    )
FROM pg_catalog.pg_constraint con
JOIN pg_catalog.pg_class c ON c.oid = con.conrelid
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
JOIN pg_catalog.pg_class rc ON rc.oid = con.confrelid
JOIN pg_catalog.pg_namespace rn ON rn.oid = rc.relnamespace
WHERE con.contype = 'f'
  AND n.nspname = ANY($1)
ORDER BY con.conrelid, con.conname
";

const CHECKS_SQL: &str = r"
SELECT con.conrelid::bigint, con.conname, pg_catalog.pg_get_constraintdef(con.oid)
FROM pg_catalog.pg_constraint con
JOIN pg_catalog.pg_class c ON c.oid = con.conrelid
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
WHERE con.contype = 'c'
  AND n.nspname = ANY($1)
ORDER BY con.conrelid, con.conname
";

const VIEWS_SQL: &str = r"
SELECT n.nspname, c.relname,
       EXISTS (
           SELECT 1 FROM pg_catalog.pg_depend d
           WHERE d.objid = c.oid AND d.deptype = 'e'
       )
FROM pg_catalog.pg_class c
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
WHERE c.relkind IN ('v', 'm')
  AND n.nspname = ANY($1)
ORDER BY n.nspname, c.relname
";

const TRIGGERS_SQL: &str = r"
SELECT n.nspname, c.relname, t.tgname,
       EXISTS (
           SELECT 1 FROM pg_catalog.pg_depend d
           WHERE d.objid = t.oid AND d.deptype = 'e'
       )
FROM pg_catalog.pg_trigger t
JOIN pg_catalog.pg_class c ON c.oid = t.tgrelid
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
WHERE NOT t.tgisinternal
  AND n.nspname = ANY($1)
ORDER BY n.nspname, c.relname, t.tgname
";

const FUNCTIONS_SQL: &str = r"
SELECT n.nspname, p.proname,
       EXISTS (
           SELECT 1 FROM pg_catalog.pg_depend d
           WHERE d.objid = p.oid AND d.deptype = 'e'
       )
FROM pg_catalog.pg_proc p
JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace
WHERE n.nspname = ANY($1)
ORDER BY n.nspname, p.proname
";

/// Connects to PostgreSQL with `default_transaction_read_only=on` and `statement_timeout`,
/// returning a normalized [`DbSchemaSnapshot`].
pub async fn inspect(
    dsn: &str,
    options: &InspectOptions,
) -> Result<DbSchemaSnapshot, DbSchemaError> {
    let safe_dsn = redact_dsn(dsn);
    let ms = options
        .timeout
        .as_millis()
        .min(u128::from(u32::MAX))
        .to_string();
    let connect_opts = PgConnectOptions::from_str(dsn.trim())
        .map_err(|e| DbSchemaError::InvalidDsn {
            dsn: safe_dsn.clone(),
            detail: e.to_string(),
        })?
        .options([
            ("default_transaction_read_only", "on"),
            ("statement_timeout", ms.as_str()),
        ]);

    let secs = options.timeout.as_secs();
    timeout(options.timeout, async {
        let mut conn = connect_opts
            .connect()
            .await
            .map_err(|e| DbSchemaError::Connect {
                dsn: safe_dsn.clone(),
                detail: e.to_string(),
            })?;
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
    conn: &mut PgConnection,
    options: &InspectOptions,
    safe_dsn: &str,
) -> Result<DbSchemaSnapshot, DbSchemaError> {
    let qerr = |e: sqlx::Error| DbSchemaError::Query {
        dsn: safe_dsn.to_owned(),
        detail: e.to_string(),
    };
    let schema_names: Vec<String> = if options.schemas.is_empty() {
        sqlx::query_scalar(SCHEMAS_SQL)
            .fetch_all(&mut *conn)
            .await
            .map_err(qerr)?
    } else {
        options.schemas.clone()
    };

    let mut schemas: BTreeMap<String, DbSchema> = schema_names
        .iter()
        .map(|name| {
            (
                name.clone(),
                DbSchema {
                    name: name.clone(),
                    tables: Vec::new(),
                    views: Vec::new(),
                    triggers: Vec::new(),
                    functions: Vec::new(),
                },
            )
        })
        .collect();
    if schema_names.is_empty() {
        return Ok(DbSchemaSnapshot {
            schema_version: SchemaVersion,
            dialect: SqlDialect::Postgres,
            source: DbSchemaSource::LiveDatabase,
            schemas: Vec::new(),
            limitations: Vec::new(),
        });
    }

    let enums = read_enums(conn, safe_dsn).await?;
    let mut tables_by_oid: BTreeMap<i64, (String, DbTable)> = BTreeMap::new();
    for row in sqlx::query(TABLES_SQL)
        .bind(&schema_names)
        .fetch_all(&mut *conn)
        .await
        .map_err(qerr)?
    {
        let oid: i64 = row.try_get(0).map_err(qerr)?;
        let schema: String = row.try_get(1).map_err(qerr)?;
        let name: String = row.try_get(2).map_err(qerr)?;
        tables_by_oid.insert(
            oid,
            (
                schema,
                DbTable {
                    name,
                    columns: Vec::new(),
                    primary_key: None,
                    indexes: Vec::new(),
                    foreign_keys: Vec::new(),
                    checks: Vec::new(),
                    source: None,
                },
            ),
        );
    }

    attach_columns(conn, &schema_names, &enums, &mut tables_by_oid, safe_dsn).await?;
    attach_indexes(conn, &schema_names, &mut tables_by_oid, safe_dsn).await?;
    attach_fks(conn, &schema_names, &mut tables_by_oid, safe_dsn).await?;
    attach_checks(conn, &schema_names, &mut tables_by_oid, safe_dsn).await?;

    for (_, (schema_name, mut table)) in tables_by_oid {
        table.indexes.sort_by(|a, b| a.name.cmp(&b.name));
        table.foreign_keys.sort_by(|a, b| a.name.cmp(&b.name));
        if let Some(s) = schemas.get_mut(&schema_name) {
            s.tables.push(table);
        }
    }
    for s in schemas.values_mut() {
        s.tables.sort_by(|a, b| a.name.cmp(&b.name));
    }

    attach_objects(conn, &schema_names, &mut schemas, safe_dsn).await?;

    Ok(DbSchemaSnapshot {
        schema_version: SchemaVersion,
        dialect: SqlDialect::Postgres,
        source: DbSchemaSource::LiveDatabase,
        schemas: schemas.into_values().collect(),
        limitations: Vec::new(),
    })
}

async fn read_enums(
    conn: &mut PgConnection,
    safe_dsn: &str,
) -> Result<BTreeMap<i64, (String, Vec<String>)>, DbSchemaError> {
    let qerr = |e: sqlx::Error| DbSchemaError::Query {
        dsn: safe_dsn.to_owned(),
        detail: e.to_string(),
    };
    let mut enums: BTreeMap<i64, (String, Vec<String>)> = BTreeMap::new();
    for row in sqlx::query(ENUMS_SQL)
        .fetch_all(&mut *conn)
        .await
        .map_err(qerr)?
    {
        let oid: i64 = row.try_get(0).map_err(qerr)?;
        let type_name: String = row.try_get(1).map_err(qerr)?;
        let label: String = row.try_get(2).map_err(qerr)?;
        enums
            .entry(oid)
            .or_insert_with(|| (type_name, Vec::new()))
            .1
            .push(label);
    }
    Ok(enums)
}

async fn attach_columns(
    conn: &mut PgConnection,
    schema_names: &[String],
    enums: &BTreeMap<i64, (String, Vec<String>)>,
    tables: &mut BTreeMap<i64, (String, DbTable)>,
    safe_dsn: &str,
) -> Result<(), DbSchemaError> {
    let qerr = |e: sqlx::Error| DbSchemaError::Query {
        dsn: safe_dsn.to_owned(),
        detail: e.to_string(),
    };
    for row in sqlx::query(COLUMNS_SQL)
        .bind(schema_names)
        .fetch_all(&mut *conn)
        .await
        .map_err(qerr)?
    {
        let oid: i64 = row.try_get(0).map_err(qerr)?;
        let name: String = row.try_get(1).map_err(qerr)?;
        let raw_type: String = row.try_get(2).map_err(qerr)?;
        let nullable: bool = row.try_get(3).map_err(qerr)?;
        let typid: i64 = row.try_get(4).map_err(qerr)?;
        let enum_info = enums.get(&typid).map(|(n, v)| (Some(n.clone()), v.clone()));
        if let Some((_, table)) = tables.get_mut(&oid) {
            table.columns.push(DbColumn {
                name,
                data_type: classify_sql_type(SqlDialect::Postgres, &raw_type, enum_info),
                nullable,
                source: None,
            });
        }
    }
    Ok(())
}

async fn attach_indexes(
    conn: &mut PgConnection,
    schema_names: &[String],
    tables: &mut BTreeMap<i64, (String, DbTable)>,
    safe_dsn: &str,
) -> Result<(), DbSchemaError> {
    let qerr = |e: sqlx::Error| DbSchemaError::Query {
        dsn: safe_dsn.to_owned(),
        detail: e.to_string(),
    };
    for row in sqlx::query(INDEXES_SQL)
        .bind(schema_names)
        .fetch_all(&mut *conn)
        .await
        .map_err(qerr)?
    {
        let oid: i64 = row.try_get(0).map_err(qerr)?;
        let name: String = row.try_get(1).map_err(qerr)?;
        let is_primary: bool = row.try_get(2).map_err(qerr)?;
        let is_unique: bool = row.try_get(3).map_err(qerr)?;
        let method: String = row.try_get(4).map_err(qerr)?;
        let predicate: Option<String> = row.try_get(5).map_err(qerr)?;
        let nkeyatts: i32 = row.try_get(6).map_err(qerr)?;
        let natts: i32 = row.try_get(7).map_err(qerr)?;
        let col_names: Vec<String> = row.try_get(8).map_err(qerr)?;
        let key_defs: Vec<String> = row.try_get(9).map_err(qerr)?;
        let descs: Vec<bool> = row.try_get(10).map_err(qerr)?;
        let opclasses: Vec<String> = row.try_get(11).map_err(qerr)?;

        let nkeys = usize::try_from(nkeyatts).unwrap_or(col_names.len());
        let total = usize::try_from(natts).unwrap_or(col_names.len());
        let mut parts = Vec::new();
        for (i, cname) in col_names.iter().enumerate().take(nkeys) {
            let desc = descs.get(i).copied().unwrap_or(false);
            let opclass = opclasses.get(i).filter(|s| !s.is_empty()).cloned();
            if cname.is_empty() {
                parts.push(DbIndexPart {
                    column: None,
                    expression: Some(key_defs.get(i).cloned().unwrap_or_default()),
                    desc,
                    opclass,
                    prefix_length: None,
                });
            } else {
                parts.push(DbIndexPart {
                    column: Some(cname.clone()),
                    expression: None,
                    desc,
                    opclass,
                    prefix_length: None,
                });
            }
        }
        let include: Vec<String> = col_names
            .iter()
            .skip(nkeys)
            .take(total.saturating_sub(nkeys))
            .filter(|s| !s.is_empty())
            .cloned()
            .collect();

        let idx = DbIndex {
            name,
            unique: is_unique || is_primary,
            method,
            predicate,
            include,
            parts,
            source: None,
        };
        if let Some((_, table)) = tables.get_mut(&oid) {
            if is_primary {
                table.primary_key = Some(idx);
            } else {
                table.indexes.push(idx);
            }
        }
    }
    Ok(())
}

async fn attach_fks(
    conn: &mut PgConnection,
    schema_names: &[String],
    tables: &mut BTreeMap<i64, (String, DbTable)>,
    safe_dsn: &str,
) -> Result<(), DbSchemaError> {
    let qerr = |e: sqlx::Error| DbSchemaError::Query {
        dsn: safe_dsn.to_owned(),
        detail: e.to_string(),
    };
    for row in sqlx::query(FKS_SQL)
        .bind(schema_names)
        .fetch_all(&mut *conn)
        .await
        .map_err(qerr)?
    {
        let oid: i64 = row.try_get(0).map_err(qerr)?;
        let name: String = row.try_get(1).map_err(qerr)?;
        let ref_schema: String = row.try_get(2).map_err(qerr)?;
        let ref_table: String = row.try_get(3).map_err(qerr)?;
        let columns: Vec<String> = row.try_get(4).map_err(qerr)?;
        let ref_columns: Vec<String> = row.try_get(5).map_err(qerr)?;
        if let Some((_, table)) = tables.get_mut(&oid) {
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
    conn: &mut PgConnection,
    schema_names: &[String],
    tables: &mut BTreeMap<i64, (String, DbTable)>,
    safe_dsn: &str,
) -> Result<(), DbSchemaError> {
    let qerr = |e: sqlx::Error| DbSchemaError::Query {
        dsn: safe_dsn.to_owned(),
        detail: e.to_string(),
    };
    for row in sqlx::query(CHECKS_SQL)
        .bind(schema_names)
        .fetch_all(&mut *conn)
        .await
        .map_err(qerr)?
    {
        let oid: i64 = row.try_get(0).map_err(qerr)?;
        let name: String = row.try_get(1).map_err(qerr)?;
        let def: String = row.try_get(2).map_err(qerr)?;
        let expr = strip_pg_check_wrapper(&def);
        if let Some((_, table)) = tables.get_mut(&oid) {
            table.checks.push(DbCheck {
                name: Some(name),
                expression: expr,
            });
        }
    }
    Ok(())
}

fn strip_pg_check_wrapper(def: &str) -> String {
    let trimmed = def.trim();
    if let Some(rest) = trimmed
        .strip_prefix("CHECK (")
        .or_else(|| trimmed.strip_prefix("CHECK("))
        && let Some(inner) = rest.strip_suffix(')')
    {
        return inner.trim().to_owned();
    }

    trimmed.to_owned()
}

async fn attach_objects(
    conn: &mut PgConnection,
    schema_names: &[String],
    schemas: &mut BTreeMap<String, DbSchema>,
    safe_dsn: &str,
) -> Result<(), DbSchemaError> {
    let qerr = |e: sqlx::Error| DbSchemaError::Query {
        dsn: safe_dsn.to_owned(),
        detail: e.to_string(),
    };
    for row in sqlx::query(VIEWS_SQL)
        .bind(schema_names)
        .fetch_all(&mut *conn)
        .await
        .map_err(qerr)?
    {
        let schema: String = row.try_get(0).map_err(qerr)?;
        let name: String = row.try_get(1).map_err(qerr)?;
        let ext: bool = row.try_get(2).map_err(qerr)?;
        if let Some(s) = schemas.get_mut(&schema) {
            s.views.push(DbView {
                name,
                extension_owned: ext,
            });
        }
    }
    for row in sqlx::query(TRIGGERS_SQL)
        .bind(schema_names)
        .fetch_all(&mut *conn)
        .await
        .map_err(qerr)?
    {
        let schema: String = row.try_get(0).map_err(qerr)?;
        let table: String = row.try_get(1).map_err(qerr)?;
        let name: String = row.try_get(2).map_err(qerr)?;
        let ext: bool = row.try_get(3).map_err(qerr)?;
        if let Some(s) = schemas.get_mut(&schema) {
            s.triggers.push(DbTrigger {
                table,
                name,
                extension_owned: ext,
            });
        }
    }
    for row in sqlx::query(FUNCTIONS_SQL)
        .bind(schema_names)
        .fetch_all(&mut *conn)
        .await
        .map_err(qerr)?
    {
        let schema: String = row.try_get(0).map_err(qerr)?;
        let name: String = row.try_get(1).map_err(qerr)?;
        let ext: bool = row.try_get(2).map_err(qerr)?;
        if let Some(s) = schemas.get_mut(&schema) {
            s.functions.push(DbFunction {
                name,
                extension_owned: ext,
            });
        }
    }
    Ok(())
}
