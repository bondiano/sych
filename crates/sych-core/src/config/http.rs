//! The HTTP surface of the server: bind, API clients with roles and token
//! references, and who may read metrics. A non-loopback bind needs auth and a TLS proxy;
//! corporate mode always needs auth.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{EnvName, Mode, Problems, endpoint};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Http {
    pub bind: String,
    pub public_base_url: Option<String>,
    /// TLS terminates in front of Sych; required for any non-loopback bind.
    pub behind_tls_proxy: bool,
    /// Client addresses allowed to read `/metrics`.
    pub metrics_allow: Vec<IpAddr>,
    /// Callers of `/v1`; a request without a known token is refused.
    pub api_clients: Vec<ApiClient>,
    /// The web UI's build (`web/dist`), served at `/ui`; none when unset. Left out of the
    /// effective config when unset, so the config hash of a server without a UI stays as it was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_dir: Option<PathBuf>,
    /// Sych servers whose dashboards the UI links between, this one included, such as
    /// production and a sandbox. Left out of the effective config when empty, as `ui_dir`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub instances: Vec<Instance>,
}

/// A Sych server the dashboard switches to: its own `/ui` under `url`, with its own sign-in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Instance {
    pub name: String,
    /// The server's `http.public_base_url`.
    pub url: String,
}

impl Default for Http {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8080".into(),
            public_base_url: None,
            behind_tls_proxy: false,
            metrics_allow: vec![
                IpAddr::from([127, 0, 0, 1]),
                IpAddr::from([0, 0, 0, 0, 0, 0, 0, 1]),
            ],
            api_clients: Vec::new(),
            ui_dir: None,
            instances: Vec::new(),
        }
    }
}

/// What an API client or a signed-in person may do, cumulative.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ApiRole {
    /// Read runs, results and publications.
    Reader,
    /// Also give feedback on findings.
    Triage,
    /// Also request reviews, reruns and cancellations.
    Writer,
    /// Also record waivers.
    Maintainer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiClient {
    /// The actor recorded in audit, feedback and idempotency scope.
    pub name: String,
    pub token_env: EnvName,
    #[serde(default = "local")]
    pub tenant_id: String,
    /// `connection/owner/name` the client may act on; `*` for every bound repository.
    pub repositories: Vec<String>,
    pub role: ApiRole,
}

fn local() -> String {
    "local".into()
}

impl ApiClient {
    pub fn allows(&self, connection: &str, full_name: &str) -> bool {
        self.repositories
            .iter()
            .any(|r| r == "*" || r.eq_ignore_ascii_case(&format!("{connection}/{full_name}")))
    }
}

impl Http {
    pub(super) fn validate(&self, mode: Mode, p: &mut Problems<'_>) {
        let bind = self.bind.parse::<SocketAddr>();
        p.check(
            bind.is_ok(),
            "http.bind",
            "must be an IP:port socket address",
        );
        let loopback = bind.is_ok_and(|b| b.ip().is_loopback());
        p.check(
            loopback || (self.behind_tls_proxy && !self.api_clients.is_empty()),
            "http.bind",
            "a non-loopback bind needs api_clients and behind_tls_proxy: true",
        );
        p.check(
            mode == Mode::Personal || !self.api_clients.is_empty(),
            "http.api_clients",
            "corporate mode needs authenticated API clients",
        );
        for (i, c) in self.api_clients.iter().enumerate() {
            p.check(
                self.api_clients
                    .iter()
                    .filter(|o| o.name == c.name || o.token_env == c.token_env)
                    .count()
                    == 1,
                format!("http.api_clients[{i}]"),
                "name and token_env must be unique",
            );
            p.check(
                !c.repositories.is_empty(),
                format!("http.api_clients[{i}].repositories"),
                "list `connection/owner/name` or `*`",
            );
        }
        self.validate_instances(p);
    }

