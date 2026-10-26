//! Sign in with `ChatGPT` for open-source local clients: PKCE authorization code with a
//! loopback redirect, dynamic client registration, the saved token set and its refresh.
//! Contract: `developers.openai.com/siwc/token-sharing-open-source` (sign-in, accounts and
//! sessions, errors and recovery). Tokens are never logged; the file is written owner-only.

use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::chat;

/// Official authorization server and inference resource of plan usage.
pub const AUTH_BASE: &str = "https://auth.openai.com";
pub const RESOURCE: &str = "https://api.openai.com/v1";
pub const SCOPE: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
/// Without this granted scope a valid sign-in does not authorize plan usage.
pub const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
pub const REDIRECT_PORT: u16 = 1455;
const DYNAMIC_CLIENT: &str = "dynamic_agent_client";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AuthError {
    /// Sign in again (`sych auth chatgpt`): no usable token set.
    #[error("sign-in needed: {0}")]
    SignIn(String),
    /// The plan is not authorized for this sign-in; do not loop through OAuth.
    #[error("ChatGPT plan usage is not granted: {0}")]
    NotGranted(String),
    #[error("client configuration: {0}")]
    Client(String),
    /// Network or server trouble; credentials are kept.
    #[error("auth server unavailable: {0}")]
    Unavailable(String),
    #[error("credential store: {0}")]
    Store(String),
}

/// One registration's token set. No `Debug`: it holds secrets.
#[derive(Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub label: String,
    pub issuer: String,
    pub subject: String,
    pub email: Option<String>,
    pub client_id: String,
    pub ext_agent_host_id: String,
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    /// Unix seconds.
    pub expires_at: i64,
    pub scopes: Vec<String>,
    pub saved_at: i64,
}

/// What a sign-in keeps between attempts: the issued client and the host id.
#[derive(Clone)]
pub struct Registration {
    pub client_id: Option<String>,
    pub ext_agent_host_id: String,
    pub id_token_hint: Option<String>,
}

/// Fresh per-attempt secrets of the authorization request.
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
    pub nonce: String,
}

pub fn now() -> i64 {
    jiff::Timestamp::now().as_second()
}

fn random(bytes: usize) -> Result<Vec<u8>, AuthError> {
    let mut buf = vec![0u8; bytes];
    fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .map_err(|e| AuthError::Store(format!("no randomness: {e}")))?;
    Ok(buf)
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Base64url without padding (RFC 4648 §5).
pub fn b64url(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(char::from(B64[(n >> (18 - 6 * i) & 63) as usize]));
        }
    }
    out
}

pub fn b64url_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in text.trim_end_matches('=').bytes() {
        let v = u32::try_from(B64.iter().position(|b| *b == c)?).ok()?;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from(acc >> bits & 0xff).ok()?);
        }
    }
    Some(out)
}

impl Pkce {
    pub fn new() -> Result<Self, AuthError> {
        let verifier = b64url(&random(32)?);
        let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
        Ok(Self {
            verifier,
            challenge,
            state: b64url(&random(24)?),
            nonce: b64url(&random(24)?),
        })
    }
}

/// A new opaque host id: not derived from the user, not a credential.
pub fn new_host_id() -> Result<String, AuthError> {
    let b = random(16)?;
    let hex = b.iter().fold(String::new(), |mut hex, x| {
        let _ = write!(hex, "{x:02x}");
        hex
    });
    // RFC 9562 version 4 and variant bits.
    let v = |s: &str, mask: u8, set: u8| {
        let n = u8::from_str_radix(s, 16).unwrap_or(0) & mask | set;
        format!("{n:02x}")
    };
    Ok(format!(
        "urn:uuid:{}-{}-{}{}-{}{}-{}",
        &hex[0..8],
        &hex[8..12],
        v(&hex[12..14], 0x0f, 0x40),
        &hex[14..16],
        v(&hex[16..18], 0x3f, 0x80),
        &hex[18..20],
        &hex[20..32]
    ))
}

