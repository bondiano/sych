use serde_json::Value;
use sqlx::{AnyPool, Row};

pub enum Column {
    Integer(&'static str),
    Text(&'static str),
}

impl Column {
    fn name(&self) -> &'static str {
        match self {
            Self::Integer(name) | Self::Text(name) => name,
        }
    }
}

pub struct Table {
    pub name: &'static str,
    pub columns: &'static [Column],
    pub primary_key: &'static [&'static str],
}

use Column::{Integer as I, Text as T};

// Kept independent from the importer: changing its projection cannot weaken these assertions.
pub const TABLES: &[Table] = &[
    Table {
        name: "repositories",
        primary_key: &["id"],
        columns: &[
            I("id"),
            T("tenant_id"),
            T("connection_id"),
            T("provider_instance"),
            T("external_id"),
            T("full_name"),
            I("installation_id"),
            T("capabilities"),
            I("capabilities_at"),
        ],
    },
    Table {
        name: "changes",
        primary_key: &["id"],
        columns: &[
            I("id"),
            T("tenant_id"),
            T("access_scope"),
            T("change_key"),
            I("desired_generation"),
            I("repository_id"),
            I("external_number"),
            T("scm_state"),
            T("head_sha"),
            T("base_sha"),
            T("base_ref"),
            I("fork"),
            I("synced_at"),
        ],
    },
    Table {
        name: "runs",
        primary_key: &["id"],
        columns: &[
            T("id"),
            T("tenant_id"),
            T("access_scope"),
            I("change_id"),
            I("generation"),
            T("parent_run_id"),
            T("inputs"),
            T("inputs_digest"),
            T("source_snapshot_id"),
            T("policy_snapshot_id"),
            T("state"),
            T("stage"),
            I("evidence_deadline"),
            T("expected_sources"),
            T("result_id"),
            T("result_sha256"),
            I("model_attempts"),
            I("input_tokens"),
            I("output_tokens"),
            I("cost_known_micro_usd"),
            I("unknown_cost_attempts"),
            T("failure"),
            I("created_at"),
            I("finished_at"),
            I("lineage_applied"),
            T("coverage"),
            I("unknown_obligations"),
        ],
    },
    Table {
        name: "artifacts",
        primary_key: &["tenant_id", "access_scope", "sha256"],
        columns: &[
            T("tenant_id"),
            T("access_scope"),
            T("sha256"),
            T("kind"),
            I("schema_version"),
            T("store_key"),
            I("size"),
            I("created_at"),
            I("retain_until"),
        ],
    },
    Table {
        name: "subscription_fees",
        primary_key: &["tenant_id", "provider", "month"],
        columns: &[
            T("tenant_id"),
            T("provider"),
            T("month"),
            I("amount_micro_usd"),
            I("allocated_micro_usd"),
            I("recorded_at"),
        ],
    },
    Table {
        name: "inbox_events",
        primary_key: &["id"],
        columns: &[
            I("id"),
            T("provider_instance"),
            T("connection_id"),
            T("delivery_id"),
            T("event"),
            T("action"),
            T("payload_sha256"),
            T("normalized"),
            I("received_at"),
            T("decision"),
        ],
    },
    Table {
        name: "audit_events",
        primary_key: &["id"],
        columns: &[
            I("id"),
            T("tenant_id"),
            T("actor"),
            T("action"),
            T("target"),
            T("correlation_id"),
            T("metadata"),
            I("at"),
            I("repository_id"),
        ],
    },
    Table {
        name: "sessions",
        primary_key: &["token_sha256"],
        columns: &[
            T("token_sha256"),
            T("tenant_id"),
            T("actor"),
            T("kind"),
            T("access"),
            I("created_at"),
            I("expires_at"),
        ],
    },
    Table {
        name: "idempotency_keys",
        primary_key: &["id"],
        columns: &[
            I("id"),
            T("tenant_id"),
            T("actor"),
            T("endpoint"),
            T("key"),
            T("body_sha256"),
            I("response_status"),
            T("response_body"),
            I("created_at"),
            I("expires_at"),
        ],
    },
    Table {
        name: "tour_sessions",
        primary_key: &["tenant_id", "tour_id", "actor"],
        columns: &[
            T("tenant_id"),
            T("tour_id"),
            T("actor"),
            T("run_id"),
            T("state"),
            T("revealed"),
            I("updated_at"),
        ],
    },
    Table {
        name: "tour_exchanges",
        primary_key: &["id"],
        columns: &[
            T("id"),
            T("tenant_id"),
            T("tour_id"),
            T("actor"),
            T("run_id"),
            T("body"),
            I("created_at"),
        ],
    },
    Table {
        name: "run_artifacts",
        primary_key: &["run_id", "role"],
        columns: &[
            T("run_id"),
            T("role"),
            T("tenant_id"),
            T("access_scope"),
            T("sha256"),
        ],
    },
    Table {
        name: "model_attempts",
        primary_key: &["id"],
        columns: &[
            T("id"),
            T("tenant_id"),
            T("run_id"),
            T("purpose"),
            T("kind"),
            T("template"),
            T("profile"),
            T("provider"),
            T("requested_model"),
            T("resolved_model"),
            T("generation_id"),
            T("billing_kind"),
            T("enforcement"),
            T("day"),
            T("month"),
            I("reserved_micro_usd"),
            I("input_tokens_bound"),
            I("output_tokens_bound"),
            T("state"),
            I("charge_micro_usd"),
            I("reference_micro_usd"),
            I("input_tokens"),
            I("output_tokens"),
            I("reasoning_tokens"),
            I("billing_mismatch"),
            I("created_at"),
            I("settled_at"),
            T("scope"),
        ],
    },
    Table {
        name: "jobs",
        primary_key: &["id"],
        columns: &[
            I("id"),
            T("run_id"),
            T("kind"),
            T("task_key"),
            T("state"),
            T("owner"),
            I("lease_epoch"),
            I("lease_until"),
            I("deadline"),
            I("attempts"),
            I("max_attempts"),
            I("next_attempt_at"),
            T("last_error"),
            I("created_at"),
            I("finished_at"),
        ],
    },
    Table {
        name: "change_events",
        primary_key: &["id"],
        columns: &[
            I("id"),
            I("change_id"),
            T("run_id"),
            T("kind"),
            T("detail"),
            I("at"),
        ],
    },
    Table {
        name: "change_intents",
        primary_key: &["id"],
        columns: &[
            I("id"),
            I("change_id"),
            T("kind"),
            I("due_at"),
            I("latest_at"),
            I("events"),
            I("first_inbox_id"),
            I("last_inbox_id"),
            I("created_at"),
            I("consumed_at"),
        ],
    },
    Table {
        name: "publication_objects",
        primary_key: &["id"],
        columns: &[
            I("id"),
            I("change_id"),
            T("slot"),
            T("kind"),
            T("marker"),
            T("remote_id"),
            I("desired_generation"),
            T("desired_result_id"),
            I("observed_generation"),
            T("observed_body_sha256"),
            T("relevance"),
            I("updated_at"),
            T("fingerprint"),
            T("anchor"),
        ],
    },
    Table {
        name: "waivers",
        primary_key: &["id"],
        columns: &[
            I("id"),
            T("tenant_id"),
            I("repository_id"),
            T("rule_id"),
            T("paths"),
            T("reason"),
            T("owner"),
            T("author"),
            I("expires_at"),
            T("until"),
            T("policy_snapshot_id"),
            I("created_at"),
            I("revoked_at"),
        ],
    },
    Table {
        name: "findings",
        primary_key: &["id"],
        columns: &[
            I("id"),
            I("change_id"),
            T("fingerprint"),
            T("finding_id"),
            T("category"),
            T("rule_id"),
            T("path"),
            T("state"),
            T("first_run_id"),
            T("last_run_id"),
            I("predecessor_id"),
            I("updated_at"),
            T("severity"),
            T("title"),
        ],
    },
    Table {
        name: "publication_operations",
        primary_key: &["id"],
        columns: &[
            I("id"),
            I("object_id"),
            T("run_id"),
            T("result_id"),
            I("generation"),
            T("state"),
            T("attempt_id"),
            T("request_digest"),
            T("body"),
            I("attempts"),
            T("owner"),
            I("lease_epoch"),
            I("lease_until"),
            T("remote_id"),
            T("last_response"),
            T("last_error"),
            I("reconcile_deadline"),
            I("next_action_at"),
            I("created_at"),
            I("finished_at"),
        ],
    },
    Table {
        name: "finding_events",
        primary_key: &["id"],
        columns: &[
            I("id"),
            I("finding_id"),
            T("run_id"),
            T("from_state"),
            T("to_state"),
            T("reason"),
            T("actor"),
            T("evidence"),
            I("at"),
        ],
    },
    Table {
        name: "feedback",
        primary_key: &["id"],
        columns: &[
            I("id"),
            T("tenant_id"),
            I("change_id"),
            I("finding_id"),
            T("actor"),
            T("judgement"),
            T("scope"),
            T("rationale"),
            T("policy_snapshot_id"),
            I("at"),
        ],
    },
];