    fn validate_instances(&self, p: &mut Problems<'_>) {
        if self.instances.is_empty() {
            return;
        }

        for (i, instance) in self.instances.iter().enumerate() {
            p.check(
                !instance.name.trim().is_empty(),
                format!("http.instances[{i}].name"),
                "must not be empty",
            );
            p.check(
                endpoint::callable(&instance.url).is_some(),
                format!("http.instances[{i}].url"),
                "must be an https:// URL, or http:// to a loopback host, without userinfo",
            );
            p.check(
                self.instances
                    .iter()
                    .filter(|o| o.name == instance.name || same_base_url(&o.url, &instance.url))
                    .count()
                    == 1,
                format!("http.instances[{i}]"),
                "name and url must be unique",
            );
        }
        p.check(
            self.public_base_url
                .as_deref()
                .is_some_and(|own| self.instances.iter().any(|i| same_base_url(&i.url, own))),
            "http.instances",
            "must list this server's public_base_url",
        );
    }

    pub(super) fn env_refs(&self) -> Vec<(String, &EnvName)> {
        self.api_clients
            .iter()
            .enumerate()
            .map(|(i, c)| (format!("http.api_clients[{i}].token_env"), &c.token_env))
            .collect()
    }
}

/// Two base URLs of one server, whatever trailing slash either has.
pub fn same_base_url(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

#[cfg(test)]
mod tests {
    use crate::config::ServerConfig;

    fn problems(text: &str) -> Vec<String> {
        match ServerConfig::parse("sych.yaml", text) {
            Ok(_) => Vec::new(),
            Err(errors) => errors.iter().map(ToString::to_string).collect(),
        }
    }

    const CLIENT: &str = "\n  api_clients:\n    - name: ci\n      token_env: CI_TOKEN\n      role: writer\n      repositories: [\"*\"]";

    #[test]
    fn exposed_or_corporate_servers_need_auth_and_a_tls_proxy() {
        assert!(
            problems("schema_version: 1\nmode: personal\n").is_empty(),
            "loopback default"
        );
        let open = problems("schema_version: 1\nmode: personal\nhttp:\n  bind: \"0.0.0.0:8080\"\n");
        assert!(
            open.iter().any(|p| p.contains("non-loopback bind")),
            "{open:?}"
        );
        let no_proxy = problems(&format!(
            "schema_version: 1\nmode: personal\nhttp:\n  bind: \"0.0.0.0:8080\"{CLIENT}\n"
        ));
        assert!(
            no_proxy.iter().any(|p| p.contains("behind_tls_proxy")),
            "{no_proxy:?}"
        );
        let proxied = format!(
            "schema_version: 1\nmode: personal\nhttp:\n  bind: \"0.0.0.0:8080\"\n  behind_tls_proxy: true{CLIENT}\n"
        );
        assert!(problems(&proxied).is_empty(), "{:?}", problems(&proxied));
        let corporate = problems("schema_version: 1\nmode: corporate\n");
        assert!(
            corporate
                .iter()
                .any(|p| p.contains("corporate mode needs authenticated API clients")),
            "{corporate:?}"
        );
    }

    #[test]
    fn instances_are_callable_unique_and_include_this_server() {
        let with = |instances: &str| {
            problems(&format!(
                "schema_version: 1\nmode: personal\nhttp:\n  public_base_url: https://sych.example\n  instances:\n{instances}"
            ))
        };
        let both = "    - {name: production, url: \"https://sych.example/\"}\n    - {name: sandbox, url: \"https://sandbox.sych.example\"}\n";
        assert!(with(both).is_empty(), "{:?}", with(both));
        let elsewhere = with("    - {name: sandbox, url: \"https://sandbox.sych.example\"}\n");
        assert!(
            elsewhere.iter().any(|p| p.contains("public_base_url")),
            "{elsewhere:?}"
        );
        let plain = with(
            "    - {name: production, url: \"https://sych.example\"}\n    - {name: sandbox, url: \"http://sandbox.sych.example\"}\n",
        );
        assert!(
            plain.iter().any(|p| p.contains("instances[1].url")),
            "{plain:?}"
        );
        let twice = with(
            "    - {name: production, url: \"https://sych.example\"}\n    - {name: production, url: \"https://sandbox.sych.example\"}\n",
        );
        assert!(twice.iter().any(|p| p.contains("unique")), "{twice:?}");
    }
}
