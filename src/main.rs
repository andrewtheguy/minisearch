mod assets;
mod handlers;
mod state;

use axum::{routing::get, Router};
use state::AppState;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let bucket_name = std::env::var("S3_BUCKET_NAME").expect("S3_BUCKET_NAME must be set");

    let aws_config = aws_config::load_from_env().await;
    let s3_client = aws_sdk_s3::Client::new(&aws_config);

    let state = AppState {
        s3_client,
        bucket_name,
    };

    let app = Router::new()
        .route("/api/health", get(|| async { "ok" }))
        .route("/files", get(handlers::list_files))
        .with_state(state)
        .fallback(assets::static_handler);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("listening on http://localhost:3000");
    axum::serve(listener, app).await.unwrap();
}
