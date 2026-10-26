//! Default-deny secret paths and value redaction, applied before model egress and before
//! diagnostics are written (spec §22.3, INV-18). Filtering is a heuristic, not a guarantee.

use std::borrow::Cow;

use regex_automata::meta::Regex;

const MARK: &str = "[REDACTED]";
const SECRET_DIRS: &[&str] = &[".ssh", ".aws", ".gnupg", ".terraform"];
const SECRET_NAMES: &[&str] = &[
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    ".netrc",
    ".pgpass",
    ".npmrc",
    ".pypirc",
    ".git-credentials",
    "credentials",
    "credentials.json",
    "kubeconfig",
    "secrets.yaml",
    "secrets.yml",
    "secrets.json",
];
const SECRET_EXTENSIONS: &[&str] = &[
    "pem", "key", "p12", "pfx", "jks", "keystore", "kdbx", "tfstate",
];
/// Example env files are allowed by path; their content is still redacted.
const EXAMPLE_SUFFIXES: &[&str] = &["example", "sample", "template", "dist"];
const TOKEN_PREFIXES: &[&str] = &[
    "github_pat_",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "glpat-",
    "sk-",
    "xoxb-",
    "xoxp-",
    "AKIA",
    "AIza",
];
const MIN_TOKEN_TAIL: usize = 16;
const SECRET_KEYS: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "private_key",
    "access_key",
];
const MIN_ASSIGNED_SECRET: usize = 8;

/// Whether a repository path is denied by default: real env files, private keys, credentials
/// and infrastructure state.
pub fn is_secret_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let mut segments = lower.split('/');
    let name = segments.next_back().unwrap_or_default();
    if segments.any(|dir| SECRET_DIRS.contains(&dir)) || SECRET_NAMES.contains(&name) {
        return true;
    }

    if name == ".env" || name.starts_with(".env.") {
        return !EXAMPLE_SUFFIXES.iter().any(|s| name.ends_with(s));
    }

    name.contains(".tfstate.")
        || name
            .rsplit_once('.')
            .is_some_and(|(stem, ext)| !stem.is_empty() && SECRET_EXTENSIONS.contains(&ext))
}

fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Replaces private key blocks, well-known token formats and values assigned to secret-like
/// keys with the built-in rules. Returns the input unchanged when nothing matched.
pub fn redact(text: &str) -> Cow<'_, str> {
    BUILTIN.redact(text)
}

static BUILTIN: Redactor = Redactor {
    prefixes: Vec::new(),
    keys: Vec::new(),
    patterns: Vec::new(),
};

/// The built-in rules plus trusted additions (`redaction` in the server config); an addition
/// never replaces a built-in rule.
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    prefixes: Vec<String>,
    /// Lowercase, like the keys they are compared with.
    keys: Vec<String>,
    patterns: Vec<Regex>,
}

impl Redactor {
    /// Extra token prefixes, secret keys and regular expressions; the error names the entry.
    pub fn new(prefixes: &[String], keys: &[String], patterns: &[String]) -> Result<Self, String> {
        if let Some(i) = prefixes.iter().position(|p| p.trim().is_empty()) {
            return Err(format!("token_prefixes[{i}]: must not be empty"));
        }

        if let Some(i) = keys.iter().position(|k| k.trim().is_empty()) {
            return Err(format!("secret_keys[{i}]: must not be empty"));
        }

        let patterns = patterns
            .iter()
            .enumerate()
            .map(|(i, p)| match Regex::new(p) {
                Err(e) => Err(format!("patterns[{i}]: invalid regular expression: {e}")),
                Ok(re) if re.is_match("") => Err(format!("patterns[{i}]: matches empty text")),
                Ok(re) => Ok(re),
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            prefixes: prefixes.to_vec(),
            keys: keys.iter().map(|k| k.to_ascii_lowercase()).collect(),
            patterns,
        })
    }

    /// The built-in rules only.
    pub fn builtin() -> &'static Self {
        &BUILTIN
    }

    /// Returns the input unchanged when nothing matched.
    pub fn redact<'t>(&self, text: &'t str) -> Cow<'t, str> {
        let mut out = redact_private_keys(text);
        for re in &self.patterns {
            out = redact_matches(re, &out);
        }
        let tokens = redact_tokens(&out, &self.prefixes);
        let assigned = redact_assignments(&tokens, &self.keys);
        if assigned == text {
            Cow::Borrowed(text)
        } else {
            Cow::Owned(assigned)
        }
    }
}

fn redact_matches(re: &Regex, text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for m in re.find_iter(text) {
        out.push_str(&text[copied..m.start()]);
        out.push_str(MARK);
        copied = m.end();
    }
    out.push_str(&text[copied..]);
    out
}

fn redact_private_keys(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("-----BEGIN ") {
        let Some(header_len) = rest[start..].find("PRIVATE KEY-----") else {
            break;
        };
        out.push_str(&rest[..start]);
        let body = &rest[start + header_len..];
        // An unterminated block is redacted to the end: truncated keys are still secrets.
        let end = body
            .find("-----END ")
            .and_then(|e| body[e + 9..].find("-----").map(|t| e + 9 + t + 5))
            .unwrap_or(body.len());
        out.push_str(MARK);
        rest = &body[end..];
    }
    out.push_str(rest);
    out
}