pub fn redirect_uri(port: u16) -> String {
    format!("http://127.0.0.1:{port}/auth/callback")
}

pub fn authorize_url(
    auth_base: &str,
    registration: &Registration,
    pkce: &Pkce,
    redirect: &str,
) -> String {
    let mut url = Url::parse(&format!("{auth_base}/api/accounts/authorize")).expect("static url");
    {
        let mut q = url.query_pairs_mut();
        match &registration.client_id {
            Some(id) => q.append_pair("client_id", id),
            None => q
                .append_pair("client_id", DYNAMIC_CLIENT)
                .append_pair("agent_name_hint", "Sych"),
        };
        q.append_pair("ext_agent_host_id", &registration.ext_agent_host_id);
        if let Some(hint) = &registration.id_token_hint {
            q.append_pair("id_token_hint", hint);
        }

        q.append_pair("response_type", "code")
            .append_pair("redirect_uri", redirect)
            .append_pair("scope", SCOPE)
            .append_pair("resource", RESOURCE)
            .append_pair("state", &pkce.state)
            .append_pair("nonce", &pkce.nonce)
            .append_pair("code_challenge_method", "S256")
            .append_pair("code_challenge", &pkce.challenge);
    }
    url.into()
}

/// The parts of the loopback callback the flow needs.
pub struct Callback {
    pub code: String,
    pub client_id: Option<String>,
}

/// Checks a callback request target (`/auth/callback?...`) against this attempt.
pub fn callback(
    target: &str,
    pkce: &Pkce,
    registration: &Registration,
) -> Result<Callback, AuthError> {
    let url = Url::parse(&format!("http://127.0.0.1{target}"))
        .map_err(|_| AuthError::SignIn("malformed callback".into()))?;
    let get = |k: &str| {
        url.query_pairs()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.into_owned())
    };
    if get("state").as_deref() != Some(pkce.state.as_str()) {
        return Err(AuthError::SignIn("callback state does not match".into()));
    }

    if let Some(error) = get("error") {
        return Err(AuthError::NotGranted(error));
    }

    let code = get("code").ok_or_else(|| AuthError::SignIn("callback has no code".into()))?;
    let issued = get("client_id");
    let client_id = match (&registration.client_id, issued) {
        (Some(saved), Some(issued)) if *saved != issued => {
            return Err(AuthError::SignIn("callback names another client".into()));
        }
        (Some(saved), _) => Some(saved.clone()),
        (None, Some(issued)) if issued != DYNAMIC_CLIENT => Some(issued),
        (None, _) => {
            return Err(AuthError::SignIn(
                "registration returned no client id".into(),
            ));
        }
    };
    Ok(Callback { code, client_id })
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<i64>,
    scope: Option<String>,
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    aud: Value,
    exp: i64,
    nonce: Option<String>,
    email: Option<String>,
}

fn claims(id_token: &str) -> Result<Claims, AuthError> {
    let payload = id_token
        .split('.')
        .nth(1)
        .and_then(b64url_decode)
        .ok_or_else(|| AuthError::SignIn("malformed id token".into()))?;
    serde_json::from_slice(&payload).map_err(|_| AuthError::SignIn("malformed id token".into()))
}

/// Issuer, audience, expiry and nonce of an ID token taken straight from the token endpoint
/// over TLS; `OpenID` Connect Core §3.1.3.7 lets that channel stand in for the JWS signature.
pub fn check_id_token(
    id_token: &str,
    issuer: &str,
    client_id: &str,
    nonce: Option<&str>,
    at: i64,
) -> Result<(String, Option<String>), AuthError> {
    let c = claims(id_token)?;
    let audience = match &c.aud {
        Value::String(a) => a == client_id,
        Value::Array(a) => a.iter().any(|v| v.as_str() == Some(client_id)),
        _ => false,
    };
    let fresh = c.exp > at;
    if c.iss != issuer || !audience || !fresh || (nonce.is_some() && c.nonce.as_deref() != nonce) {
        return Err(AuthError::SignIn(
            "id token issuer, audience, expiry or nonce does not match".into(),
        ));
    }

    Ok((c.sub, c.email))
}

