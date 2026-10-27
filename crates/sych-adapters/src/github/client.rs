//! GitHub REST transport: the trusted API base from config, no redirects or proxies, typed
//! errors by mapping v1 (`sych_core::scm::map_status`).

use std::time::Duration;

use reqwest::redirect::Policy;
use reqwest::{Client, Method, RequestBuilder, Response};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sych_core::scm::{ScmCall, ScmError, ScmErrorKind, map_status};

use super::app::{AppKey, InstallationToken, Secret, TokenCache};

const TIMEOUT: Duration = Duration::from_secs(30);
const API_VERSION: &str = "2022-11-28";
const PER_PAGE: usize = 100;

#[derive(Deserialize)]
struct TokenReply {
    token: String,
    expires_at: jiff::Timestamp,
}

pub struct Github {
    http: Client,
    base: String,
    key: AppKey,
    tokens: TokenCache,
}

/// The installation that covers a repository and the permissions it was granted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installation {
    pub id: u64,
    pub permissions: Value,
}

fn transient(e: &reqwest::Error, call: ScmCall) -> ScmError {
    // A write whose reply was lost may have happened.
    let kind = if call == ScmCall::Read {
        ScmErrorKind::Transient
    } else {
        ScmErrorKind::PublicationUnknown
    };
    ScmError::new(kind, format!("github request: {e}"))
}

/// Maps a non-success reply; the message keeps GitHub's `message`, never request headers.
async fn failure(response: Response, call: ScmCall) -> ScmError {
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let retry_after = header("retry-after").and_then(|v| v.parse().ok());
    let rate_limited =
        retry_after.is_some() || header("x-ratelimit-remaining").as_deref() == Some("0");
    let body: Value = response.json().await.unwrap_or(Value::Null);
    let message = body["message"].as_str().unwrap_or("no message");
    ScmError {
        kind: map_status(status, rate_limited, call),
        message: format!("HTTP {status}: {message}"),
        retry_after_seconds: retry_after,
    }
}

impl Github {
    pub fn new(base: &str, key: AppKey) -> Result<Self, ScmError> {
        let http = Client::builder()
            .timeout(TIMEOUT)
            .redirect(Policy::none())
            .no_proxy()
            .user_agent(concat!("sych/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| ScmError::new(ScmErrorKind::Transient, format!("http client: {e}")))?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            key,
            tokens: TokenCache::default(),
        })
    }

    fn request(&self, method: Method, path: &str, token: &Secret) -> RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(token.expose())
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", API_VERSION)
    }

    pub(crate) async fn send<T: DeserializeOwned>(
        &self,
        request: RequestBuilder,
        call: ScmCall,
    ) -> Result<T, ScmError> {
        let response = request.send().await.map_err(|e| transient(&e, call))?;
        if !response.status().is_success() {
            return Err(failure(response, call).await);
        }

        response
            .json()
            .await
            .map_err(|e| ScmError::new(ScmErrorKind::Transient, format!("github reply: {e}")))
    }

    /// The installation of `owner/name`, as the App.
    pub async fn installation(&self, full_name: &str, now: i64) -> Result<Installation, ScmError> {
        let jwt = self.key.jwt(now)?;
        let v: Value = self
            .send(
                self.request(
                    Method::GET,
                    &format!("/repos/{full_name}/installation"),
                    &jwt,
                ),
                ScmCall::Read,
            )
            .await?;
        Ok(Installation {
            id: v["id"]
                .as_u64()
                .ok_or_else(|| ScmError::new(ScmErrorKind::Transient, "installation without id"))?,
            permissions: v["permissions"].clone(),
        })
    }

    /// An installation token limited to the one repository it is used for.
    pub async fn token(
        &self,
        installation: u64,
        full_name: &str,
        now: i64,
    ) -> Result<Secret, ScmError> {
        if let Some(token) = self.tokens.fresh(installation, full_name, now) {
            return Ok(token);
        }

        let name = full_name.rsplit('/').next().unwrap_or(full_name);
        let jwt = self.key.jwt(now)?;
        let reply: TokenReply = self
            .send(
                self.request(
                    Method::POST,
                    &format!("/app/installations/{installation}/access_tokens"),
                    &jwt,
                )
                .json(&serde_json::json!({ "repositories": [name] })),
                ScmCall::Read,
            )
            .await?;
        let token = Secret::new(reply.token);
        self.tokens.put(
            installation,
            full_name,
            InstallationToken {
                token: token.clone(),
                expires_at: reply.expires_at.as_second(),
            },
        );
        Ok(token)
    }

    /// A GET whose 3xx reply is returned as is (redirects are never followed with the token).
    pub(crate) async fn raw_get(&self, path: &str, token: &Secret) -> Result<Response, ScmError> {
        let response = self
            .request(Method::GET, path, token)
            .send()
            .await
            .map_err(|e| transient(&e, ScmCall::Read))?;
        if response.status().is_success() || response.status().is_redirection() {
            Ok(response)
        } else {
            Err(failure(response, ScmCall::Read).await)
        }
    }

    /// The client without any credential, for signed URLs outside the API.
    pub(crate) fn plain(&self) -> &Client {
        &self.http
    }

    pub async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        token: &Secret,
    ) -> Result<T, ScmError> {
        self.send(self.request(Method::GET, path, token), ScmCall::Read)
            .await
    }

    /// Every page of a list endpoint, up to `max` items; `true` when the list ended within it.
    pub async fn list(
        &self,
        path: &str,
        token: &Secret,
        max: usize,
    ) -> Result<(Vec<Value>, bool), ScmError> {
        let mut items = Vec::new();
        let sep = if path.contains('?') { '&' } else { '?' };
        for page in 1.. {
            let batch: Vec<Value> = self
                .get(
                    &format!("{path}{sep}per_page={PER_PAGE}&page={page}"),
                    token,
                )
                .await?;
            let last = batch.len() < PER_PAGE;
            items.extend(batch);
            if last {
                return Ok((items, true));
            }

            if items.len() >= max {
                items.truncate(max);
                return Ok((items, false));
            }
        }
        unreachable!("pages are unbounded")
    }

    pub async fn write<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        token: &Secret,
        body: &Value,
        call: ScmCall,
    ) -> Result<T, ScmError> {
        self.send(self.request(method, path, token).json(body), call)
            .await
    }

    /// A delete answers `204` with no body.
    pub(crate) async fn delete(&self, path: &str, token: &Secret) -> Result<(), ScmError> {
        let call = ScmCall::Write;
        let response = self
            .request(Method::DELETE, path, token)
            .send()
            .await
            .map_err(|e| transient(&e, call))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(failure(response, call).await)
        }
    }
}
