use taskcast_cli::commands::playground::{playground_routes, PlaygroundArgs};

// ─── Helper ──────────────────────────────────────────────────────────────────

async fn find_available_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

// ─── playground::run() serves playground at /_playground/ ───────────────────

#[tokio::test]
async fn playground_run_serves_content() {
    let port = find_available_port().await;
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::playground::run(PlaygroundArgs { port }).await;
    });

    // Poll until server is ready (up to 2 seconds)
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if client
            .get(&format!("http://127.0.0.1:{port}/"))
            .send()
            .await
            .is_ok()
        {
            break;
        }
    }

    // Playground index should be served — Axum's nest strips the prefix,
    // so /_playground/index.html should hit the /{*path} handler with path="index.html"
    let res = reqwest::get(&format!("http://127.0.0.1:{port}/_playground/index.html"))
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "Expected 200 for /_playground/index.html, got {}",
        res.status()
    );

    let content_type = res
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        content_type.contains("text/html"),
        "Expected text/html content-type, got: {content_type}"
    );

    handle.abort();
}

// ─── playground::run() redirects / to /_playground/ ─────────────────────────

#[tokio::test]
async fn playground_run_redirects_root() {
    let port = find_available_port().await;
    let handle = tokio::spawn(async move {
        let _ = taskcast_cli::commands::playground::run(PlaygroundArgs { port }).await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Use a client that does not follow redirects
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let res = client
        .get(&format!("http://127.0.0.1:{port}/"))
        .send()
        .await
        .unwrap();

    assert_eq!(
        res.status().as_u16(),
        307,
        "Expected 307 Temporary Redirect for /, got {}",
        res.status()
    );

    let location = res
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(
        location, "/_playground/",
        "Expected redirect to /_playground/, got: {location}"
    );

    handle.abort();
}

// ─── playground_routes() via axum-test ──────────────────────────────────────

#[tokio::test]
async fn playground_routes_index_via_axum_test() {
    let app = playground_routes();
    let server = axum_test::TestServer::new(app);

    // GET / on the nested router should serve index.html
    let res = server.get("/").await;
    res.assert_status_ok();
    let content_type = res
        .headers()
        .get("content-type")
        .expect("should have content-type")
        .to_str()
        .unwrap();
    assert!(
        content_type.contains("text/html"),
        "Expected text/html, got: {content_type}"
    );
}

#[tokio::test]
async fn playground_routes_unknown_path_spa_fallback() {
    let app = playground_routes();
    let server = axum_test::TestServer::new(app);

    // Unknown path should serve SPA fallback (index.html)
    let res = server.get("/some/unknown/route").await;
    res.assert_status_ok();
    let content_type = res
        .headers()
        .get("content-type")
        .expect("should have content-type")
        .to_str()
        .unwrap();
    assert!(
        content_type.contains("text/html"),
        "SPA fallback should serve text/html, got: {content_type}"
    );
}

#[tokio::test]
async fn playground_routes_serves_asset_files() {
    let app = playground_routes();
    let server = axum_test::TestServer::new(app);

    // The index.html should be accessible via the catch-all route too
    let res = server.get("/index.html").await;
    res.assert_status_ok();
    let content_type = res
        .headers()
        .get("content-type")
        .expect("should have content-type")
        .to_str()
        .unwrap();
    assert!(
        content_type.contains("text/html"),
        "Expected text/html for index.html, got: {content_type}"
    );
}