pub const IDENTITY_TABLES: &[&str] = &[
    "repositories",
    "changes",
    "inbox_events",
    "change_intents",
    "jobs",
    "change_events",
    "publication_objects",
    "publication_operations",
    "audit_events",
    "findings",
    "finding_events",
    "feedback",
    "waivers",
    "idempotency_keys",
];

pub async fn rows(pool: &AnyPool, postgres: bool, table: &Table) -> Vec<Vec<Value>> {
    let columns = table
        .columns
        .iter()
        .map(|column| format!("\"{}\"", column.name()))
        .collect::<Vec<_>>()
        .join(", ");
    let order = table
        .primary_key
        .iter()
        .map(|name| {
            let column = table
                .columns
                .iter()
                .find(|column| column.name() == *name)
                .expect("known PK");
            match column {
                Column::Integer(_) => format!("\"{name}\""),
                Column::Text(_) => format!(
                    "\"{name}\" COLLATE {}",
                    if postgres { "\"C\"" } else { "BINARY" }
                ),
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {columns} FROM {} ORDER BY {order}",
        table.name
    )))
    .fetch_all(pool)
    .await
    .expect("canonical table projection")
    .iter()
    .map(|row| {
        table
            .columns
            .iter()
            .enumerate()
            .map(|(index, column)| match column {
                Column::Integer(_) => row
                    .try_get::<Option<i64>, _>(index)
                    .expect("exact signed i64")
                    .map_or(Value::Null, Value::from),
                Column::Text(_) => row
                    .try_get::<Option<String>, _>(index)
                    .expect("exact text")
                    .map_or(Value::Null, Value::from),
            })
            .collect()
    })
    .collect()
}

pub async fn assert_empty(pool: &AnyPool) {
    for table in TABLES {
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM {}",
            table.name
        )))
        .fetch_one(pool)
        .await
        .expect("target count");
        assert_eq!(count, 0, "{} must remain empty", table.name);
    }
}

pub async fn identities(pool: &AnyPool) -> Vec<(i64, i64)> {
    let mut result = Vec::new();
    for table in IDENTITY_TABLES {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT last_value, CAST(CASE WHEN is_called THEN 1 ELSE 0 END AS BIGINT) FROM {table}_id_seq"
        ))).fetch_one(pool).await.expect("identity state");
        result.push((row.get(0), row.get(1)));
    }
    result
}
