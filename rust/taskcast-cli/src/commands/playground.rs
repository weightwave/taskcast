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
