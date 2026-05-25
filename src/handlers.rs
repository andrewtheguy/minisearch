use axum::{extract::State, http::StatusCode, Json};
use serde::Serialize;

use crate::state::AppState;

#[derive(Serialize)]
pub struct ListFilesResponse {
    pub files: Vec<S3Object>,
}

#[derive(Serialize)]
pub struct S3Object {
    pub key: String,
    pub size: i64,
    pub last_modified: String,
}

pub async fn list_files(
    State(state): State<AppState>,
) -> Result<Json<ListFilesResponse>, (StatusCode, String)> {
    let output = state
        .s3_client
        .list_objects_v2()
        .bucket(&state.bucket_name)
        .send()
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to list S3 objects: {e}"),
            )
        })?;

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
