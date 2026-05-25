use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde::Serialize;
use tower_http::services::{ServeDir, ServeFile};

#[derive(Clone)]
struct AppState {
    s3_client: aws_sdk_s3::Client,
    bucket_name: String,
}

#[derive(Serialize)]
struct ListFilesResponse {
    files: Vec<S3Object>,
}

#[derive(Serialize)]
struct S3Object {
    key: String,
    size: i64,
    last_modified: String,
}

async fn list_files(
    State(state): State<AppState>,
) -> Result<Json<ListFilesResponse>, (StatusCode, String)> {
    let output = state
        .s3_client
        .list_objects_v2()
        .bucket(&state.bucket_name)
        .send()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to list S3 objects: {e}")))?;

    let files = output
        .contents()
        .iter()
        .map(|obj| S3Object {
            key: obj.key().unwrap_or("").to_string(),
            size: obj.size().unwrap_or(0),
            last_modified: obj
                .last_modified()
                .map(|dt| {
                    dt.fmt(aws_sdk_s3::primitives::DateTimeFormat::DateTime)
                        .unwrap_or_default()
                })
                .unwrap_or_default(),
        })
        .collect();

    Ok(Json(ListFilesResponse { files }))
}

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
        .route("/files", get(list_files))
        .with_state(state)
        .fallback_service(
            ServeDir::new("frontend/dist")
                .fallback(ServeFile::new("frontend/dist/index.html")),
        );

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("listening on http://localhost:3000");
    axum::serve(listener, app).await.unwrap();
}
