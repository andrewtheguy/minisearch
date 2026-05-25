use std::collections::HashSet;
use std::path::Path;

use anyhow::Context;
use log::{debug, info, warn};
use tantivy::collector::TopDocs;
use tantivy::query::TermQuery;
use tantivy::schema::{IndexRecordOption, Value};
use tantivy::{doc, Searcher, Term, TantivyDocument};

use crate::search;

const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "rst", "json", "jsonc", "jsonl", "ndjson", "csv", "tsv", "log",
    "xml", "html", "htm", "yml", "yaml", "toml", "ini", "conf", "cfg", "env", "css", "scss",
    "less", "js", "mjs", "cjs", "jsx", "ts", "tsx", "vue", "svelte", "py", "rb", "go", "rs",
    "java", "kt", "swift", "c", "h", "cc", "cpp", "hpp", "sh", "bash", "zsh", "fish", "ps1",
    "sql", "tf", "hcl", "gitignore", "editorconfig", "lock",
];

const TEXT_BASENAMES: &[&str] = &[
    "readme",
    "license",
    "licence",
    "copying",
    "authors",
    "changelog",
    "makefile",
    "dockerfile",
    "jenkinsfile",
    "procfile",
];

const TEXT_APP_TYPES: &[&str] = &[
    "application/json",
    "application/xml",
    "application/yaml",
    "application/x-yaml",
    "application/javascript",
    "application/ecmascript",
    "application/x-sh",
    "application/x-shellscript",
    "application/sql",
];

fn is_text_by_name(key: &str) -> bool {
    let path = Path::new(key);

    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext_lower = ext.to_ascii_lowercase();
        if TEXT_EXTENSIONS.iter().any(|&e| e == ext_lower) {
            return true;
        }
    }

    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        let name_lower = name.to_ascii_lowercase();
        if TEXT_BASENAMES.iter().any(|&b| b == name_lower) {
            return true;
        }
    }

    false
}

fn is_text_content_type(content_type: &str) -> bool {
    content_type.starts_with("text/") || TEXT_APP_TYPES.contains(&content_type)
}

