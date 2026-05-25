use std::collections::{HashMap, HashSet};
use std::path::Path;

use tantivy::schema::Value;
use tantivy::{doc, Term, TantivyDocument};

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
    content_type.starts_with("text/") || TEXT_APP_TYPES.iter().any(|&t| content_type == t)
}

fn load_existing_index(index: &tantivy::Index, schema: &search::SearchSchema) -> HashMap<String, String> {
    let reader = match index.reader() {
        Ok(r) => r,
        Err(_) => return HashMap::new(),
    };
    let searcher = reader.searcher();
    let mut existing = HashMap::new();

    for segment_reader in searcher.segment_readers() {
        let store_reader = match segment_reader.get_store_reader(0) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for doc_id in 0..segment_reader.max_doc() {
            if segment_reader.is_deleted(doc_id) {
                continue;
            }
            let doc: TantivyDocument = match store_reader.get(doc_id) {
                Ok(d) => d,
                Err(_) => continue,
            };
            let key = doc
                .get_first(schema.key)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let last_modified = doc
                .get_first(schema.last_modified)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !key.is_empty() {
                existing.insert(key, last_modified);
            }
        }
    }

    existing
}

pub async fn run_indexer() {
    let bucket_name = std::env::var("S3_BUCKET_NAME").expect("S3_BUCKET_NAME must be set");
    let aws_config = aws_config::load_from_env().await;
    let s3_client = aws_sdk_s3::Client::new(&aws_config);

    let search_schema = search::build_schema();
    let index_path = search::index_path();
    let index = search::open_or_create_index(&index_path, &search_schema.schema)
        .expect("failed to open/create index");

    let existing = load_existing_index(&index, &search_schema);
    println!("existing index: {} documents", existing.len());

    let mut writer = index.writer(50_000_000).expect("failed to create index writer");

    let mut indexed = 0usize;
    let mut unchanged = 0usize;
    let mut removed = 0usize;
    let mut skipped_non_text = 0usize;
    let mut skipped_non_utf8 = 0usize;
    let mut failed = 0usize;
    let mut seen_keys = HashSet::new();
    let mut continuation_token: Option<String> = None;

    loop {
        let mut req = s3_client.list_objects_v2().bucket(&bucket_name);
        if let Some(token) = &continuation_token {
            req = req.continuation_token(token);
        }
        let output = req.send().await.expect("failed to list S3 objects");

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

            if let Some(indexed_modified) = existing.get(&key) {
                if *indexed_modified == last_modified {
                    unchanged += 1;
                    continue;
                }
            }

            if !is_text_by_name(&key) {
                match s3_client
                    .head_object()
                    .bucket(&bucket_name)
                    .key(&key)
                    .send()
                    .await
                {
                    Ok(head) => {
                        let is_text = head
                            .content_type()
                            .is_some_and(|ct| is_text_content_type(ct));
                        if !is_text {
                            skipped_non_text += 1;
                            continue;
                        }
                    }
                    Err(_) => {
                        skipped_non_text += 1;
                        continue;
                    }
                }
            }

            println!("indexing: {key}");

            writer.delete_term(Term::from_field_text(search_schema.key, &key));

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
                        eprintln!("warning: failed to read body for {key}: {e}");
                        failed += 1;
                        continue;
                    }
                },
                Err(e) => {
                    eprintln!("warning: failed to download {key}: {e}");
                    failed += 1;
                    continue;
                }
            };

            let text = match String::from_utf8(body.to_vec()) {
                Ok(t) => t,
                Err(_) => {
                    skipped_non_utf8 += 1;
                    continue;
                }
            };

            writer
                .add_document(doc!(
                    search_schema.key => key.as_str(),
                    search_schema.content => text.as_str(),
                    search_schema.size => size,
                    search_schema.last_modified => last_modified.as_str(),
                ))
                .unwrap();

            indexed += 1;
            if indexed % 100 == 0 {
                writer.commit().unwrap();
                println!("progress: indexed {indexed} files...");
            }
        }

        if output.is_truncated() == Some(true) {
            continuation_token = output.next_continuation_token().map(|s| s.to_string());
        } else {
            break;
        }
    }

    for key in existing.keys() {
        if !seen_keys.contains(key) {
            writer.delete_term(Term::from_field_text(search_schema.key, key));
            removed += 1;
        }
    }

    writer.commit().unwrap();
    let total = indexed + unchanged + skipped_non_text + skipped_non_utf8 + failed + removed;
    println!("\ndone: {total} files total");
    println!("  indexed:          {indexed}");
    println!("  unchanged:        {unchanged}");
    println!("  removed:          {removed}");
    println!("  skipped non-text: {skipped_non_text}");
    println!("  skipped non-utf8: {skipped_non_utf8}");
    println!("  failed:           {failed}");
}
