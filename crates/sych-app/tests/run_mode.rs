//! A server run never takes a route that is allowed only locally: a personal
//! subscription signed in on the owner's machine is not a hosted reviewer's credential.

use std::fs;
use std::path::Path;

use sych_app::backend::{ResolveError, resolve};
use sych_core::config::{RunMode, ServerConfig};

#[test]
fn subscriptions_resolve_locally_but_not_in_a_server_run() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/personal/sych.yaml");
    let text = fs::read_to_string(&path).expect("example");
    let server = ServerConfig::parse("sych.yaml", &text).expect("valid");
    let local_only = server
        .models
        .profiles
        .iter()
        .find(|(_, p)| {
            server.models.providers[&p.provider].allowed_modes == [RunMode::PersonalLocal]
        })
        .map(|(name, _)| name.clone())
        .expect("a local-only subscription profile");
    let refused = resolve(&path, &server, &local_only, RunMode::PersonalService);
    let Err(ResolveError::Invalid(e)) = refused else {
        panic!("a local-only provider resolved in a server run");
    };
    assert!(e.contains("not allowed in PersonalService mode"), "{e}");
}
