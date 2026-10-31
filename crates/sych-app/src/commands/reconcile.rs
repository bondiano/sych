use std::process::ExitCode;

use sych_core::Timestamp;
use sych_core::config::ApiRole;
use sych_storage::Storage;

use crate::args::ReconcileArguments;
use crate::config;
use crate::errors::Failure;
use crate::server::build::caller;
use crate::storage::{block_on, load_store_config};

/// Lists unresolved publications; never calls the SCM itself — the server reconciles. A new
/// CREATE happens only through `--allow-create`, an audited operator decision.
pub fn run(args: &ReconcileArguments) -> anyhow::Result<ExitCode> {
    let store = load_store_config(args.config.as_deref())?;
    let now = Timestamp::now();
    let (change, allow) = (args.change, args.allow_create);
    // The audited actor is a configured writer, not a free name.
    let actor = match (&args.actor, allow) {
        (Some(name), Some(_)) => {
            let server = config::server(args.config.as_deref())?;
            Some(caller(&server, name, ApiRole::Writer)?.name)
        }
        _ => None,
    };
    let reason = args.reason.clone();
    let (listed, allowed) = block_on(async move {
        let storage = Storage::open(&store).await?;
        let allowed = match (allow, actor, reason) {
            (Some(op), Some(actor), Some(reason)) => {
                let all = storage.unresolved_publications(None).await?;
                match all.iter().find(|u| u.op_id == op) {
                    // The audit record belongs to the change's tenant.
                    Some(u) => {
                        let tenant = storage.change(u.change_id).await?.partition.tenant_id;
                        Some(
                            storage
                                .authorize_recreate(op, &tenant, &actor, &reason, now)
                                .await?,
                        )
                    }
                    None => Some(false),
                }
            }
            _ => None,
        };
        let listed = storage.unresolved_publications(change).await?;
        Ok::<_, sych_storage::StorageError>((listed, allowed))
    })??;
    for u in &listed {
        let age = now.as_second() - u.created_at;
        let next = match u.reconcile_deadline {
            Some(d) if d <= now.as_second() => "operator decision (window exhausted)".to_owned(),
            Some(d) => format!("reconciling for {} s more", d - now.as_second()),
            None => "in flight".to_owned(),
        };
        println!(
            "op {} change {} {} {} age {age}s attempts {}: {} — {}",
            u.op_id,
            u.change_id,
            u.slot,
            u.state,
            u.attempts,
            next,
            u.last_error.as_deref().unwrap_or("")
        );
    }

    match allowed {
        Some(false) => Err(Failure::invalid(
            "the operation is not UNCERTAIN without a known remote id; nothing was changed",
        )
        .into()),
        Some(true) => {
            println!("new CREATE allowed and audited; a duplicate comment may appear");
            Ok(ExitCode::SUCCESS)
        }
        None => Ok(ExitCode::SUCCESS),
    }
}
