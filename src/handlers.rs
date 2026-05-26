use std::collections::BTreeSet;
use std::ops::Range;
use std::time::Duration;

use anyhow::Context;
use aws_sdk_s3::presigning::PresigningConfig;
use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use tantivy::collector::{Count, TopDocs};
use tantivy::query::{BooleanQuery, Occur, Query as TantivyQuery, QueryParser, RegexQuery, TermQuery};
use tantivy::schema::{Field, IndexRecordOption, Value};
use tantivy::tokenizer::{TextAnalyzer, TokenStream};
use tantivy::{TantivyDocument, Term};

use crate::error::AppError;
use crate::state::{AppState, ProfileState, SearchState};

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    #[default]
    Both,
    Filename,
    Content,
}

#[derive(Deserialize)]
pub struct SearchParams {
    pub q: Option<String>,
    pub page: Option<usize>,
    pub ext: Option<String>,
    pub mode: Option<SearchMode>,
    pub prefix: Option<String>,
}

#[derive(Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub count: usize,
    pub limit: usize,
    pub page: usize,
    pub total_pages: usize,
    pub results: Vec<SearchResult>,
}

#[derive(Serialize)]
pub struct SearchResult {
    pub key: String,
    pub snippet: Vec<SearchSnippetSegment>,
    pub score: f32,
    pub size: u64,
    pub last_modified: String,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
pub struct SearchSnippetSegment {
    pub text: String,
    pub highlighted: bool,
    pub start: usize,
    pub end: usize,
}

fn get_search(state: &ProfileState) -> SearchState {
    state.search.read().unwrap_or_else(|e| e.into_inner()).clone()
}

pub async fn redirect_to_profile(
    State(state): State<AppState>,
) -> axum::response::Redirect {
    let name = &state.profiles[0].name;
    axum::response::Redirect::temporary(&format!("/p/{name}/browse/"))
}

#[derive(Serialize)]
pub struct ProfileInfoResponse {
    pub name: String,
    pub description: String,
    pub last_indexed: String,
}

pub async fn profile_info(
    State(state): State<AppState>,
    Path(profile_name): Path<String>,
) -> Result<Json<ProfileInfoResponse>, AppError> {
    let profile = state
        .get_profile(&profile_name)
        .ok_or_else(|| AppError::not_found(format!("profile not found: {profile_name}")))?;

    let last_indexed = crate::state::read_last_indexed(&profile.state.work_dir);

    Ok(Json(ProfileInfoResponse {
        name: profile.name.clone(),
        description: profile.description.clone(),
        last_indexed,
    }))
}

pub async fn search(
    State(state): State<AppState>,
    Path(profile_name): Path<String>,
    Query(params): Query<SearchParams>,
) -> Result<Json<SearchResponse>, AppError> {
    let profile = state
        .get_profile(&profile_name)
        .ok_or_else(|| AppError::not_found(format!("profile not found: {profile_name}")))?;

    let query_str = params
        .q
        .filter(|q| !q.trim().is_empty())
        .ok_or_else(|| AppError::bad_request("missing or empty query parameter 'q'"))?;

    let search_state = get_search(&profile.state);
    let schema = &search_state.schema;
    let reader = &search_state.reader;

    let mode = params.mode.unwrap_or_default();
    let extensions: Vec<String> = params
        .ext
        .as_deref()
        .filter(|e| !e.trim().is_empty())
        .map(|e| {
            e.split(',')
                .map(|s| s.trim().to_ascii_lowercase())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let searcher = reader.searcher();
    let search_fields = match mode {
        SearchMode::Both => vec![schema.key, schema.content],
        SearchMode::Filename => vec![schema.key],
        SearchMode::Content => vec![schema.content],
    };
    let query_parser = QueryParser::for_index(searcher.index(), search_fields);
    let parsed_query = query_parser
        .parse_query(&query_str)
        .map_err(|e| AppError::bad_request(format!("invalid query: {e}")))?;

    let mut query: Box<dyn TantivyQuery> = if extensions.is_empty() {
        parsed_query
    } else {
        let ext_queries: Vec<Box<dyn TantivyQuery>> = extensions
            .iter()
            .map(|ext| {
                Box::new(TermQuery::new(
                    Term::from_field_text(schema.extension, ext),
                    IndexRecordOption::Basic,
                )) as Box<dyn TantivyQuery>
            })
            .collect();
        Box::new(BooleanQuery::new(vec![
            (Occur::Must, parsed_query),
            (Occur::Must, Box::new(BooleanQuery::union(ext_queries))),
        ]))
    };

    if let Some(pfx) = params.prefix.as_deref().filter(|p| !p.is_empty()) {
        let escaped = regex_syntax::escape(pfx);
        let prefix_query = RegexQuery::from_pattern(&format!("^{escaped}.*"), schema.key_raw)
            .map_err(|e| AppError::bad_request(format!("invalid prefix: {e}")))?;
        query = Box::new(BooleanQuery::new(vec![
            (Occur::Must, query),
            (Occur::Must, Box::new(prefix_query)),
        ]));
    }

    let page = params.page.unwrap_or(1).max(1);
    let offset = (page - 1) * PAGE_LIMIT;

    let (total_count, top_docs) = searcher
        .search(&query, &(Count, TopDocs::with_limit(PAGE_LIMIT).and_offset(offset).order_by_score()))
        .context("search failed")?;

    let snippet_terms = query_terms_for_field(&*query, schema.content);
    let snippet_tokenizer = searcher
        .index()
        .tokenizer_for_field(schema.content)
        .context("snippet tokenizer failed")?;

    let mut results = Vec::with_capacity(top_docs.len());
    for (score, doc_address) in &top_docs {
        let doc: TantivyDocument = searcher
            .doc(*doc_address)
            .context("failed to retrieve doc")?;

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

        let content = doc_text(&doc, schema.content);
        let snippet = render_snippet(&content, snippet_tokenizer.clone(), &snippet_terms);

        results.push(SearchResult {
            key,
            snippet,
            score: *score,
            size,
            last_modified,
        });
    }

    let total_pages = if total_count == 0 { 0 } else { total_count.div_ceil(PAGE_LIMIT) };

    Ok(Json(SearchResponse {
        query: query_str,
        count: total_count,
        limit: PAGE_LIMIT,
        page,
        total_pages,
        results,
    }))
}

const PAGE_LIMIT: usize = 10;
const MAX_SNIPPET_CHARS: usize = 150;

fn query_terms_for_field(query: &dyn TantivyQuery, field: Field) -> BTreeSet<String> {
    let mut terms = BTreeSet::new();
    query.query_terms(&mut |term, _| {
        if term.field() == field
            && let Some(term_str) = term.value().as_str()
        {
            terms.insert(term_str.to_lowercase());
        }
    });
    terms
}

fn doc_text(doc: &TantivyDocument, field: Field) -> String {
    let mut text = String::new();
    for value in doc.get_all(field) {
        if let Some(value_str) = value.as_str() {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(value_str);
        }
    }
    text
}

fn render_snippet(
    text: &str,
    mut tokenizer: TextAnalyzer,
    terms: &BTreeSet<String>,
) -> Vec<SearchSnippetSegment> {
    let ranges = highlight_ranges(text, &mut tokenizer, terms);
    let fragment = best_fragment(text, &ranges, MAX_SNIPPET_CHARS);
    let visible_ranges = ranges
        .iter()
        .filter_map(|range| {
            let start = range.start.max(fragment.start);
            let end = range.end.min(fragment.end);
            if start < end {
                Some(start - fragment.start..end - fragment.start)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    snippet_segments(&text[fragment], &visible_ranges)
}

fn highlight_ranges(
    text: &str,
    tokenizer: &mut TextAnalyzer,
    terms: &BTreeSet<String>,
) -> Vec<Range<usize>> {
    if terms.is_empty() {
        return Vec::new();
    }

    let mut ranges = Vec::new();
    let mut token_stream = tokenizer.token_stream(text);
    while let Some(token) = token_stream.next() {
        if !terms.contains(&token.text.to_lowercase()) {
            continue;
        }
        if token.offset_from >= token.offset_to || token.offset_to > text.len() {
            continue;
        }
        if !text.is_char_boundary(token.offset_from) || !text.is_char_boundary(token.offset_to) {
            continue;
        }
        ranges.push(token.offset_from..token.offset_to);
    }
    merge_ranges(ranges)
}

fn merge_ranges(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut() {
            if range.start <= last.end {
                last.end = last.end.max(range.end);
            } else {
                merged.push(range);
            }
        } else {
            merged.push(range);
        }
    }
    merged
}

fn best_fragment(text: &str, ranges: &[Range<usize>], max_chars: usize) -> Range<usize> {
    let char_offsets = char_offsets(text);
    let total_chars = char_offsets.len().saturating_sub(1);
    if total_chars <= max_chars {
        return 0..text.len();
    }
    if ranges.is_empty() {
        return 0..char_offsets[max_chars];
    }

    let mut best = 0..char_offsets[max_chars];
    let mut best_score = 0usize;
    for range in ranges {
        let start_char = byte_to_char_index(&char_offsets, range.start);
        let end_char = byte_to_char_index(&char_offsets, range.end);
        let hit_chars = end_char.saturating_sub(start_char).max(1);
        let context_chars = max_chars.saturating_sub(hit_chars) / 2;
        let mut fragment_start_char = start_char.saturating_sub(context_chars);
        let mut fragment_end_char = (fragment_start_char + max_chars).min(total_chars);
        if fragment_end_char < end_char {
            fragment_end_char = end_char.min(total_chars);
            fragment_start_char = fragment_end_char.saturating_sub(max_chars);
        }

        let candidate = char_offsets[fragment_start_char]..char_offsets[fragment_end_char];
        let score = ranges
            .iter()
            .filter(|highlight| {
                highlight.start < candidate.end && highlight.end > candidate.start
            })
            .count();
        if score > best_score || (score == best_score && candidate.start < best.start) {
            best = candidate;
            best_score = score;
        }
    }
    best
}

fn char_offsets(text: &str) -> Vec<usize> {
    text.char_indices()
        .map(|(offset, _)| offset)
        .chain(std::iter::once(text.len()))
        .collect()
}

fn byte_to_char_index(char_offsets: &[usize], byte_offset: usize) -> usize {
    match char_offsets.binary_search(&byte_offset) {
        Ok(index) => index,
        Err(index) => index.saturating_sub(1),
    }
}

fn snippet_segments(fragment: &str, ranges: &[Range<usize>]) -> Vec<SearchSnippetSegment> {
    let mut segments = Vec::new();
    let mut start_from = 0usize;
    for range in ranges {
        if start_from < range.start {
            segments.push(SearchSnippetSegment {
                text: fragment[start_from..range.start].to_string(),
                highlighted: false,
                start: start_from,
                end: range.start,
            });
        }
        segments.push(SearchSnippetSegment {
            text: fragment[range.clone()].to_string(),
            highlighted: true,
            start: range.start,
            end: range.end,
        });
        start_from = range.end;
    }
    if start_from < fragment.len() {
        segments.push(SearchSnippetSegment {
            text: fragment[start_from..].to_string(),
            highlighted: false,
            start: start_from,
            end: fragment.len(),
        });
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jieba_terms(text: &str) -> BTreeSet<String> {
        let mut terms = BTreeSet::new();
        let mut tokenizer = TextAnalyzer::from(tantivy_jieba::JiebaTokenizer::new());
        let mut token_stream = tokenizer.token_stream(text);
        while let Some(token) = token_stream.next() {
            terms.insert(token.text.to_lowercase());
        }
        terms
    }

    #[test]
    fn render_snippet_handles_chinese_search_terms() {
        let terms = jieba_terms("朱古力");
        let snippet = render_snippet(
            "甜品朱古力蛋糕 & <menu>",
            TextAnalyzer::from(tantivy_jieba::JiebaTokenizer::new()),
            &terms,
        );

        assert_eq!(
            snippet,
            vec![
                SearchSnippetSegment {
                    text: "甜品".to_string(),
                    highlighted: false,
                    start: 0,
                    end: 6,
                },
                SearchSnippetSegment {
                    text: "朱古力".to_string(),
                    highlighted: true,
                    start: 6,
                    end: 15,
                },
                SearchSnippetSegment {
                    text: "蛋糕 & <menu>".to_string(),
                    highlighted: false,
                    start: 15,
                    end: 30,
                },
            ],
        );
    }

    #[test]
    fn render_snippet_ignores_highlights_outside_selected_fragment() {
        let terms = BTreeSet::from(["alpha".to_string(), "omega".to_string()]);
        let snippet = render_snippet(
            &format!("alpha {} omega omega", "middle ".repeat(200)),
            TextAnalyzer::from(tantivy::tokenizer::SimpleTokenizer::default()),
            &terms,
        );
        let highlighted = snippet
            .iter()
            .filter(|segment| segment.highlighted)
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>();

        assert_eq!(highlighted, vec!["omega", "omega"]);
    }

    #[test]
    fn m4a_mime_type_is_audio_mp4() {
        let mime = new_mime_guess::from_path("test.m4a").first_or_octet_stream();
        assert_eq!(mime, "audio/mp4");
    }
}

#[derive(Deserialize)]
pub struct PresignParams {
    pub key: Option<String>,
}

pub async fn presign(
    State(state): State<AppState>,
    Path(profile_name): Path<String>,
    Query(params): Query<PresignParams>,
) -> Result<axum::response::Redirect, AppError> {
    let profile = state
        .get_profile(&profile_name)
        .ok_or_else(|| AppError::not_found(format!("profile not found: {profile_name}")))?;

    let key = params
        .key
        .filter(|k| !k.trim().is_empty())
        .ok_or_else(|| AppError::bad_request("missing or empty query parameter 'key'"))?;

    let mime = new_mime_guess::from_path(&key).first_or_octet_stream();
    let content_type = if mime.type_() == "text" {
        format!("{mime}; charset=utf-8")
    } else {
        mime.to_string()
    };

    let presign_config = PresigningConfig::expires_in(Duration::from_secs(3600))
        .context("presign config error")?;

    let presigned = profile
        .state
        .s3_client
        .get_object()
        .bucket(&profile.state.bucket_name)
        .key(&key)
        .response_content_type(&content_type)
        .response_content_disposition("inline")
        .presigned(presign_config)
        .await
        .context("presign failed")?;

    Ok(axum::response::Redirect::temporary(presigned.uri()))
}

#[derive(Deserialize)]
pub struct BrowseParams {
    pub prefix: Option<String>,
    pub continuation_token: Option<String>,
}

#[derive(Serialize)]
pub struct BrowseResponse {
    pub prefix: String,
    pub folders: Vec<BrowseFolder>,
    pub files: Vec<BrowseFile>,
    pub is_truncated: bool,
    pub next_continuation_token: Option<String>,
}

#[derive(Serialize)]
pub struct BrowseFolder {
    pub key: String,
    pub name: String,
}

#[derive(Serialize)]
pub struct BrowseFile {
    pub key: String,
    pub name: String,
    pub size: u64,
    pub last_modified: String,
}

pub async fn browse(
    State(state): State<AppState>,
    Path(profile_name): Path<String>,
    Query(params): Query<BrowseParams>,
) -> Result<Json<BrowseResponse>, AppError> {
    let profile = state
        .get_profile(&profile_name)
        .ok_or_else(|| AppError::not_found(format!("profile not found: {profile_name}")))?;

    let prefix = params.prefix.unwrap_or_default();

    let mut req = profile
        .state
        .s3_client
        .list_objects_v2()
        .bucket(&profile.state.bucket_name)
        .delimiter("/")
        .prefix(&prefix);

    if let Some(token) = &params.continuation_token {
        req = req.continuation_token(token);
    }

    let output = req.send().await.context("failed to list S3 objects")?;

    let folders: Vec<BrowseFolder> = output
        .common_prefixes()
        .iter()
        .filter_map(|cp| {
            let key = cp.prefix()?.to_string();
            let name = key.strip_prefix(&prefix).unwrap_or(&key).to_string();
            Some(BrowseFolder { key, name })
        })
        .collect();

    let files: Vec<BrowseFile> = output
        .contents()
        .iter()
        .filter_map(|obj| {
            let key = obj.key()?.to_string();
            if key == prefix {
                return None;
            }
            let name = key.strip_prefix(&prefix).unwrap_or(&key).to_string();
            let size = obj.size().unwrap_or(0) as u64;
            let last_modified = obj
                .last_modified()
                .map(|dt| {
                    dt.fmt(aws_sdk_s3::primitives::DateTimeFormat::DateTime)
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            Some(BrowseFile { key, name, size, last_modified })
        })
        .collect();

    let is_truncated = output.is_truncated().unwrap_or(false);
    let next_continuation_token = if is_truncated {
        output.next_continuation_token().map(|s| s.to_string())
    } else {
        None
    };

    Ok(Json(BrowseResponse {
        prefix,
        folders,
        files,
        is_truncated,
        next_continuation_token,
    }))
}
