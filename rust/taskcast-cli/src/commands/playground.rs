use clap::Args;

#[derive(Args, Debug)]
pub struct PlaygroundArgs {
    /// Port to listen on
    #[arg(short, long, default_value = "5173")]
    pub port: u16,
}

#[derive(rust_embed::RustEmbed)]
#[folder = "../../packages/playground/dist"]
pub struct PlaygroundAssets;

/// Serve embedded playground files under /_playground/
pub fn playground_routes() -> axum::Router {
    axum::Router::new()
        .route("/{*path}", axum::routing::get(serve_playground_asset))
        .route("/", axum::routing::get(serve_playground_index))
}

async fn serve_playground_index() -> axum::response::Response {
    serve_playground_file("index.html")
}

async fn serve_playground_asset(
    axum::extract::Path(path): axum::extract::Path<String>,
) -> axum::response::Response {
    serve_playground_file(&path)
}

pub fn serve_playground_file(path: &str) -> axum::response::Response {
    use axum::http::{header, StatusCode};
    use axum::response::IntoResponse;

    match PlaygroundAssets::get(path) {
        Some(asset) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, mime.as_ref())],
                asset.data,
            )
                .into_response()
        }
        None => {
            // SPA fallback: serve index.html for unknown paths
            match PlaygroundAssets::get("index.html") {
                Some(index) => (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "text/html")],
                    index.data,
                )
                    .into_response(),
                None => StatusCode::NOT_FOUND.into_response(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    #[test]
    fn known_file_returns_correct_mime_type() {
        let response = serve_playground_file("index.html");
        let response = response.into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get("content-type")
            .expect("should have content-type header")
            .to_str()
            .unwrap();
        assert!(
            content_type.contains("text/html"),
            "Expected text/html, got: {content_type}"
        );
    }

    #[test]
    fn unknown_path_falls_back_to_index_html() {
        // SPA fallback: any unknown path should serve index.html
        let response = serve_playground_file("some/unknown/route");
        let response = response.into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get("content-type")
            .expect("should have content-type header")
            .to_str()
            .unwrap();
        assert!(
            content_type.contains("text/html"),
            "SPA fallback should serve text/html, got: {content_type}"
        );
    }

    #[test]
    fn css_asset_returns_correct_mime_type() {
        // Find a CSS asset from the embedded files
        let css_file = PlaygroundAssets::iter().find(|f| f.ends_with(".css"));
        if let Some(css_path) = css_file {
            let response = serve_playground_file(&css_path);
            let response = response.into_response();
            assert_eq!(response.status(), StatusCode::OK);
            let content_type = response
                .headers()
                .get("content-type")
                .expect("should have content-type header")
                .to_str()
                .unwrap();
            assert!(
                content_type.contains("css"),
                "Expected CSS mime type, got: {content_type}"
            );
        }
    }

    #[test]
    fn js_asset_returns_correct_mime_type() {
        // Find a JS asset from the embedded files
        let js_file = PlaygroundAssets::iter().find(|f| f.ends_with(".js"));
        if let Some(js_path) = js_file {
            let response = serve_playground_file(&js_path);
            let response = response.into_response();
            assert_eq!(response.status(), StatusCode::OK);
            let content_type = response
                .headers()
                .get("content-type")
                .expect("should have content-type header")
                .to_str()
                .unwrap();
            assert!(
                content_type.contains("javascript"),
                "Expected javascript mime type, got: {content_type}"
            );
        }
    }

    #[test]
    fn rust_embed_has_index_html() {
        // Verify that the embedded assets include index.html
        assert!(
            PlaygroundAssets::get("index.html").is_some(),
            "PlaygroundAssets should embed index.html"
        );
    }

    #[test]
    fn rust_embed_has_at_least_one_asset() {
        // Verify that rust-embed actually embeds files
        let count = PlaygroundAssets::iter().count();
        assert!(
            count > 0,
            "PlaygroundAssets should contain at least one embedded file"
        );
    }

    #[test]
    fn known_file_body_is_not_empty() {
        let _response = serve_playground_file("index.html");
        // We can't easily read the body in a sync test, but we can verify
        // the asset data from rust-embed directly
        let asset = PlaygroundAssets::get("index.html").unwrap();
        assert!(
            !asset.data.is_empty(),
            "index.html asset should have non-empty content"
        );
    }
}

pub async fn run(args: PlaygroundArgs) -> Result<(), Box<dyn std::error::Error>> {
    let PlaygroundArgs { port } = args;

    let app = axum::Router::new()
        .nest("/_playground", playground_routes())
        .route(
            "/",
            axum::routing::get(|| async { axum::response::Redirect::temporary("/_playground/") }),
        );
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    println!("[taskcast] Playground UI at http://localhost:{port}/_playground/");
    println!("[taskcast] Use \"External\" mode in the UI to connect to a remote server.");
    axum::serve(listener, app).await?;

    Ok(())
}
