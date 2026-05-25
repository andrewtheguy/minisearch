use std::time::Duration;

use aws_sdk_s3::presigning::PresigningConfig;
use axum::{extract::Query, extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::Value;
use tantivy::snippet::SnippetGenerator;
use tantivy::TantivyDocument;

use crate::state::AppState;

#[derive(Deserialize)]
pub struct SearchParams {
    pub q: Option<String>,
}

#[derive(Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub count: usize,
    pub results: Vec<SearchResult>,
}

#[derive(Serialize)]
pub struct SearchResult {
    pub key: String,
    pub snippet_html: String,
    pub score: f32,
    pub size: u64,
    pub last_modified: String,
}

pub async fn search(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<SearchResponse>, (StatusCode, String)> {
    let query_str = params
        .q
        .filter(|q| !q.trim().is_empty())
        .ok_or((StatusCode::BAD_REQUEST, "missing or empty query parameter 'q'".into()))?;

    let schema = state
        .search_schema
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "search index not available — run `cargo run -- index` first".into()))?;

    let reader = state
        .search_reader
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "search index not available — run `cargo run -- index` first".into()))?;

    let searcher = reader.searcher();
    let query_parser = QueryParser::for_index(searcher.index(), vec![schema.key, schema.content]);
    let query = query_parser
        .parse_query(&query_str)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid query: {e}")))?;

    let top_docs = searcher
        .search(&query, &TopDocs::with_limit(20).order_by_score())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("search failed: {e}")))?;

    let snippet_generator = SnippetGenerator::create(&searcher, &query, schema.content)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("snippet generation failed: {e}")))?;

    let mut results = Vec::with_capacity(top_docs.len());
    for (score, doc_address) in &top_docs {
        let doc: TantivyDocument = searcher
            .doc(*doc_address)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to retrieve doc: {e}")))?;

        let key = doc
            .get_first(schema.key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let size = doc
            .get_first(schema.size)
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let last_modified = doc
            .get_first(schema.last_modified)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let snippet = snippet_generator.snippet_from_doc(&doc);
        let snippet_html = snippet.to_html();

        results.push(SearchResult {
            key,
            snippet_html,
            score: *score,
            size,
            last_modified,
        });
    }

    Ok(Json(SearchResponse {
        query: query_str,
        count: results.len(),
        results,
    }))
}

#[derive(Deserialize)]
pub struct PresignParams {
    pub key: Option<String>,
}

#[derive(Serialize)]
pub struct PresignResponse {
    pub url: String,
}

pub async fn presign(
    State(state): State<AppState>,
    Query(params): Query<PresignParams>,
) -> Result<Json<PresignResponse>, (StatusCode, String)> {
    let key = params
        .key
        .filter(|k| !k.trim().is_empty())
        .ok_or((StatusCode::BAD_REQUEST, "missing or empty query parameter 'key'".into()))?;

    let mime = mime_guess::from_path(&key).first_or_octet_stream();
    let content_type = if mime.type_() == mime_guess::mime::TEXT {
        format!("{mime}; charset=utf-8")
    } else {
        mime.to_string()
    };

    let presign_config = PresigningConfig::expires_in(Duration::from_secs(3600))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("presign config error: {e}")))?;

    let presigned = state
        .s3_client
        .get_object()
        .bucket(&state.bucket_name)
        .key(&key)
        .response_content_type(&content_type)
        .response_content_disposition("inline")
        .presigned(presign_config)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("presign failed: {e}")))?;

    Ok(Json(PresignResponse {
        url: presigned.uri().to_string(),
    }))
}
