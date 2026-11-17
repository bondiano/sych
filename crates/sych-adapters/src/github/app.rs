//! GitHub App credentials: a short-lived RS256 JWT signs as the App, and
//! installation tokens scoped to one repository are cached until shortly before they expire,
//! independently of any run.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Mutex;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::rand::SystemRandom;
use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
use serde_json::{Map, Value};
use sych_core::scm::{ScmError, ScmErrorKind};

/// A token this close to expiry is refreshed before use.
const REFRESH_MARGIN_SECONDS: i64 = 300;

/// A credential value; never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

pub struct AppKey {
    pub app_id: u64,
    key: RsaKeyPair,
}

fn unauthorized(message: &str) -> ScmError {
    ScmError::new(ScmErrorKind::Unauthorized, message)
}

impl AppKey {
    /// Reads a PEM private key as GitHub issues it (PKCS#1) or as PKCS#8.
    pub fn from_pem(app_id: u64, pem: &str) -> Result<Self, ScmError> {
        let pkcs1 = pem.contains("BEGIN RSA PRIVATE KEY");
        let body: String = pem
            .lines()
            .filter(|l| !l.starts_with("-----"))
            .map(str::trim)
            .collect();
        let der = STANDARD
            .decode(body)
            .map_err(|_| unauthorized("the App private key is not PEM"))?;
        let key = if pkcs1 {
            RsaKeyPair::from_der(&der)
        } else {
            RsaKeyPair::from_pkcs8(&der)
        }
        .map_err(|_| unauthorized("the App private key is not an RSA key"))?;
        Ok(Self { app_id, key })
    }

    /// The App JWT: issued a minute in the past for clock drift, valid nine minutes.
    pub fn jwt(&self, now: i64) -> Result<Secret, ScmError> {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
        let claims = URL_SAFE_NO_PAD.encode(format!(
            r#"{{"iat":{},"exp":{},"iss":"{}"}}"#,
            now - 60,
            now + 540,
            self.app_id
        ));
        let message = format!("{header}.{claims}");
        let mut signature = vec![0; self.key.public().modulus_len()];
        self.key
            .sign(
                &RSA_PKCS1_SHA256,
                &SystemRandom::new(),
                message.as_bytes(),
                &mut signature,
            )
            .map_err(|_| unauthorized("cannot sign the App JWT"))?;
        Ok(Secret(format!(
            "{message}.{}",
            URL_SAFE_NO_PAD.encode(signature)
        )))
    }
}

#[derive(Debug, Clone)]
pub struct InstallationToken {
    pub token: Secret,
    /// Unix seconds.
    pub expires_at: i64,
}

/// What an installation token may do. Reading the change, its code and its CI never needs a
/// write, so the token that fetches untrusted code cannot publish; only the publisher's can.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TokenScope {
    Read,
    Publish,
}

impl TokenScope {
    /// The permissions this scope asks for, capped by what the installation was `granted`: one
    /// it lacks is left out, since GitHub refuses a token that asks for more.
    pub fn permissions(self, granted: &Value) -> Value {
        let wanted: &[(&str, &str)] = match self {
            Self::Read => &[
                ("actions", "read"),
                ("checks", "read"),
                ("contents", "read"),
                ("issues", "read"),
                ("metadata", "read"),
                ("pull_requests", "read"),
            ],
            Self::Publish => &[
                ("checks", "write"),
                ("issues", "write"),
                ("metadata", "read"),
                ("pull_requests", "write"),
            ],
        };
        let rank = |level: &str| match level {
            "read" => 1,
            "write" => 2,
            _ => 0,
        };
        let mut out = Map::new();
        for &(name, level) in wanted {
            let have = granted[name].as_str().unwrap_or("none");
            let level = if rank(have) < rank(level) {
                have
            } else {
                level
            };
            if rank(level) > 0 {
                out.insert(name.to_owned(), Value::String(level.to_owned()));
            }
        }
        Value::Object(out)
    }
}

/// Tokens by `(installation, repository, scope)`, shared by the API and the engine.
#[derive(Debug, Default)]
pub struct TokenCache {
    tokens: Mutex<BTreeMap<(u64, String, TokenScope), InstallationToken>>,
}