fn lookup_last_modified(
    searcher: &Searcher,
    schema: &search::SearchSchema,
    key: &str,
) -> Option<String> {
    let term = Term::from_field_text(schema.key_raw, key);
    let query = TermQuery::new(term, IndexRecordOption::Basic);
    let top_docs = searcher.search(&query, &TopDocs::with_limit(1).order_by_score()).ok()?;
    let (_score, doc_address) = top_docs.first()?;
    let doc: TantivyDocument = searcher.doc(*doc_address).ok()?;
    doc.get_first(schema.last_modified)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

pub async fn run_indexer(config: &crate::config::AppConfig) -> anyhow::Result<()> {
    let s3_client = config.s3_client().await;

    let search_schema = search::build_schema();
    let search::IndexPathResult { path: index_path, bucket: bucket_name } =
        search::index_path(&config.tantivy_index_path, &config.aws_endpoint_url, &config.s3_bucket_name)?;
    let index = search::open_or_create_index(&index_path, &search_schema.schema)?;

    let lookup_searcher = index.reader().ok().map(|r| r.searcher());

    let mut writer = index
        .writer(50_000_000)
        .context("failed to create index writer")?;

    let mut indexed = 0usize;
    let mut indexed_filename_only = 0usize;
    let mut unchanged = 0usize;
    let mut removed = 0usize;
    let mut failed = 0usize;
    let mut seen_keys = HashSet::new();
    let mut continuation_token: Option<String> = None;

    loop {
        let mut req = s3_client.list_objects_v2().bucket(&bucket_name);
        if let Some(token) = &continuation_token {
            req = req.continuation_token(token);
        }
        let output = req.send().await.context("failed to list S3 objects")?;

        let contents = output.contents();
        for obj in contents {
            let key = match obj.key() {
                Some(k) => k.to_string(),
                None => continue,
            };
            let size = obj.size().unwrap_or(0) as u64;
            let last_modified = obj
                .last_modified()
                .map(|dt| {
                    dt.fmt(aws_sdk_s3::primitives::DateTimeFormat::DateTime)
                        .unwrap_or_default()
                })
                .unwrap_or_default();

            seen_keys.insert(key.clone());

            if let Some(searcher) = &lookup_searcher
                && let Some(indexed_modified) = lookup_last_modified(searcher, &search_schema, &key)
                && indexed_modified == last_modified
            {
                unchanged += 1;
                continue;
            }

            let is_text = if is_text_by_name(&key) {
                true
            } else {
                match s3_client
                    .head_object()
                    .bucket(&bucket_name)
                    .key(&key)
                    .send()
                    .await
                {
                    Ok(head) => head
                        .content_type()
                        .is_some_and(is_text_content_type),
                    Err(_) => false,
                }
            };

            if !is_text {
                debug!("indexing (filename only): {key}");
                writer.delete_term(Term::from_field_text(search_schema.key_raw, &key));
                writer.add_document(doc!(
                    search_schema.key => key.as_str(),
                    search_schema.key_raw => key.as_str(),
                    search_schema.size => size,
                    search_schema.last_modified => last_modified.as_str(),
                ))?;
                indexed_filename_only += 1;
            } else {
                debug!("indexing: {key}");

                let body = match s3_client
                    .get_object()
                    .bucket(&bucket_name)
                    .key(&key)
                    .send()
                    .await
                {
                    Ok(output) => match output.body.collect().await {
                        Ok(bytes) => bytes.into_bytes(),
                        Err(e) => {
                            warn!("failed to read body for {key}: {e}");
                            failed += 1;
                            continue;
                        }
                    },
                    Err(e) => {
                        warn!("failed to download {key}: {e}");
                        failed += 1;
                        continue;
                    }
                };

                writer.delete_term(Term::from_field_text(search_schema.key_raw, &key));
                let text = String::from_utf8(body.into()).ok();

                if let Some(text) = &text {
                    writer.add_document(doc!(
                        search_schema.key => key.as_str(),
                        search_schema.key_raw => key.as_str(),
                        search_schema.content => text.as_str(),
                        search_schema.size => size,
                        search_schema.last_modified => last_modified.as_str(),
                    ))?;
                    indexed += 1;
                } else {
                    debug!("non-utf8, indexing filename only: {key}");
                    writer.add_document(doc!(
                        search_schema.key => key.as_str(),
                        search_schema.key_raw => key.as_str(),
                        search_schema.size => size,
                        search_schema.last_modified => last_modified.as_str(),
                    ))?;
                    indexed_filename_only += 1;
                }
            }

            let total_indexed = indexed + indexed_filename_only;
            if total_indexed.is_multiple_of(100) {
                writer.commit()?;
                info!("progress: indexed {total_indexed} files...");
            }
        }

        if output.is_truncated() == Some(true) {
            continuation_token = output.next_continuation_token().map(|s| s.to_string());
        } else {
            break;
        }
    }

    if let Some(searcher) = &lookup_searcher {
        for segment_reader in searcher.segment_readers() {
            let Ok(store_reader) = segment_reader.get_store_reader(0) else {
                continue;
            };
            for doc_id in 0..segment_reader.max_doc() {
                if segment_reader.is_deleted(doc_id) {
                    continue;
                }
                let Ok(doc) = store_reader.get::<TantivyDocument>(doc_id) else {
                    continue;
                };
                let key = doc
                    .get_first(search_schema.key_raw)
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !key.is_empty() && !seen_keys.contains(key) {
                    writer.delete_term(Term::from_field_text(search_schema.key_raw, key));
                    removed += 1;
                }
            }
        }
    }

    writer.commit()?;
    let total = indexed + indexed_filename_only + unchanged + failed;
    info!("done: {total} files total");
    info!("  indexed (full):       {indexed}");
    info!("  indexed (filename):   {indexed_filename_only}");
    info!("  unchanged:            {unchanged}");
    info!("  removed:              {removed}");
    info!("  failed:               {failed}");
    Ok(())
}
