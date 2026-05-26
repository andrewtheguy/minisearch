use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use log::error;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),

    #[error("{0}")]
    NotFound(String),

    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl AppError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self::BadRequest(msg.into())
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }

}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            AppError::Internal(err) => {
                error!("internal error: {err:#}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_string(),
                )
            }
        };
        (status, message).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::response::IntoResponse;

    async fn response_parts(error: AppError) -> (StatusCode, String) {
        let response = error.into_response();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();

        (status, body)
    }

    #[tokio::test]
    async fn client_errors_return_status_and_message() {
        let cases = [
            (
                AppError::bad_request("bad query"),
                StatusCode::BAD_REQUEST,
                "bad query",
            ),
            (
                AppError::not_found("missing profile"),
                StatusCode::NOT_FOUND,
                "missing profile",
            ),
        ];

        for (error, expected_status, expected_body) in cases {
            let (status, body) = response_parts(error).await;

            assert_eq!(status, expected_status);
            assert_eq!(body, expected_body);
        }
    }

    #[tokio::test]
    async fn internal_errors_hide_error_chain_from_clients() {
        let (status, body) = response_parts(AppError::Internal(anyhow::anyhow!(
            "database password leaked"
        )))
        .await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body, "internal server error");
    }
}
