//! Who calls `/v1`: a configured API client by bearer token, or a browser session of a person
//! signed in through GitHub (or of a client that exchanged its token). Access is checked per
//! repository and role; reads trust a person's roles from sign-in, every change they make is
//! checked with GitHub again.

use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, StatusCode};
use ring::hmac;
use sych_core::api::Grant;
use sych_core::config::{ApiClient, ApiRole};
use sych_core::sha256;

use super::App;
use super::api::{engine, now, status, storage_error};
use super::error::ApiError;
use super::findings::Refused;

/// The cookie that carries a browser session's token.
pub const SESSION_COOKIE: &str = "sych_session";

/// The authenticated caller.
pub struct Caller {
    /// Recorded in audit, feedback, idempotency scope and tour sessions.
    pub actor: String,
    pub tenant_id: String,
    access: Access,
}

enum Access {
    Client(ApiClient),
    /// Roles as GitHub granted them at sign-in.
    Person {
        login: String,
        grants: Vec<Grant>,
    },
}

/// The configured client whose token is `presented`; tags compare in constant time.
pub(super) fn client_by_token(app: &App, presented: &str) -> Option<ApiClient> {
    let key = hmac::Key::new(hmac::HMAC_SHA256, b"sych api token comparison");
    let tag = hmac::sign(&key, presented.as_bytes());
    app.clients
        .iter()
        .find(|(_, token)| hmac::verify(&key, token.expose().as_bytes(), tag.as_ref()).is_ok())
        .map(|(client, _)| client.clone())
}

/// The value of cookie `name`.
pub(super) fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all("cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

/// The stored form of a session token.
pub(super) fn token_digest(token: &str) -> String {
    sha256(token.as_bytes()).to_string()
}

/// A browser's request that changes something comes from the UI's own origin; a cookie alone
/// would let another site act for the person.
pub(super) fn same_origin(app: &App, headers: &HeaderMap) -> bool {
    let expected = app
        .public_base_url
        .as_deref()
        .and_then(|u| url::Url::parse(u).ok())
        .map(|u| u.origin().ascii_serialization());
    let origin = headers.get("origin").and_then(|v| v.to_str().ok());
    expected.is_some() && origin == expected.as_deref()
}

fn unauthenticated(message: &str) -> ApiError {
    status(
        StatusCode::UNAUTHORIZED,
        "UNAUTHENTICATED",
        message.to_owned(),
    )
}

fn forbidden(message: String) -> ApiError {
    status(StatusCode::FORBIDDEN, "FORBIDDEN", message)
}

fn not_found() -> ApiError {
    status(StatusCode::NOT_FOUND, "NOT_FOUND", "no such object")
}

/// GitHub's repository level as a role; `none` and custom roles grant nothing.
pub(super) fn role_of(level: &str) -> Option<ApiRole> {
    match level {
        "admin" | "maintain" => Some(ApiRole::Maintainer),
        "write" => Some(ApiRole::Writer),
        "triage" => Some(ApiRole::Triage),
        "read" => Some(ApiRole::Reader),
        _ => None,
    }
}

impl FromRequestParts<Arc<App>> for Caller {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, app: &Arc<App>) -> Result<Self, ApiError> {
        let bearer = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        if let Some(presented) = bearer {
            return client_by_token(app, presented)
                .map(Self::client)
                .ok_or_else(|| unauthenticated("unknown token"));
        }

        let token = cookie(&parts.headers, SESSION_COOKIE)
            .ok_or_else(|| unauthenticated("a bearer token or a session is required"))?;
        if !matches!(parts.method, Method::GET | Method::HEAD) && !same_origin(app, &parts.headers)
        {
            return Err(forbidden(
                "a session changes nothing from another origin".into(),
            ));
        }

        let session = app
            .storage
            .session(&token_digest(token), now(app).as_second())
            .await
            .map_err(|e| storage_error(&e))?
            .ok_or_else(|| unauthenticated("the session ended or expired"))?;
        if session.kind == "CLIENT" {
            // A client removed from the config loses its sessions with it.
            return app
                .clients
                .iter()
                .find(|(c, _)| c.name == session.actor && c.tenant_id == session.tenant_id)
                .map(|(c, _)| Self::client(c.clone()))
                .ok_or_else(|| unauthenticated("the session's client is not configured"));
        }

        let login = session
            .actor
            .strip_prefix("github:")
            .unwrap_or(&session.actor)
            .to_owned();
        Ok(Self {
            actor: session.actor,
            tenant_id: session.tenant_id,
            access: Access::Person {
                login,
                grants: session.grants,
            },
        })
    }
}

impl Caller {
    pub(super) fn client(client: ApiClient) -> Self {
        Self {
            actor: client.name.clone(),
            tenant_id: client.tenant_id.clone(),
            access: Access::Client(client),
        }
    }

    /// `user` for a person, `client` for an API client.
    pub(super) fn kind(&self) -> &'static str {
        match self.access {
            Access::Client(_) => "client",
            Access::Person { .. } => "user",
        }
    }

    /// The role somewhere at all; the repository's own is checked by [`Caller::scope`].
    pub(super) fn need(&self, role: ApiRole) -> Result<(), ApiError> {
        let has = match &self.access {
            Access::Client(c) => c.role >= role,
            // Signed in reads at least nothing: lists are filtered by repository anyway.
            Access::Person { grants, .. } => {
                role == ApiRole::Reader || grants.iter().any(|g| g.role >= role)
            }
        };
        if has {
            Ok(())
        } else {
            Err(forbidden(format!("{role:?} is needed").to_lowercase()))
        }
    }

    /// The caller's role on `connection/full_name`, if it may read it.
    pub(super) fn role_on(&self, connection: &str, full_name: &str) -> Option<ApiRole> {
        match &self.access {
            Access::Client(c) => c.allows(connection, full_name).then_some(c.role),
            Access::Person { grants, .. } => grants
                .iter()
                .find(|g| {
                    g.repository.connection_id == connection
                        && g.repository.full_name.eq_ignore_ascii_case(full_name)
                })
                .map(|g| g.role),
        }
    }

    /// Reads every repository of its tenant: a client scoped to `*`.
    pub(super) fn reads_everything(&self) -> bool {
        matches!(&self.access, Access::Client(c) if c.repositories.iter().any(|r| r == "*"))
    }

    /// Repository authorization for `role`; a repository it may not read is not found. A
    /// person's change is confirmed with GitHub now, not only by the roles of sign-in.
    pub(super) async fn scope(
        &self,
        app: &App,
        connection: Option<&str>,
        full_name: Option<&str>,
        role: ApiRole,
    ) -> Result<(), ApiError> {
        let (Some(connection), Some(full_name)) = (connection, full_name) else {
            return Err(not_found());
        };
        let granted = self.role_on(connection, full_name).ok_or_else(not_found)?;
        if granted < role {
            return Err(forbidden(format!("{role:?} is needed").to_lowercase()));
        }

        let Access::Person { login, .. } = &self.access else {
            return Ok(());
        };
        if role == ApiRole::Reader {
            return Ok(());
        }

        let level = engine(app)?
            .level(connection, full_name, login)
            .await
            .map_err(|e| match e {
                Refused::Unavailable(m) => storage_error(&m),
                Refused::Permission { .. } => forbidden(e.to_string()),
            })?;
        match role_of(&level) {
            Some(now) if now >= role => Ok(()),
            _ => Err(forbidden(format!(
                "GitHub now grants `{level}` on {full_name}; {role:?} is needed"
            ))),
        }
    }
}
