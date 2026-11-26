//! Table, column, constraint, index and drop DDL replay handlers.

use sqlparser::ast::{
    AlterColumnOperation, AlterTable, AlterTableOperation, ColumnDef, ColumnOption, CreateIndex,
    CreateTable, ObjectName, ObjectType, RenameTableNameKind, Spanned, TableConstraint,
};
use sych_core::db_schema::{DbCheck, DbColumn, DbForeignKey, DbIndex, DbIndexPart, DbTable};

use super::SchemaState;
use super::split::{col_part, index_col_to_part, pos};

impl SchemaState {
    pub(super) fn apply_create_table(&mut self, file: &str, base_line: u32, ct: CreateTable) {
        if ct.temporary {
            return;
        }

        let (schema_name, table_name) = self.split_name(&ct.name);
        if table_name.is_empty() {
            return;
        }

        let mut table = DbTable {
            name: table_name.clone(),
            columns: Vec::new(),
            primary_key: None,
            indexes: Vec::new(),
            foreign_keys: Vec::new(),
            checks: Vec::new(),
            source: Some(pos(file, base_line, ct.name.span().start.line)),
        };

        for col_def in ct.columns {
            self.add_column_def(file, base_line, &schema_name, &mut table, col_def);
        }
        for constraint in ct.constraints {
            self.add_table_constraint(file, base_line, &schema_name, &mut table, constraint);
        }

        // Primary key columns are implicitly NOT NULL in SQL.
        let pk_cols: Vec<String> = table
            .primary_key_columns()
            .into_iter()
            .map(str::to_owned)
            .collect();
        for col in &mut table.columns {
            if pk_cols.contains(&col.name) {
                col.nullable = false;
            }
        }

        let schema = self.schema_mut(&schema_name);
        if let Some(existing) = schema.table_mut(&table_name) {
            if !ct.if_not_exists {
                *existing = table;
            }
        } else {
            schema.tables.push(table);
        }
    }

    fn add_column_def(
        &self,
        file: &str,
        base_line: u32,
        schema_name: &str,
        table: &mut DbTable,
        col_def: ColumnDef,
    ) {
        let col_name = col_def.name.value;
        let col_pos = pos(file, base_line, col_def.name.span.start.line);
        let raw_type = col_def.data_type.to_string();
        let data_type = self.resolve_column_type(schema_name, &raw_type);

        let lower_type = raw_type.to_ascii_lowercase();
        let is_serial = matches!(lower_type.as_str(), "serial" | "bigserial" | "smallserial");
        let mut nullable = !is_serial;

        for opt in col_def.options {
            match opt.option {
                ColumnOption::NotNull => nullable = false,
                ColumnOption::Null => nullable = true,
                ColumnOption::PrimaryKey(pk) => {
                    nullable = false;
                    let pk_name = opt
                        .name
                        .or(pk.name)
                        .map_or_else(|| format!("{}_pkey", table.name), |i| i.value);
                    table.primary_key = Some(DbIndex {
                        name: pk_name,
                        unique: true,
                        method: String::new(),
                        predicate: None,
                        include: Vec::new(),
                        parts: vec![col_part(col_name.clone())],
                        source: Some(col_pos.clone()),
                    });
                }
                ColumnOption::Unique(uq) => {
                    let uq_name = opt
                        .name
                        .or(uq.name)
                        .map_or_else(|| format!("{}_{col_name}_key", table.name), |i| i.value);
                    table.indexes.push(DbIndex {
                        name: uq_name,
                        unique: true,
                        method: String::new(),
                        predicate: None,
                        include: Vec::new(),
                        parts: vec![col_part(col_name.clone())],
                        source: Some(col_pos.clone()),
                    });
                }
                ColumnOption::ForeignKey(fk) => {
                    let (ref_schema, ref_table) = self.split_name(&fk.foreign_table);
                    let fk_name = opt
                        .name
                        .or(fk.name)
                        .map_or_else(|| format!("{}_{col_name}_fkey", table.name), |i| i.value);
                    let ref_columns: Vec<String> =
                        fk.referred_columns.into_iter().map(|i| i.value).collect();
                    table.foreign_keys.push(DbForeignKey {
                        name: fk_name,
                        columns: vec![col_name.clone()],
                        ref_schema: Some(ref_schema),
                        ref_table,
                        ref_columns,
                        source: Some(col_pos.clone()),
                    });
                }
                ColumnOption::Check(chk) => {
                    table.checks.push(DbCheck {
                        name: opt.name.or(chk.name).map(|i| i.value),
                        expression: chk.expr.to_string(),
                    });
                }
                _ => {}
            }
        }

        table.columns.push(DbColumn {
            name: col_name,
            data_type,
            nullable,
            source: Some(col_pos),
        });
    }

