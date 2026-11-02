//! Browser sign-in: a person through the App's GitHub user authorization, or an API client by
//! its bearer token. A session is an `HttpOnly` cookie whose token only the database's SHA-256
//! knows; it lasts 8 hours and ends at logout.

use std::fmt::{Display, Write};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use ring::rand::{SecureRandom, SystemRandom};
use serde::Deserialize;
use serde_json::json;
use sych_adapters::github::Secret;
use sych_core::api::{Grant, Me, SignIn};
use sych_core::config::GithubOauth;
use sych_storage::Session;

use super::App;
use super::api::{engine, now, status, storage_error};
use super::caller::{
    Caller, SESSION_COOKIE, client_by_token, cookie, role_of, same_origin, token_digest,
};
use super::error::ApiError;
use super::views::grants;

const SESSION_SECONDS: i64 = 8 * 3600;
const STATE_COOKIE: &str = "sych_oauth_state";
const STATE_SECONDS: i64 = 600;
/// Where the browser lands after signing in.
const HOME: &str = "/ui/";

fn random_token() -> Result<String, ApiError> {
    let mut bytes = [0_u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| storage_error(&"no system randomness"))?;
    Ok(bytes.iter().fold(String::new(), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    }))
}

/// A `Set-Cookie` value; `Secure` whenever the UI is served over https.
fn set_cookie(app: &App, name: &str, value: &str, path: &str, seconds: i64) -> HeaderValue {
    let secure = app
        .public_base_url
        .as_deref()
        .is_some_and(|u| u.starts_with("https://"));
    let text = format!(
        "{name}={value}; Path={path}; Max-Age={seconds}; HttpOnly; SameSite=Lax{}",
        if secure { "; Secure" } else { "" }
    );
    HeaderValue::from_str(&text).unwrap_or_else(|_| HeaderValue::from_static(""))
}

fn forbidden(message: &str) -> ApiError {
    status(StatusCode::FORBIDDEN, "FORBIDDEN", message.to_owned())
}

/// The connection's OAuth client and secret; one without sign-in is not found.
fn oauth<'a>(app: &'a App, connection: &str) -> Result<(&'a GithubOauth, &'a Secret), ApiError> {
    let config = app
        .github
        .get(connection)
        .and_then(|(c, _)| c.oauth.as_ref());
    let secret = app.oauth.get(connection);
    match (config, secret, &app.public_base_url) {
        (Some(config), Some(secret), Some(_)) => Ok((config, secret)),
        _ => Err(status(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            "no sign-in for this connection",
        )),
    }
}

fn callback_url(app: &App, connection: &str) -> String {
    let base = app.public_base_url.as_deref().unwrap_or_default();
    format!(
        "{}/auth/github/{connection}/callback",
        base.trim_end_matches('/')
    )
}

/// Starts a session and answers with its cookie and `then`.
async fn start(app: &App, session: &Session, then: Response) -> Result<Response, ApiError> {
    let token = random_token()?;
    app.storage
        .start_session(&token_digest(&token), session, now(app).as_second())
        .await
        .map_err(|e| storage_error(&e))?;
    let mut response = then;
    response.headers_mut().append(
        header::SET_COOKIE,
        set_cookie(app, SESSION_COOKIE, &token, "/", SESSION_SECONDS),
    );
    Ok(response)
}

/// `GET /auth/github`: the connections a person can sign in through.
pub async fn sign_ins(State(app): State<Arc<App>>) -> Json<Vec<SignIn>> {
    Json(
        app.github
            .keys()
            .filter(|id| oauth(&app, id).is_ok())
            .map(|id| SignIn {
                connection_id: id.clone(),
                login_url: format!("/auth/github/{id}/login"),
            })
            .collect(),
    )
}

/// `GET /auth/github/:connection/login`: to GitHub, with a state only this browser holds.
pub async fn login(
    State(app): State<Arc<App>>,
    Path(connection): Path<String>,
) -> Result<Response, ApiError> {
    let (config, _) = oauth(&app, &connection)?;
    let state = random_token()?;
    let authorize = url::Url::parse_with_params(
        &format!(
            "{}/login/oauth/authorize",
            config.web_base_url.trim_end_matches('/')
        ),
        [
            ("client_id", config.client_id.as_str()),
            ("redirect_uri", &callback_url(&app, &connection)),
            ("state", &state),
        ],
    )
    .map_err(|e| storage_error(&e))?;
    let path = format!("/auth/github/{connection}");
    Ok((
        StatusCode::FOUND,
        [
            (
                header::LOCATION,
                HeaderValue::from_str(authorize.as_str()).map_err(|e| storage_error(&e))?,
            ),
            (
                header::SET_COOKIE,
                set_cookie(&app, STATE_COOKIE, &state, &path, STATE_SECONDS),
            ),
        ],
    )
        .into_response())
}