impl TokenCache {
    /// A cached token with more than the refresh margin left.
    pub fn fresh(
        &self,
        installation: u64,
        repo: &str,
        scope: TokenScope,
        now: i64,
    ) -> Option<Secret> {
        self.tokens
            .lock()
            .ok()?
            .get(&(installation, repo.to_owned(), scope))
            .filter(|t| t.expires_at - REFRESH_MARGIN_SECONDS > now)
            .map(|t| t.token.clone())
    }

    pub fn put(&self, installation: u64, repo: &str, scope: TokenScope, token: InstallationToken) {
        if let Ok(mut tokens) = self.tokens.lock() {
            tokens.insert((installation, repo.to_owned(), scope), token);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    use super::*;

    #[test]
    fn a_scope_asks_for_no_more_than_it_needs_or_the_installation_has() {
        let granted = serde_json::json!({"contents": "write", "pull_requests": "read",
            "checks": "write", "metadata": "read", "administration": "write"});
        assert_eq!(
            TokenScope::Read.permissions(&granted),
            serde_json::json!({"checks": "read", "contents": "read", "metadata": "read",
                "pull_requests": "read"})
        );
        assert_eq!(
            TokenScope::Publish.permissions(&granted),
            serde_json::json!({"checks": "write", "metadata": "read", "pull_requests": "read"})
        );
    }

    #[test]
    fn tokens_refresh_before_expiry() {
        let cache = TokenCache::default();
        cache.put(
            1,
            "r",
            TokenScope::Read,
            InstallationToken {
                token: Secret::new("t".into()),
                expires_at: 1_000,
            },
        );
        assert!(cache.fresh(1, "r", TokenScope::Read, 600).is_some());
        assert!(
            cache.fresh(1, "r", TokenScope::Read, 701).is_none(),
            "inside the margin"
        );
        assert!(cache.fresh(1, "other", TokenScope::Read, 0).is_none());
        assert!(
            cache.fresh(1, "r", TokenScope::Publish, 600).is_none(),
            "per scope"
        );
        assert_eq!(format!("{:?}", Secret::new("t".into())), "Secret(***)");
    }

    /// Signs with a fresh key from `ssh-keygen` (PKCS#1 as GitHub issues it, and PKCS#8) and
    /// verifies with its public half; skipped where `ssh-keygen` is missing.
    #[test]
    fn jwt_is_rs256_signed_by_the_app_key() {
        use ring::signature::{KeyPair, RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey};
        let dir = tempfile::tempdir().expect("tempdir");
        for format in ["PEM", "PKCS8"] {
            let path = dir.path().join(format);
            let made = Command::new("ssh-keygen")
                .args([
                    "-q", "-t", "rsa", "-b", "2048", "-m", format, "-N", "", "-f",
                ])
                .arg(&path)
                .status();
            if !made.is_ok_and(|s| s.success()) {
                eprintln!("ssh-keygen unavailable; skipped");
                return;
            }

            let pem = fs::read_to_string(&path).expect("key");
            let key = AppKey::from_pem(123, &pem).expect("rsa key");
            let jwt = key.jwt(1_800_000_000).expect("jwt");
            let parts: Vec<&str> = jwt.expose().split('.').collect();
            assert_eq!(parts.len(), 3);
            let claims =
                String::from_utf8(URL_SAFE_NO_PAD.decode(parts[1]).expect("b64")).expect("utf8");
            assert_eq!(claims, r#"{"iat":1799999940,"exp":1800000540,"iss":"123"}"#);
            let signature = URL_SAFE_NO_PAD.decode(parts[2]).expect("b64");
            let public = UnparsedPublicKey::new(
                &RSA_PKCS1_2048_8192_SHA256,
                key.key.public_key().as_ref().to_vec(),
            );
            public
                .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
                .expect("valid signature");
        }
    }

    #[test]
    fn garbage_is_not_a_key() {
        assert!(AppKey::from_pem(1, "-----BEGIN RSA PRIVATE KEY-----\nAAAA\n-----END").is_err());
    }
}
