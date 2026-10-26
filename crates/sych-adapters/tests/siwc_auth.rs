//! Sign in with `ChatGPT` building blocks (roadmap M1.10): base64url, PKCE, host id, ID token
//! claims and the loopback callback checks.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sych_adapters::siwc::auth::{
    AUTH_BASE, AuthError, Pkce, REDIRECT_PORT, Registration, authorize_url, b64url, b64url_decode,
    callback, check_id_token, new_host_id, redirect_uri,
};

#[test]
fn base64url_round_trips_without_padding() {
    assert_eq!(b64url(b"f"), "Zg");
    assert_eq!(b64url(b"fo"), "Zm8");
    assert_eq!(b64url(b"foo"), "Zm9v");
    assert_eq!(b64url(&[0xfb, 0xff]), "-_8");
    for data in [&b""[..], b"a", b"ab", b"abc", &[0, 255, 128, 7]] {
        assert_eq!(b64url_decode(&b64url(data)).as_deref(), Some(data));
    }
}

#[test]
fn pkce_challenge_is_the_s256_of_the_verifier() {
    // RFC 7636 appendix B.
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    assert_eq!(
        b64url(&Sha256::digest(verifier.as_bytes())),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
    let host = new_host_id().expect("host id");
    assert!(host.starts_with("urn:uuid:") && host.len() == 45, "{host}");
    assert_eq!(&host[23..24], "4");
}

fn token(claims: &Value) -> String {
    format!("e30.{}.sig", b64url(claims.to_string().as_bytes()))
}

#[test]
fn id_token_must_match_issuer_audience_expiry_and_nonce() {
    let good = json!({
        "iss": "https://auth.openai.com", "sub": "user-1", "aud": "oaiapp_1",
        "exp": 2_000, "nonce": "n1", "email": "a@example.com"
    });
    let ok = check_id_token(
        &token(&good),
        "https://auth.openai.com",
        "oaiapp_1",
        Some("n1"),
        1_000,
    );
    assert_eq!(ok, Ok(("user-1".into(), Some("a@example.com".into()))));
    for (field, value) in [
        ("iss", json!("https://evil.example")),
        ("aud", json!("other")),
        ("exp", json!(999)),
        ("nonce", json!("n2")),
    ] {
        let mut bad = good.clone();
        bad[field] = value;
        let r = check_id_token(
            &token(&bad),
            "https://auth.openai.com",
            "oaiapp_1",
            Some("n1"),
            1_000,
        );
        assert!(r.is_err(), "{field} accepted");
    }
}

#[test]
fn callback_state_and_issued_client_are_checked() {
    let pkce = Pkce {
        verifier: "v".into(),
        challenge: "c".into(),
        state: "s1".into(),
        nonce: "n".into(),
    };
    let new = Registration {
        client_id: None,
        ext_agent_host_id: "urn:uuid:x".into(),
        id_token_hint: None,
    };
    let cb = callback(
        "/auth/callback?code=c1&state=s1&client_id=oaiapp_9",
        &pkce,
        &new,
    );
    assert_eq!(cb.expect("ok").client_id.as_deref(), Some("oaiapp_9"));
    assert!(
        callback(
            "/auth/callback?code=c1&state=s2&client_id=oaiapp_9",
            &pkce,
            &new
        )
        .is_err()
    );
    assert!(callback("/auth/callback?code=c1&state=s1", &pkce, &new).is_err());
    let denied = callback("/auth/callback?error=access_denied&state=s1", &pkce, &new);
    assert!(matches!(denied, Err(AuthError::NotGranted(_))));
    let saved = Registration {
        client_id: Some("oaiapp_9".into()),
        ..new
    };
    assert!(
        callback(
            "/auth/callback?code=c&state=s1&client_id=oaiapp_8",
            &pkce,
            &saved
        )
        .is_err()
    );
    assert!(callback("/auth/callback?code=c&state=s1", &pkce, &saved).is_ok());
    let url = authorize_url(AUTH_BASE, &saved, &pkce, &redirect_uri(REDIRECT_PORT));
    assert!(url.contains("client_id=oaiapp_9") && !url.contains("agent_name_hint"));
    assert!(
        url.contains("chatgpt.tokens.use.direct") && url.contains("code_challenge_method=S256")
    );
}