    fn add_table_constraint(
        &self,
        file: &str,
        base_line: u32,
        _schema_name: &str,
        table: &mut DbTable,
        constraint: TableConstraint,
    ) {
        match constraint {
            TableConstraint::PrimaryKey(pk) => {
                let cpos = pos(file, base_line, pk.span().start.line);
                let name = pk
                    .name
                    .or(pk.index_name)
                    .map_or_else(|| format!("{}_pkey", table.name), |i| i.value);
                let parts: Vec<DbIndexPart> = pk.columns.iter().map(index_col_to_part).collect();
                for part in &parts {
                    if let Some(cname) = &part.column
                        && let Some(c) = table.columns.iter_mut().find(|c| &c.name == cname)
                    {
                        c.nullable = false;
                    }
                }
                table.primary_key = Some(DbIndex {
                    name,
                    unique: true,
                    method: pk
                        .index_type
                        .map_or_else(String::new, |t| t.to_string().to_ascii_lowercase()),
                    predicate: None,
                    include: pk.include.into_iter().map(|i| i.value).collect(),
                    parts,
                    source: Some(cpos),
                });
            }
            TableConstraint::Unique(uq) => {
                let cpos = pos(file, base_line, uq.span().start.line);
                let parts: Vec<DbIndexPart> = uq.columns.iter().map(index_col_to_part).collect();
                let name = uq.name.or(uq.index_name).map_or_else(
                    || {
                        let cols: Vec<&str> = parts.iter().map(DbIndexPart::display_name).collect();
                        format!("{}_{}_key", table.name, cols.join("_"))
                    },
                    |i| i.value,
                );
                table.indexes.push(DbIndex {
                    name,
                    unique: true,
                    method: uq
                        .index_type
                        .map_or_else(String::new, |t| t.to_string().to_ascii_lowercase()),
                    predicate: None,
                    include: uq.include.into_iter().map(|i| i.value).collect(),
                    parts,
                    source: Some(cpos),
                });
            }
            TableConstraint::ForeignKey(fk) => {
                let cpos = pos(file, base_line, fk.span().start.line);
                let (ref_schema, ref_table) = self.split_name(&fk.foreign_table);
                let columns: Vec<String> = fk.columns.into_iter().map(|i| i.value).collect();
                let ref_columns: Vec<String> =
                    fk.referred_columns.into_iter().map(|i| i.value).collect();
                let name = fk.name.or(fk.index_name).map_or_else(
                    || format!("{}_{}_fkey", table.name, columns.join("_")),
                    |i| i.value,
                );
                table.foreign_keys.push(DbForeignKey {
                    name,
                    columns,
                    ref_schema: Some(ref_schema),
                    ref_table,
                    ref_columns,
                    source: Some(cpos),
                });
            }
            TableConstraint::Check(chk) => {
                table.checks.push(DbCheck {
                    name: chk.name.map(|i| i.value),
                    expression: chk.expr.to_string(),
                });
            }
            TableConstraint::Index(idx) => {
                let cpos = pos(file, base_line, idx.span().start.line);
                let parts: Vec<DbIndexPart> = idx.columns.iter().map(index_col_to_part).collect();
                let name = idx.name.map_or_else(
                    || {
                        let cols: Vec<&str> = parts.iter().map(DbIndexPart::display_name).collect();
                        format!("{}_{}_idx", table.name, cols.join("_"))
                    },
                    |i| i.value,
                );
                table.indexes.push(DbIndex {
                    name,
                    unique: false,
                    method: idx
                        .index_type
                        .map_or_else(String::new, |t| t.to_string().to_ascii_lowercase()),
                    predicate: None,
                    include: Vec::new(),
                    parts,
                    source: Some(cpos),
                });
            }
            _ => {}
        }
    }

