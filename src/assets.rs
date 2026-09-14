//! The web UI. Release builds compile the frontend bundle into the binary
//! (`build.rs` stages it in `OUT_DIR` and sets `cfg(embed_frontend)`); dev builds
//! embed nothing and point at the Vite dev server instead.

use axum::response::Response;

#[cfg(embed_frontend)]
mod embedded {
    use axum::{
        http::{StatusCode, header},
        response::{Html, IntoResponse, Response},
    };
    use rust_embed::Embed;

    #[derive(Embed)]
    #[folder = "$OUT_DIR/frontend-dist"]
    struct StaticAssets;

    pub fn serve(uri: &axum::http::Uri) -> Response {
        let path = uri.path().trim_start_matches('/');
        if let Some(file) = StaticAssets::get(path) {
            let mime = new_mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref())], file.data).into_response()
        } else if let Some(index) = StaticAssets::get("index.html") {
            Html(index.data).into_response()
        } else {
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

#[cfg(embed_frontend)]
pub async fn static_handler(uri: axum::http::Uri) -> Response {
    embedded::serve(&uri)
}

#[cfg(not(embed_frontend))]
pub async fn static_handler() -> Response {
    use axum::{http::StatusCode, response::IntoResponse};
    (
        StatusCode::NOT_FOUND,
        "frontend is not embedded in this dev build: run `bun run dev` in frontend/ and open http://localhost:5173, or build with --release (or MINISEARCH_EMBED_FRONTEND=1)\n",
    )
        .into_response()
}
