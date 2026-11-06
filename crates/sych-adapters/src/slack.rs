//! Slack request signing: `X-Slack-Signature` is `v0=` and the hex HMAC-SHA256 of
//! `v0:<timestamp>:<raw body>` under the app's signing secret.

use std::fmt::Write;

use ring::hmac;

use crate::github::webhook::hex;

/// A request older or newer than this is refused as a possible replay.
pub const MAX_SKEW_SECONDS: i64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SlackRejection {
    #[error("missing X-Slack-Request-Timestamp or X-Slack-Signature")]
    Missing,
    #[error("the request timestamp is more than five minutes away")]
    Stale,
    #[error("the signature does not match")]
    BadSignature,
}

fn base(timestamp: &str, body: &[u8]) -> Vec<u8> {
    [format!("v0:{timestamp}:").as_bytes(), body].concat()
}

/// Checks the signature and the timestamp against `now`; `ring` compares in constant time.
pub fn verify(
    secret: &[u8],
    timestamp: Option<&str>,
    signature: Option<&str>,
    body: &[u8],
    now: i64,
) -> Result<(), SlackRejection> {
    let (Some(timestamp), Some(signature)) = (timestamp, signature) else {
        return Err(SlackRejection::Missing);
    };
    let at: i64 = timestamp.parse().map_err(|_| SlackRejection::Stale)?;
    if (now - at).abs() > MAX_SKEW_SECONDS {
        return Err(SlackRejection::Stale);
    }

    let tag = signature
        .strip_prefix("v0=")
        .and_then(hex)
        .ok_or(SlackRejection::BadSignature)?;
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret);
    hmac::verify(&key, &base(timestamp, body), &tag).map_err(|_| SlackRejection::BadSignature)
}

/// The signature Slack would send; for tests.
pub fn sign(secret: &[u8], timestamp: &str, body: &[u8]) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret);
    let tag = hmac::sign(&key, &base(timestamp, body));
    tag.as_ref().iter().fold(String::from("v0="), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

#[cfg(test)]
mod tests {
    use super::{SlackRejection, sign, verify};

    #[test]
    fn only_a_fresh_request_signed_with_the_secret_passes() {
        let body = b"command=%2Fsych&text=help";
        let signed = sign(b"secret", "1000", body);
        assert_eq!(
            verify(b"secret", Some("1000"), Some(&signed), body, 1100),
            Ok(())
        );
        assert_eq!(
            verify(b"other", Some("1000"), Some(&signed), body, 1100),
            Err(SlackRejection::BadSignature)
        );
        assert_eq!(
            verify(b"secret", Some("1000"), Some(&signed), b"text=digest", 1100),
            Err(SlackRejection::BadSignature)
        );
        assert_eq!(
            verify(b"secret", Some("1000"), Some(&signed), body, 1301),
            Err(SlackRejection::Stale)
        );
        assert_eq!(
            verify(b"secret", None, Some(&signed), body, 1000),
            Err(SlackRejection::Missing)
        );
    }
}