    pub(super) fn apply_alter_table(&mut self, file: &str, base_line: u32, at: AlterTable) {
        let (schema_name, table_name) = self.split_name(&at.name);
        let mut rename_to: Option<String> = None;

        for op in at.operations {
            self.apply_alter_op(
                file,
                base_line,
                &schema_name,
                &table_name,
                op,
                &mut rename_to,
            );
        }

        if let Some(new_name) = rename_to
            && let Some(t) = self
                .schemas
                .get_mut(&schema_name)
                .and_then(|s| s.table_mut(&table_name))
        {
            t.name = new_name;
        }
    }

    fn apply_alter_op(
        &mut self,
        file: &str,
        base_line: u32,
        schema_name: &str,
        table_name: &str,
        op: AlterTableOperation,
        rename_to: &mut Option<String>,
    ) {
        match op {
            AlterTableOperation::AddColumn { column_def, .. } => {
                let Some(mut tmp_table) = self
                    .schemas
                    .get(schema_name)
                    .and_then(|s| s.table(table_name))
                    .cloned()
                else {
                    return;
                };
                self.add_column_def(file, base_line, schema_name, &mut tmp_table, column_def);
                if let Some(target) = self
                    .schemas
                    .get_mut(schema_name)
                    .and_then(|s| s.table_mut(table_name))
                {
                    *target = tmp_table;
                }
            }
            AlterTableOperation::AddConstraint { constraint, .. } => {
                let Some(mut tmp_table) = self
                    .schemas
                    .get(schema_name)
                    .and_then(|s| s.table(table_name))
                    .cloned()
                else {
                    return;
                };
                self.add_table_constraint(file, base_line, schema_name, &mut tmp_table, constraint);
                if let Some(target) = self
                    .schemas
                    .get_mut(schema_name)
                    .and_then(|s| s.table_mut(table_name))
                {
                    *target = tmp_table;
                }
            }
            AlterTableOperation::RenameTable { table_name: kind } => {
                let obj = match kind {
                    RenameTableNameKind::As(n) | RenameTableNameKind::To(n) => n,
                };
                *rename_to = Some(self.split_name(&obj).1);
            }
            other => self.apply_alter_mutation(schema_name, table_name, other),
        }
    }

