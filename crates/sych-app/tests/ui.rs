//! The web UI's build served at `/ui`: pages fall back to `index.html`, a missing hashed asset
//! is a 404 rather than HTML, nothing outside the build directory is reachable, and every
//! answer carries the headers that keep the page to its own origin.

use std::fs;

use reqwest::StatusCode;
use reqwest::redirect::Policy;
use sych_app::server::ui;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn serve() -> (tempfile::TempDir, String) {
    let root = tempfile::tempdir().expect("tempdir");
    fs::write(root.path().join("secret.txt"), "SECRET").expect("secret");
    let dist = root.path().join("dist");
    fs::create_dir_all(dist.join("assets")).expect("dist");
    fs::write(dist.join("index.html"), "<html>sych ui</html>").expect("index");
    fs::write(dist.join("assets/app-1.js"), "console.log(1)").expect("asset");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("addr").to_string();
    let routes = ui::routes::<()>(&dist);
    tokio::spawn(async move { axum::serve(listener, routes).await });
    (root, address)
}

#[tokio::test(flavor = "current_thread")]
async fn pages_fall_back_to_the_app_and_assets_do_not() {
    let (_root, address) = serve().await;
    let client = reqwest::Client::builder()
        .redirect(Policy::none())
        .build()
        .expect("client");
    let get = |path: &str| client.get(format!("http://{address}{path}")).send();

    let root = get("/").await.expect("root");
    assert_eq!(root.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(root.headers()["location"], "/ui/");
    for page in ["/ui/", "/ui/changes/7"] {
        let reply = get(page).await.expect("page");
        assert_eq!(reply.status(), StatusCode::OK, "{page}");
        let headers = reply.headers().clone();
        assert!(
            headers["content-security-policy"]
                .to_str()
                .is_ok_and(|csp| csp.contains("default-src 'self'")
                    && csp.contains("frame-ancestors 'none'"))
        );
        assert_eq!(headers["x-content-type-options"], "nosniff");
        assert_eq!(headers["cache-control"], "no-cache", "{page}");
        assert_eq!(reply.text().await.expect("body"), "<html>sych ui</html>");
    }

    let asset = get("/ui/assets/app-1.js").await.expect("asset");
    assert_eq!(asset.status(), StatusCode::OK);
    assert!(
        asset.headers()["cache-control"]
            .to_str()
            .is_ok_and(|c| c.contains("immutable"))
    );
    let missing = get("/ui/assets/app-2.js").await.expect("missing");
    assert_eq!(
        missing.status(),
        StatusCode::NOT_FOUND,
        "never HTML cached as a script"
    );
    assert_eq!(missing.headers()["cache-control"], "no-cache");
}

#[tokio::test(flavor = "current_thread")]
async fn nothing_outside_the_build_is_served() {
    let (_root, address) = serve().await;
    for path in [
        "/ui/../secret.txt",
        "/ui/%2e%2e/secret.txt",
        "/ui/assets/../../secret.txt",
    ] {
        // A raw request: an HTTP client would normalize the dots away.
        let mut stream = TcpStream::connect(&address).await.expect("connect");
        let request = format!("GET {path} HTTP/1.1\r\nHost: sych\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await.expect("write");
        let mut reply = String::new();
        stream.read_to_string(&mut reply).await.expect("read");
        assert!(!reply.contains("SECRET"), "{path}: {reply}");
    }
}