fn scopes(scope: Option<&str>) -> Vec<String> {
    scope
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

/// `application/x-www-form-urlencoded`, through the URL query serializer.
fn form(pairs: &[(&str, &str)]) -> String {
    let mut url = Url::parse("http://form.invalid/").expect("static url");
    url.query_pairs_mut().extend_pairs(pairs);
    url.query().unwrap_or_default().to_owned()
}

async fn token_request(
    client: &Client,
    auth_base: &str,
    pairs: &[(&str, &str)],
) -> Result<TokenResponse, AuthError> {
    let response = client
        .post(format!("{auth_base}/api/accounts/oauth/token"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form(pairs))
        .send()
        .await
        .map_err(|_| AuthError::Unavailable("token endpoint unreachable".into()))?;
    let status = response.status().as_u16();
    let text = response
        .text()
        .await
        .map_err(|_| AuthError::Unavailable("token response lost".into()))?;
    if status == 200 {
        return serde_json::from_str(&text)
            .map_err(|_| AuthError::Unavailable("malformed token response".into()));
    }

    let code = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v["error"].as_str().map(str::to_owned))
        .unwrap_or_default();
    Err(match code.as_str() {
        "invalid_client" => AuthError::Client(code),
        "invalid_grant"
        | "invalid_refresh_token"
        | "token_expired"
        | "refresh_token_expired"
        | "refresh_token_invalidated"
        | "refresh_token_reused" => AuthError::SignIn(code),
        _ if status >= 500 => AuthError::Unavailable(format!("HTTP {status}")),
        _ => AuthError::SignIn(format!("HTTP {status} {code}")),
    })
}

/// The `issuer` the discovery document names; ID tokens must carry exactly it.
pub async fn issuer(client: &Client, auth_base: &str) -> Result<String, AuthError> {
    let doc: Value = client
        .get(format!("{auth_base}/.well-known/openid-configuration"))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|_| AuthError::Unavailable("openid configuration unreachable".into()))?
        .json()
        .await
        .map_err(|_| AuthError::Unavailable("malformed openid configuration".into()))?;
    doc["issuer"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| AuthError::Unavailable("openid configuration has no issuer".into()))
}

pub struct Exchange<'a> {
    pub auth_base: &'a str,
    pub issuer: &'a str,
    pub label: &'a str,
    pub host_id: &'a str,
    pub redirect: &'a str,
}

/// Exchanges the code and checks identity and the plan scope before anything is saved.
pub async fn exchange(
    client: &Client,
    e: &Exchange<'_>,
    callback: &Callback,
    pkce: &Pkce,
) -> Result<Credentials, AuthError> {
    let client_id = callback.client_id.as_deref().unwrap_or_default();
    let token = token_request(
        client,
        e.auth_base,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
            ("code", &callback.code),
            ("code_verifier", &pkce.verifier),
            ("redirect_uri", e.redirect),
            ("resource", RESOURCE),
        ],
    )
    .await?;
    let id_token = token
        .id_token
        .ok_or_else(|| AuthError::SignIn("token response has no id token".into()))?;
    let at = now();
    let (subject, email) = check_id_token(&id_token, e.issuer, client_id, Some(&pkce.nonce), at)?;
    let scopes = scopes(token.scope.as_deref());
    if !scopes.iter().any(|s| s == PLAN_SCOPE) {
        return Err(AuthError::NotGranted(format!(
            "{PLAN_SCOPE} was not granted"
        )));
    }

    Ok(Credentials {
        label: e.label.to_owned(),
        issuer: e.issuer.to_owned(),
        subject,
        email,
        client_id: client_id.to_owned(),
        ext_agent_host_id: e.host_id.to_owned(),
        id_token,
        access_token: token.access_token,
        refresh_token: token
            .refresh_token
            .ok_or_else(|| AuthError::SignIn("token response has no refresh token".into()))?,
        token_type: token.token_type.unwrap_or_else(|| "Bearer".into()),
        expires_at: at + token.expires_in.unwrap_or(0),
        scopes,
        saved_at: at,
    })
}

