//! Provider endpoint URLs, parsed rather than prefix-matched: userinfo such as
//! `http://localhost@elsewhere` would otherwise pass for the host it only names (spec §22.3).

use url::{Host, Url};

/// An `https://` URL, or `http://` to a loopback host; never with userinfo.
pub(super) fn callable(text: &str) -> Option<Url> {
    let url = Url::parse(text).ok()?;
    let ok = url.username().is_empty()
        && url.password().is_none()
        && (url.scheme() == "https" || url.scheme() == "http" && loopback(&url));
    ok.then_some(url)
}

pub(super) fn is_loopback_http(text: &str) -> bool {
    callable(text).is_some_and(|url| url.scheme() == "http")
}

fn loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain == "localhost",
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// Whether `url` is the endpoint `base` or a path under it, on the same origin.
pub(super) fn inside(base: &str, url: &str) -> bool {
    let (Some(base), Some(url)) = (callable(base), callable(url)) else {
        return false;
    };
    let prefix = base.path().trim_end_matches('/');
    base.origin() == url.origin()
        && url
            .path()
            .strip_prefix(prefix)
            .is_some_and(|tail| tail.is_empty() || tail.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn userinfo_does_not_pass_for_a_loopback_host() {
        assert!(!is_loopback_http(
            "http://localhost:8080@outside.invalid/api/v1"
        ));
        assert!(callable("https://user:pw@openrouter.ai/api/v1").is_none());
        assert!(is_loopback_http("http://127.0.0.1:8080/v1"));
        assert!(is_loopback_http("http://[::1]/v1"));
        assert!(callable("http://example.com/v1").is_none());
    }

    #[test]
    fn inside_compares_origin_and_path_segments() {
        let base = "https://openrouter.ai/api/v1/";
        assert!(inside(
            base,
            "https://openrouter.ai/api/v1/chat/completions"
        ));
        assert!(inside(base, "https://openrouter.ai/api/v1?x=1"));
        assert!(!inside(base, "https://openrouter.ai/api/v10"));
        assert!(!inside(base, "https://openrouter.ai:444/api/v1"));
        assert!(!inside(base, "https://openrouter.ai/api/v1/../../admin"));
        assert!(!inside(
            "http://localhost:8080/api/v1",
            "http://localhost:8080@outside.invalid/api/v1/chat/completions"
        ));
    }
}
