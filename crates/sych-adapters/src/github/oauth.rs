//! A person's GitHub identity for the web UI: the code exchange of the App's user
//! authorization, their login, and their role on each repository of the App's installations
//! they reach. The user token serves these reads only and is never stored.

use serde_json::{Value, json};
use sych_core::scm::{ScmCall, ScmError, ScmErrorKind};

use super::{Github, Secret};

/// Repositories read per installation; a person reaching more signs in with the first ones.
const MAX_PAGES: usize = 50;
const PER_PAGE: usize = 100;

/// `admin | maintain | write | triage | read | none` from a repository's `permissions` flags.
fn role(permissions: &Value) -> &'static str {
    let flag = |name: &str| permissions[name].as_bool().unwrap_or(false);
    [
        ("admin", "admin"),
        ("maintain", "maintain"),
        ("push", "write"),
        ("triage", "triage"),
        ("pull", "read"),
    ]
    .into_iter()
    .find(|(name, _)| flag(name))
    .map_or("none", |(_, role)| role)
}

impl Github {
    /// Exchanges the authorization `code` at `web_base` (`/login/oauth/access_token`).
    pub async fn user_token(
        &self,
        web_base: &str,
        client_id: &str,
        secret: &Secret,
        code: &str,
        redirect_uri: &str,
    ) -> Result<Secret, ScmError> {
        let url = format!(
            "{}/login/oauth/access_token",
            web_base.trim_end_matches('/')
        );
        let request = self
            .plain()
            .post(url)
            .header("accept", "application/json")
            .json(&json!({
                "client_id": client_id, "client_secret": secret.expose(), "code": code,
                "redirect_uri": redirect_uri,
            }));
        // A refused code is a 200 with `error`, not an HTTP failure.
        let reply: Value = self.send(request, ScmCall::Read).await?;
        reply["access_token"]
            .as_str()
            .filter(|t| !t.is_empty())
            .map(|t| Secret::new(t.to_owned()))
            .ok_or_else(|| {
                let error = reply["error"].as_str().unwrap_or("no access token");
                ScmError::new(
                    ScmErrorKind::Unauthorized,
                    format!("sign-in refused: {error}"),
                )
            })
    }

    /// The login a user token belongs to.
    pub async fn user_login(&self, token: &Secret) -> Result<String, ScmError> {
        let v: Value = self.get("/user", token).await?;
        v["login"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| ScmError::new(ScmErrorKind::Transient, "a user without login"))
    }

    /// `(owner/name, role)` of every repository of this App's installations the user reaches.
    pub async fn user_repositories(
        &self,
        token: &Secret,
    ) -> Result<Vec<(String, String)>, ScmError> {
        // shortcut: the first 100 installations only; page them when one person needs more.
        let installations: Value = self.get("/user/installations?per_page=100", token).await?;
        let mut out = Vec::new();
        for installation in installations["installations"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let id = installation["id"]
                .as_u64()
                .ok_or_else(|| ScmError::new(ScmErrorKind::Transient, "installation without id"))?;
            for page in 1..=MAX_PAGES {
                let v: Value = self
                    .get(
                        &format!(
                            "/user/installations/{id}/repositories?per_page={PER_PAGE}&page={page}"
                        ),
                        token,
                    )
                    .await?;
                let repos = v["repositories"].as_array().cloned().unwrap_or_default();
                out.extend(repos.iter().filter_map(|r| {
                    let name = r["full_name"].as_str()?;
                    Some((name.to_owned(), role(&r["permissions"]).to_owned()))
                }));
                if repos.len() < PER_PAGE {
                    break;
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::role;

    #[test]
    fn the_highest_flag_names_the_role() {
        let all =
            json!({"admin": false, "maintain": true, "push": true, "triage": true, "pull": true});
        assert_eq!(role(&all), "maintain");
        assert_eq!(role(&json!({"pull": true, "triage": true})), "triage");
        assert_eq!(role(&json!({"push": true})), "write");
        assert_eq!(role(&json!({})), "none");
    }
}