fn redact_tokens(text: &str, extra: &[String]) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        let boundary = i == 0 || !is_token_byte(bytes[i - 1]);
        let token = boundary
            .then(|| {
                TOKEN_PREFIXES
                    .iter()
                    .copied()
                    .chain(extra.iter().map(String::as_str))
                    .find(|p| bytes[i..].starts_with(p.as_bytes()))
            })
            .flatten()
            .map(|p| {
                let tail = bytes[i + p.len()..]
                    .iter()
                    .take_while(|&&b| is_token_byte(b))
                    .count();
                (p.len(), tail)
            });
        match token {
            Some((prefix, tail)) if tail >= MIN_TOKEN_TAIL => {
                out.push_str(&text[copied..i]);
                out.push_str(MARK);
                i += prefix + tail;
                copied = i;
            }
            _ => i += 1,
        }
    }
    out.push_str(&text[copied..]);
    out
}

fn redact_assignments(text: &str, extra: &[String]) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        out.push_str(&redact_assignment(line, extra));
    }
    out
}

/// `key: value`, `key = "value"`, `"key": "value"` where the key names a secret.
fn redact_assignment<'l>(line: &'l str, extra: &[String]) -> Cow<'l, str> {
    let Some(sep) = line.find([':', '=']) else {
        return Cow::Borrowed(line);
    };
    let key = line[..sep]
        .trim()
        .trim_matches(['"', '\''])
        .to_ascii_lowercase();
    let key_is_secret = SECRET_KEYS
        .iter()
        .copied()
        .chain(extra.iter().map(String::as_str))
        .any(|k| key.contains(k))
        && !key.ends_with("_env");
    let value_start = sep + 1 + (line.len() - sep - 1 - line[sep + 1..].trim_start().len());
    let raw = &line[value_start..];
    let quoted = raw.starts_with(['"', '\'']);
    let value_len = raw[usize::from(quoted)..]
        .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | '}'))
        .unwrap_or(raw.len() - usize::from(quoted));
    if !key_is_secret || value_len < MIN_ASSIGNED_SECRET || raw.starts_with(MARK) {
        return Cow::Borrowed(line);
    }

    let from = value_start + usize::from(quoted);
    Cow::Owned(format!(
        "{}{MARK}{}",
        &line[..from],
        &line[from + value_len..]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denies_real_secret_paths_and_allows_examples() {
        for path in [
            ".env",
            "app/.env.local",
            "deploy/tls.key",
            "certs/server.PEM",
            "infra/terraform.tfstate",
            "infra/terraform.tfstate.backup",
            "home/.ssh/config",
            "id_ed25519",
            ".npmrc",
        ] {
            assert!(is_secret_path(path), "{path}");
        }
        for path in [
            ".env.example",
            "src/key.rs",
            "keys/README.md",
            "id_ed25519.pub",
            "src/token.ts",
            ".key",
        ] {
            assert!(!is_secret_path(path), "{path}");
        }
    }

    #[test]
    fn redacts_private_keys_tokens_and_assignments() {
        let text = "a\n-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE \
                    KEY-----\nb ghp_0123456789abcdefABCDEF here\napi_key: \"sk-or-v1-short\"\n\
                    password = hunter2hunter2\napi_key_env: SYCH_OPENROUTER_API_KEY\n";
        let red = redact(text);
        assert_eq!(
            red,
            "a\n[REDACTED]\nb [REDACTED] here\napi_key: \"[REDACTED]\"\npassword = \
             [REDACTED]\napi_key_env: SYCH_OPENROUTER_API_KEY\n"
        );
    }

    #[test]
    fn unterminated_private_key_is_redacted_to_the_end() {
        assert_eq!(
            redact("x -----BEGIN RSA PRIVATE KEY-----\nMIIE"),
            "x [REDACTED]"
        );
    }

    #[test]
    fn non_ascii_text_is_scanned_safely() {
        assert_eq!(
            redact("Сыч ключ ghp_0123456789abcdefABCDEF ёж"),
            "Сыч ключ [REDACTED] ёж"
        );
    }

    #[test]
    fn clean_text_is_borrowed() {
        let text = "token budget: 6000\nsk-short\nask-me-anything-please-now";
        assert!(matches!(redact(text), Cow::Borrowed(_)));
    }

    #[test]
    fn additions_extend_the_built_in_rules() {
        let own = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let r = Redactor::new(
            &own(&["acme_"]),
            &own(&["Signing_Seed"]),
            &own(&[r"ACME-\d{6}"]),
        )
        .expect("valid");
        let text = "a acme_0123456789abcdefXYZ\nsigning_seed = s33d-s33d-s33d\nid ACME-123456 \
                    x\nb ghp_0123456789abcdefABCDEF\n";
        assert_eq!(
            r.redact(text),
            "a [REDACTED]\nsigning_seed = [REDACTED]\nid [REDACTED] x\nb [REDACTED]\n"
        );
        assert!(
            redact(text).contains("ACME-123456"),
            "the defaults stay as they are"
        );

        for (prefixes, keys, patterns, problem) in [
            (
                own(&[" "]),
                vec![],
                vec![],
                "token_prefixes[0]: must not be empty",
            ),
            (
                vec![],
                own(&[""]),
                vec![],
                "secret_keys[0]: must not be empty",
            ),
            (vec![], vec![], own(&["ok", "("]), "patterns[1]: invalid"),
            (
                vec![],
                vec![],
                own(&["x*"]),
                "patterns[0]: matches empty text",
            ),
        ] {
            let err = Redactor::new(&prefixes, &keys, &patterns).expect_err(problem);
            assert!(err.starts_with(problem), "{err}");
        }
    }
}