/// Refreshes near expiry; the rotated refresh token replaces the old one with the rest.
pub async fn refresh(
    client: &Client,
    auth_base: &str,
    old: &Credentials,
) -> Result<Credentials, AuthError> {
    let token = token_request(
        client,
        auth_base,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", &old.client_id),
            ("refresh_token", &old.refresh_token),
            ("resource", RESOURCE),
        ],
    )
    .await?;
    let at = now();
    let mut new = old.clone();
    if let Some(id_token) = token.id_token {
        check_id_token(&id_token, &old.issuer, &old.client_id, None, at)?;
        new.id_token = id_token;
    }

    let granted = token.scope.as_deref().map(|s| scopes(Some(s)));
    new.scopes = granted.unwrap_or(new.scopes);
    if !new.scopes.iter().any(|s| s == PLAN_SCOPE) {
        return Err(AuthError::NotGranted(format!(
            "{PLAN_SCOPE} is no longer granted"
        )));
    }

    new.access_token = token.access_token;
    new.refresh_token = token.refresh_token.unwrap_or(new.refresh_token);
    new.expires_at = at + token.expires_in.unwrap_or(0);
    new.saved_at = at;
    Ok(new)
}

/// No redirects or system proxy, like every model transport.
pub fn client() -> Result<Client, AuthError> {
    chat::client().map_err(|e| AuthError::Client(e.message))
}

/// Model slugs the account may use, in the server's order; listing is not inference.
pub async fn list_models(client: &Client, token: &str) -> Result<Vec<String>, String> {
    let response = client
        .get(format!("{RESOURCE}/models"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| "model list unreachable".to_owned())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("model list HTTP {status}"));
    }

    let body: Value = response
        .json()
        .await
        .map_err(|_| "model list malformed".to_owned())?;
    Ok(body["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["visibility"] == "list")
        .filter_map(|m| m["slug"].as_str().or(m["id"].as_str()).map(str::to_owned))
        .collect())
}

pub fn load(path: &Path) -> Result<Option<Credentials>, AuthError> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| AuthError::Store(format!("{} is not a credential file", path.display()))),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(AuthError::Store(format!("{}: {e}", path.display()))),
    }
}

/// Writes owner-only through a temporary file and a rename, so a crash leaves the old set.
pub fn save(path: &Path, credentials: &Credentials) -> Result<(), AuthError> {
    let store = |e: io::Error| AuthError::Store(format!("{}: {e}", path.display()));
    let tmp = path.with_extension("tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(store)?;
    let bytes =
        serde_json::to_vec_pretty(credentials).map_err(|e| AuthError::Store(e.to_string()))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(store)?;
    fs::rename(&tmp, path).map_err(store)
}

/// Serializes refreshes of one token set across processes: a rotating refresh token must not
/// be raced. A lock older than a minute is taken to be left by a dead process.
pub struct RefreshLock(PathBuf);

impl RefreshLock {
    pub fn try_take(path: &Path) -> Result<Option<Self>, AuthError> {
        let lock = path.with_extension("lock");
        let stale = fs::metadata(&lock)
            .and_then(|m| m.modified())
            .is_ok_and(|t| t.elapsed().is_ok_and(|e| e.as_secs() > 60));
        if stale {
            let _ = fs::remove_file(&lock);
        }

        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&lock)
        {
            Ok(_) => Ok(Some(Self(lock))),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(None),
            Err(e) => Err(AuthError::Store(format!("{}: {e}", lock.display()))),
        }
    }
}

impl Drop for RefreshLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