#[derive(Deserialize)]
pub struct Callback {
    code: String,
    state: String,
}

/// `GET /auth/github/:connection/callback`: who signed in and their role on each repository of
/// this connection that Sych knows. The user token is dropped here.
pub async fn callback(
    State(app): State<Arc<App>>,
    Path(connection): Path<String>,
    headers: HeaderMap,
    Query(q): Query<Callback>,
) -> Result<Response, ApiError> {
    let (config, secret) = oauth(&app, &connection)?;
    if cookie(&headers, STATE_COOKIE) != Some(q.state.as_str()) {
        return Err(forbidden("the sign-in state does not match; start again"));
    }

    let conn = engine(&app)?
        .conns
        .get(&connection)
        .ok_or_else(|| forbidden("the connection is not configured"))?;
    let refused = |e: &dyn Display| forbidden(&format!("GitHub refused: {e}"));
    let token = conn
        .gh
        .user_token(
            &config.web_base_url,
            &config.client_id,
            secret,
            &q.code,
            &callback_url(&app, &connection),
        )
        .await
        .map_err(|e| refused(&e))?;
    let login = conn.gh.user_login(&token).await.map_err(|e| refused(&e))?;
    let reached = conn
        .gh
        .user_repositories(&token)
        .await
        .map_err(|e| refused(&e))?;
    let tenant = conn.config.tenant_id.clone();
    let known = app
        .storage
        .repositories(&tenant)
        .await
        .map_err(|e| storage_error(&e))?;
    let grants: Vec<Grant> = known
        .into_iter()
        .filter(|r| r.connection_id == connection && conn.config.allows(&r.full_name))
        .filter_map(|repository| {
            let (_, level) = reached
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(&repository.full_name))?;
            Some(Grant {
                repository,
                role: role_of(level)?,
            })
        })
        .collect();
    let session = Session {
        tenant_id: tenant,
        actor: format!("github:{login}"),
        kind: "USER".into(),
        grants,
        expires_at: now(&app).as_second() + SESSION_SECONDS,
    };
    let mut home = (StatusCode::SEE_OTHER, [(header::LOCATION, HOME)]).into_response();
    home.headers_mut().append(
        header::SET_COOKIE,
        set_cookie(
            &app,
            STATE_COOKIE,
            "",
            &format!("/auth/github/{connection}"),
            0,
        ),
    );
    start(&app, &session, home).await
}

/// `POST /auth/token`: a browser session acting as the API client of the bearer token.
pub async fn token(State(app): State<Arc<App>>, headers: HeaderMap) -> Result<Response, ApiError> {
    let client = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .and_then(|t| client_by_token(&app, t))
        .ok_or_else(|| {
            status(
                StatusCode::UNAUTHORIZED,
                "UNAUTHENTICATED",
                "a client's bearer token is required",
            )
        })?;
    let session = Session {
        tenant_id: client.tenant_id.clone(),
        actor: client.name.clone(),
        kind: "CLIENT".into(),
        grants: Vec::new(),
        expires_at: now(&app).as_second() + SESSION_SECONDS,
    };
    let reply = Json(json!({"actor": client.name})).into_response();
    start(&app, &session, reply).await
}

/// `POST /auth/logout`: ends the browser's session, also one that already expired.
pub async fn logout(State(app): State<Arc<App>>, headers: HeaderMap) -> Result<Response, ApiError> {
    if !same_origin(&app, &headers) {
        return Err(forbidden("a session changes nothing from another origin"));
    }

    if let Some(token) = cookie(&headers, SESSION_COOKIE) {
        app.storage
            .end_session(&token_digest(token), now(&app).as_second())
            .await
            .map_err(|e| storage_error(&e))?;
    }

    let mut reply = Json(json!({"ended": true})).into_response();
    reply.headers_mut().append(
        header::SET_COOKIE,
        set_cookie(&app, SESSION_COOKIE, "", "/", 0),
    );
    Ok(reply)
}

/// `GET /v1/me`.
pub async fn me(State(app): State<Arc<App>>, caller: Caller) -> Result<Json<Me>, ApiError> {
    let repositories = grants(&app, &caller).await?;
    Ok(Json(Me {
        admin: super::admin::operator(&caller),
        kind: caller.kind().into(),
        actor: caller.actor,
        tenant_id: caller.tenant_id,
        repositories,
    }))
}