    fn apply_alter_mutation(
        &mut self,
        schema_name: &str,
        table_name: &str,
        op: AlterTableOperation,
    ) {
        let resolved_type = match &op {
            AlterTableOperation::AlterColumn {
                op: AlterColumnOperation::SetDataType { data_type, .. },
                ..
            }
            | AlterTableOperation::ModifyColumn { data_type, .. }
            | AlterTableOperation::ChangeColumn { data_type, .. } => {
                Some(self.resolve_column_type(schema_name, &data_type.to_string()))
            }
            _ => None,
        };
        let Some(t) = self
            .schemas
            .get_mut(schema_name)
            .and_then(|s| s.table_mut(table_name))
        else {
            return;
        };

        match op {
            AlterTableOperation::DropColumn { column_names, .. } => {
                let drop_names: Vec<String> = column_names.into_iter().map(|i| i.value).collect();
                t.columns.retain(|c| !drop_names.contains(&c.name));
            }
            AlterTableOperation::RenameColumn {
                old_column_name,
                new_column_name,
            } => {
                if let Some(c) = t
                    .columns
                    .iter_mut()
                    .find(|c| c.name == old_column_name.value)
                {
                    c.name = new_column_name.value;
                }
            }
            AlterTableOperation::AlterColumn { column_name, op } => {
                if let Some(c) = t.columns.iter_mut().find(|c| c.name == column_name.value) {
                    match op {
                        AlterColumnOperation::SetNotNull => c.nullable = false,
                        AlterColumnOperation::DropNotNull => c.nullable = true,
                        AlterColumnOperation::SetDataType { .. } => {
                            if let Some(dt) = resolved_type {
                                c.data_type = dt;
                            }
                        }
                        _ => {}
                    }
                }
            }
            AlterTableOperation::ModifyColumn {
                col_name, options, ..
            } => {
                let not_null = options
                    .iter()
                    .any(|o| matches!(o, ColumnOption::NotNull | ColumnOption::PrimaryKey(_)));
                if let Some(c) = t.columns.iter_mut().find(|c| c.name == col_name.value) {
                    if let Some(dt) = resolved_type {
                        c.data_type = dt;
                    }

                    c.nullable = !not_null;
                }
            }
            AlterTableOperation::ChangeColumn {
                old_name,
                new_name,
                options,
                ..
            } => {
                let not_null = options
                    .iter()
                    .any(|o| matches!(o, ColumnOption::NotNull | ColumnOption::PrimaryKey(_)));
                if let Some(c) = t.columns.iter_mut().find(|c| c.name == old_name.value) {
                    c.name = new_name.value;
                    if let Some(dt) = resolved_type {
                        c.data_type = dt;
                    }

                    c.nullable = !not_null;
                }
            }
            AlterTableOperation::DropConstraint { name, .. } => {
                if t.primary_key
                    .as_ref()
                    .is_some_and(|pk| pk.name == name.value)
                {
                    t.primary_key = None;
                }

                t.indexes.retain(|i| i.name != name.value);
                t.foreign_keys.retain(|fk| fk.name != name.value);
                t.checks
                    .retain(|c| c.name.as_deref() != Some(name.value.as_str()));
            }
            AlterTableOperation::DropPrimaryKey { .. } => t.primary_key = None,
            AlterTableOperation::DropForeignKey { name, .. } => {
                t.foreign_keys.retain(|fk| fk.name != name.value);
            }
            AlterTableOperation::DropIndex { name } => {
                t.indexes.retain(|i| i.name != name.value);
            }
            _ => {}
        }
    }

    pub(super) fn apply_create_index(&mut self, file: &str, base_line: u32, ci: CreateIndex) {
        let (schema_name, table_name) = self.split_name(&ci.table_name);
        let parts: Vec<DbIndexPart> = ci.columns.iter().map(index_col_to_part).collect();
        let idx_name = ci.name.as_ref().map_or_else(
            || {
                let cols: Vec<&str> = parts.iter().map(DbIndexPart::display_name).collect();
                format!("{table_name}_{}_idx", cols.join("_"))
            },
            |n| self.split_name(n).1,
        );
        let idx = DbIndex {
            name: idx_name.clone(),
            unique: ci.unique,
            method: ci
                .using
                .map_or_else(String::new, |u| u.to_string().to_ascii_lowercase()),
            predicate: ci.predicate.map(|p| p.to_string()),
            include: ci.include.into_iter().map(|i| i.value).collect(),
            parts,
            source: Some(pos(file, base_line, ci.table_name.span().start.line)),
        };

        if let Some(t) = self
            .schemas
            .get_mut(&schema_name)
            .and_then(|s| s.table_mut(&table_name))
            && !t.indexes.iter().any(|i| i.name == idx_name)
        {
            t.indexes.push(idx);
        }
    }

    pub(super) fn apply_drop(&mut self, object_type: ObjectType, names: Vec<ObjectName>) {
        for obj in names {
            let (schema_name, name) = self.split_name(&obj);
            match object_type {
                ObjectType::Table => {
                    if let Some(s) = self.schemas.get_mut(&schema_name) {
                        s.tables.retain(|t| t.name != name);
                    }
                }
                ObjectType::View | ObjectType::MaterializedView => {
                    if let Some(s) = self.schemas.get_mut(&schema_name) {
                        s.views.retain(|v| v.name != name);
                    }
                }
                ObjectType::Index => {
                    for s in self.schemas.values_mut() {
                        for t in &mut s.tables {
                            t.indexes.retain(|i| i.name != name);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
