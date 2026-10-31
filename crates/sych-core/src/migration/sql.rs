//! What a SQL migration changes in the schema: a small scanner for the DDL that
//! affects existing readers and writers. It is a locator for context, not a SQL parser; an
//! unrecognized statement is simply not a schema change here.

use std::iter::Peekable;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::CodeRef;

/// One table or column a migration drops, renames, alters or adds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SchemaChange {
    pub table: String,
    pub column: Option<String>,
    /// `DROP COLUMN`, `RENAME COLUMN`, `ALTER COLUMN`, `ADD COLUMN`, `ADD CHECK`, `DROP TABLE`,
    /// `RENAME TABLE`.
    pub change: String,
}

/// A schema change with the code of either side that names it: the old app's readers and
/// writers live on the base side, the new app's on the head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SchemaRef {
    #[serde(flatten)]
    pub change: SchemaChange,
    pub migration: CodeRef,
    pub references: Vec<CodeRef>,
    /// The search hit its limit: more references may exist.
    pub truncated: bool,
}

fn ident(token: &str) -> String {
    let bare = token.trim_matches(|c: char| matches!(c, '"' | '`' | '[' | ']' | ';'));
    bare.rsplit('.')
        .next()
        .unwrap_or(bare)
        .trim_matches(|c: char| matches!(c, '"' | '`' | '[' | ']'))
        .to_lowercase()
}

/// Splits on commas outside parentheses: `ALTER TABLE t DROP a, ADD b int`.
fn clauses(text: &str) -> Vec<&str> {
    let (mut depth, mut start, mut out) = (0i32, 0, Vec::new());
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&text[start..]);
    out
}

fn skip_if_exists<'a>(words: &mut Peekable<impl Iterator<Item = &'a str>>) {
    for keyword in ["if", "exists"] {
        if words
            .peek()
            .is_some_and(|w| w.eq_ignore_ascii_case(keyword))
        {
            words.next();
        }
    }
}

fn alter_clause(table: &str, clause: &str) -> Option<SchemaChange> {
    let mut words = clause.split_whitespace().peekable();
    let verb = words.next()?.to_lowercase();
    let change = |change: &str, column: Option<String>| SchemaChange {
        table: table.to_owned(),
        column,
        change: change.to_owned(),
    };
    if verb == "rename" && words.peek().is_some_and(|w| w.eq_ignore_ascii_case("to")) {
        return Some(change("RENAME TABLE", None));
    }

    // `ADD [CONSTRAINT name] CHECK (col …)` narrows what a column may hold.
    if verb == "add"
        && let Some(column) = check_column(clause)
    {
        return Some(change("ADD CHECK", Some(column)));
    }

    let kind = match verb.as_str() {
        "drop" => "DROP COLUMN",
        "rename" => "RENAME COLUMN",
        "alter" | "modify" => "ALTER COLUMN",
        "add" => "ADD COLUMN",
        _ => return None,
    };
    if words
        .peek()
        .is_some_and(|w| w.eq_ignore_ascii_case("column"))
    {
        words.next();
    }

    skip_if_exists(&mut words);
    let column = words.next().map(ident)?;
    // Constraints and indexes are not columns.
    let constraint = [
        "constraint",
        "primary",
        "foreign",
        "unique",
        "index",
        "check",
        "key",
    ];
    (!constraint.contains(&column.as_str())).then(|| change(kind, Some(column)))
}

/// The first identifier inside `CHECK (…)`, the column it constrains.
fn check_column(clause: &str) -> Option<String> {
    let lower = clause.to_lowercase();
    let at = lower.find("check")?;
    let inner = &clause[at..];
    let open = inner.find('(')?;
    let column: String = inner[open + 1..]
        .trim_start_matches([' ', '('])
        .chars()
        .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '"' | '`' | '.'))
        .collect();
    let column = ident(&column);
    (!column.is_empty()).then_some(column)
}

/// The schema changes of one migration file.
pub fn schema_changes(sql: &str) -> Vec<SchemaChange> {
    let mut out = Vec::new();
    let without_comments: String = sql
        .lines()
        .map(|l| l.split("--").next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    for statement in without_comments.split(';') {
        let mut words = statement.split_whitespace().peekable();
        let (Some(first), Some(second)) = (words.next(), words.next()) else {
            continue;
        };
        if !second.eq_ignore_ascii_case("table") {
            continue;
        }

        skip_if_exists(&mut words);
        if words.peek().is_some_and(|w| w.eq_ignore_ascii_case("only")) {
            words.next();
        }

        let Some(table) = words.next().map(ident) else {
            continue;
        };
        if first.eq_ignore_ascii_case("drop") {
            out.push(SchemaChange {
                table,
                column: None,
                change: "DROP TABLE".into(),
            });
        } else if first.eq_ignore_ascii_case("alter") {
            let rest: Vec<&str> = words.collect();
            let rest = rest.join(" ");
            out.extend(
                clauses(&rest)
                    .into_iter()
                    .filter_map(|c| alter_clause(&table, c)),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shapes(sql: &str) -> Vec<(String, Option<String>, String)> {
        schema_changes(sql)
            .into_iter()
            .map(|c| (c.table, c.column, c.change))
            .collect()
    }

    #[test]
    fn ddl_that_affects_readers_is_found() {
        let sql = "-- drop the old status\nALTER TABLE IF EXISTS public.\"Orders\" DROP COLUMN IF EXISTS legacy_status, \
                   ADD COLUMN note text NULL, ADD CONSTRAINT orders_pk PRIMARY KEY (id);\n\
                   ALTER TABLE orders RENAME COLUMN state TO phase;\nALTER TABLE orders ALTER COLUMN id TYPE bigint;\n\
                   DROP TABLE IF EXISTS order_events;\nALTER TABLE carts RENAME TO baskets;\n\
                   CREATE INDEX x ON orders (id);\nUPDATE orders SET state = status;\n\
                   ALTER TABLE orders ADD CONSTRAINT orders_status CHECK (status IN ('new', 'paid'));";
        let s = |t: &str, c: Option<&str>, k: &str| (t.into(), c.map(Into::into), k.into());
        assert_eq!(
            shapes(sql),
            [
                s("orders", Some("legacy_status"), "DROP COLUMN"),
                s("orders", Some("note"), "ADD COLUMN"),
                s("orders", Some("state"), "RENAME COLUMN"),
                s("orders", Some("id"), "ALTER COLUMN"),
                s("order_events", None, "DROP TABLE"),
                s("carts", None, "RENAME TABLE"),
                s("orders", Some("status"), "ADD CHECK"),
            ]
        );
    }
}
