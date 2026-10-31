//! `sych auth chatgpt`: the official Sign in with `ChatGPT` flow for a local owner. The
//! browser returns to a loopback listener; on a remote host the redirected URL can
//! be pasted instead. Only the token file is written; no model request is made.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;
use std::{fs, io, thread};

use sych_adapters::siwc::auth::{self, AUTH_BASE, AuthError, Exchange, Pkce, Registration};
use sych_core::config::{Adapter, RunMode};

use crate::args::AuthChatgptArguments;
use crate::backend::{ResolveError, connection_file};
use crate::config;
use crate::errors::Failure;
use crate::storage::block_on;

const WAIT: Duration = Duration::from_secs(600);

/// The request target of the first `GET /auth/callback` the listener receives.
fn listen(listener: &TcpListener, found: &mpsc::Sender<String>) {
    for stream in listener.incoming().flatten() {
        let mut line = String::new();
        let mut reader = BufReader::new(&stream);
        if reader.read_line(&mut line).is_err() {
            continue;
        }

        let target = line
            .split_whitespace()
            .nth(1)
            .unwrap_or_default()
            .to_owned();
        let ours = target.starts_with("/auth/callback?");
        let (status, body) = if ours {
            (
                "200 OK",
                "Sych received the sign-in. You can close this tab.",
            )
        } else {
            ("404 Not Found", "")
        };
        let _ = (&stream).write_all(
            format!(
                "HTTP/1.1 {status}\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        );
        if ours {
            let _ = found.send(target);
            return;
        }
    }
}

/// A pasted redirect URL, for a browser that cannot reach this host's loopback.
fn paste(found: &mpsc::Sender<String>) {
    for line in io::stdin().lock().lines().map_while(Result::ok) {
        if let Some(i) = line.find("/auth/callback?") {
            let _ = found.send(line[i..].trim().to_owned());
            return;
        }
    }
}

/// The host id is chosen once and kept even when a sign-in fails.
fn host_id(path: &Path, saved: Option<&auth::Credentials>) -> Result<String, Failure> {
    if let Some(c) = saved {
        return Ok(c.ext_agent_host_id.clone());
    }

    let file = path.with_extension("host");
    if let Ok(id) = fs::read_to_string(&file) {
        return Ok(id.trim().to_owned());
    }

    let id = auth::new_host_id().map_err(|e| Failure::invalid(e.to_string()))?;
    fs::write(&file, &id).map_err(|e| Failure::invalid(format!("{}: {e}", file.display())))?;
    Ok(id)
}

fn failure(e: &AuthError) -> Failure {
    match e {
        AuthError::SignIn(_) | AuthError::NotGranted(_) => Failure::permission(e.to_string()),
        _ => Failure::no_result(e.to_string()),
    }
}

pub fn run(args: &AuthChatgptArguments) -> anyhow::Result<ExitCode> {
    let server = config::effective(Some(&args.config), None)?.server;
    let name = &args.provider;
    let provider = server
        .models
        .providers
        .get(name)
        .filter(|p| p.adapter == Adapter::ChatgptSubscription)
        .ok_or_else(|| {
            Failure::invalid(format!("`{name}` is not a chatgpt_subscription provider"))
        })?;
    if !provider.allowed_modes.contains(&RunMode::PersonalLocal) {
        return Err(
            Failure::invalid(format!("`{name}` is not allowed in personal_local mode")).into(),
        );
    }

    let path: PathBuf = connection_file(provider, name).map_err(|e| match e {
        ResolveError::Invalid(e) | ResolveError::Credentials(e) => Failure::invalid(e),
    })?;
    let saved = auth::load(&path).map_err(|e| failure(&e))?;
    let registration = Registration {
        client_id: saved.as_ref().map(|c| c.client_id.clone()),
        ext_agent_host_id: host_id(&path, saved.as_ref())?,
        id_token_hint: saved.as_ref().map(|c| c.id_token.clone()),
    };
    let pkce = Pkce::new().map_err(|e| failure(&e))?;
    let redirect = auth::redirect_uri(args.port);
    let listener = TcpListener::bind(("127.0.0.1", args.port))
        .map_err(|e| Failure::invalid(format!("cannot listen on 127.0.0.1:{}: {e}", args.port)))?;
    let (tx, rx) = mpsc::channel();
    let tx2 = tx.clone();
    thread::spawn(move || listen(&listener, &tx));
    thread::spawn(move || paste(&tx2));

    eprintln!(
        "Open this URL in a browser and sign in to ChatGPT:\n\n{}\n\n\
         The browser returns to http://127.0.0.1:{port}. On a remote host forward the port \
         (ssh -L {port}:127.0.0.1:{port} HOST) or paste the address the browser ends on here.",
        auth::authorize_url(AUTH_BASE, &registration, &pkce, &redirect),
        port = args.port
    );
    let target = rx
        .recv_timeout(WAIT)
        .map_err(|_| Failure::no_result("no sign-in arrived within 10 minutes"))?;
    let callback = auth::callback(&target, &pkce, &registration).map_err(|e| failure(&e))?;
    let label = format!("{name}@{}", args.config.display());
    let (credentials, models) = block_on(async {
        let client = auth::client().map_err(|e| failure(&e))?;
        let issuer = auth::issuer(&client, AUTH_BASE)
            .await
            .map_err(|e| failure(&e))?;
        let exchange = Exchange {
            auth_base: AUTH_BASE,
            issuer: &issuer,
            label: &label,
            host_id: &registration.ext_agent_host_id,
            redirect: &redirect,
        };
        let credentials = auth::exchange(&client, &exchange, &callback, &pkce)
            .await
            .map_err(|e| failure(&e))?;
        if let Some(old) = &saved
            && old.subject != credentials.subject
        {
            return Err(Failure::invalid(
                "signed in as another account; its file is kept, use another connection_ref_env",
            ));
        }

        let models = auth::list_models(&client, &credentials.access_token).await;
        Ok((credentials, models))
    })??;
    auth::save(&path, &credentials).map_err(|e| failure(&e))?;
    eprintln!(
        "Signed in{}; token file {} (0600).",
        credentials
            .email
            .as_deref()
            .map(|e| format!(" as {e}"))
            .unwrap_or_default(),
        path.display()
    );
    match models {
        Ok(models) => eprintln!("Models for this account: {}", models.join(", ")),
        Err(e) => eprintln!("Could not list models ({e}); set the profile model by hand."),
    }
    Ok(ExitCode::SUCCESS)
}
