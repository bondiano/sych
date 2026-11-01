use std::process::ExitCode;
use std::sync::Arc;

use sych_core::Timestamp;
use sych_storage::Storage;

use crate::args::ServeArguments;
use crate::config;
use crate::server::build::{EngineConfig, connections, engine};
use crate::server::{App, Role, serve};
use crate::storage::{block_on, store_config};

/// `sych serve`, `sych api` or `sych worker`: one engine, placed by `role`.
pub fn run(args: &ServeArguments, role: Role) -> anyhow::Result<ExitCode> {
    let server = config::server(Some(&args.config))?;
    let store = store_config(Some(&args.config), &server)?;
    let bind = server.http.bind.parse()?;
    let conns = connections(&server)?;
    let shown = config::shown(&args.config)?;
    block_on(async {
        let storage = Storage::open(&store).await?;
        let app = Arc::new(App {
            config: Some(shown),
            ..App::new(&server, storage, store.artifacts.clone(), Timestamp::now)?
        });
        let engine = engine(
            Arc::clone(&app),
            conns,
            EngineConfig {
                config: &args.config,
                server: &server,
                store,
                models: args.models.clone(),
                semantic: args.semantic,
            },
        );
        serve(app, bind, Some(engine), role).await
    })??;
    Ok(ExitCode::SUCCESS)
}
