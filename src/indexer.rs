use std::collections::HashSet;
use std::path::Path;

use anyhow::Context;
use log::{debug, info, warn};
use tantivy::collector::TopDocs;
use tantivy::query::TermQuery;
use tantivy::schema::{IndexRecordOption, Value};
use tantivy::{doc, Searcher, Term, TantivyDocument};

use crate::backend::Backend;
use crate::search;

const MAX_CONTENT_INDEX_SIZE: u64 = 10 * 1024 * 1024;

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

    if let Some(ext) = extension_from_key(key)
        && TEXT_EXTENSIONS.contains(&ext.as_str())
    {
        return true;
    }

    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        let name_lower = name.to_ascii_lowercase();
        if TEXT_BASENAMES.iter().any(|&b| b == name_lower) {
            return true;
        }
    }

    false
}

fn extract_extension(key: &str) -> String {
    extension_from_key(key).unwrap_or_default()
}

fn extension_from_key(key: &str) -> Option<String> {
    Path::new(key)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .or_else(|| {
            let file_name = Path::new(key).file_name()?.to_str()?;
            let dotfile_ext = file_name.strip_prefix('.')?;
            if dotfile_ext.is_empty() || dotfile_ext.contains('.') {
                return None;
            }
            if TEXT_EXTENSIONS.contains(&dotfile_ext) {
                Some(dotfile_ext.to_string())
            } else {
                None
            }
        })
}

fn is_text_content_type(content_type: &str) -> bool {
    let content_type = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    content_type.starts_with("text/") || TEXT_APP_TYPES.contains(&content_type.as_str())
}

const BUCKET_ID_MARKER: &str = ".minisearch-bucketid";

fn check_bucket_id(
    state_exists: bool,
    local_id: Option<&str>,
    remote_id: Option<&str>,
) -> anyhow::Result<Option<String>> {
    match (state_exists, local_id, remote_id) {
        (false, _, None) | (true, None, None) => Ok(None),
        (false, _, Some(id)) | (true, None, Some(id)) => Ok(Some(id.to_string())),
        (true, Some(local), None) => {
            anyhow::bail!(
                "state.json has bucket_id '{local}' but bucket has no {BUCKET_ID_MARKER} marker"
            );
        }
        (true, Some(local), Some(remote)) => {
            if local == remote {
                Ok(Some(remote.to_string()))
            } else {
                anyhow::bail!(
                    "bucket ID mismatch: state.json has '{local}' but bucket marker has '{remote}'"
                );
            }
        }
    }
}

async fn validate_bucket_id(
    backend: &Backend,
    work_dir: &Path,
) -> anyhow::Result<Option<String>> {
    let state = crate::state::read_state(work_dir).await;
    let state_exists = state.is_some();
    let local_id = state.as_ref().and_then(|s| s.bucket_id.as_deref());

    let remote_id = backend
        .get_marker_content(BUCKET_ID_MARKER)
        .await?;

    let result = check_bucket_id(state_exists, local_id, remote_id.as_deref())?;
    if let Some(id) = &result {
        info!("bucket ID verified: {id}");
    }
    Ok(result)
}

fn validate_backend_consistency(
    state: Option<&crate::state::IndexState>,
    config_backend: &str,
) -> anyhow::Result<()> {
    if let Some(state) = state
        && let Some(state_backend) = &state.backend
        && state_backend != config_backend
    {
        anyhow::bail!(
            "backend mismatch: state.json has '{state_backend}' but config has '{config_backend}' \
             — delete the work directory and re-index"
        );
    }
    Ok(())
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

pub async fn run_indexer(
    profile: &crate::config::ProfileConfig,
    backend: &Backend,
    work_dir: &Path,
) -> anyhow::Result<()> {
    info!("indexing profile: {}", profile.name);

    let state = crate::state::read_state(work_dir).await;
    validate_backend_consistency(state.as_ref(), backend.backend_name())?;

    let bucket_id = validate_bucket_id(backend, work_dir).await?;

    let search_schema = search::build_schema();
    let index_path = work_dir.join(crate::config::INDEX_DIR);
    let index = search::open_or_create_index(&index_path, &search_schema.schema).await?;

    if state.is_none() {
        write_state(work_dir, backend.backend_name(), &bucket_id, None).await?;
        info!("created initial state.json");
    }

    let lookup_searcher = index.reader().ok().map(|r| r.searcher());

    let mut writer = index
        .writer(50_000_000)
        .context("failed to create index writer")?;

    let mut indexed = 0usize;
    let mut indexed_filename_only = 0usize;
    let mut unchanged = 0usize;
    let mut removed = 0usize;
    let mut failed = 0usize;
    let mut skipped_large = 0usize;
    let mut seen_keys = HashSet::new();
    let mut failed_keys = HashSet::new();

    let objects = backend.list_all_objects().await?;

    for obj in &objects {
        let key = &obj.key;
        let size = obj.size;
        let last_modified = &obj.last_modified;

        seen_keys.insert(key.clone());

        if let Some(searcher) = &lookup_searcher
            && let Some(indexed_modified) = lookup_last_modified(searcher, &search_schema, key)
            && indexed_modified == *last_modified
        {
            unchanged += 1;
            continue;
        }

        let ext = extract_extension(key);

        let is_text = if is_text_by_name(key) {
            true
        } else if let Some(ct) = &obj.content_type {
            is_text_content_type(ct)
        } else {
            match backend.head_content_type(key).await {
                Ok(Some(ct)) => is_text_content_type(&ct),
                _ => false,
            }
        };

        if !is_text || size > MAX_CONTENT_INDEX_SIZE {
            if is_text {
                debug!("skipping content (file too large: {size} bytes): {key}");
                skipped_large += 1;
            } else {
                debug!("indexing (filename only): {key}");
            }
            writer.delete_term(Term::from_field_text(search_schema.key_raw, key));
            writer.add_document(doc!(
                search_schema.key => key.as_str(),
                search_schema.key_raw => key.as_str(),
                search_schema.extension => ext.as_str(),
                search_schema.size => size,
                search_schema.last_modified => last_modified.as_str(),
            ))?;
            indexed_filename_only += 1;
        } else {
            debug!("indexing: {key}");

            let body = match backend.get_object_body(key).await {
                Ok(bytes) => bytes,
                Err(e) => {
                    warn!("failed to download {key}: {e}");
                    failed_keys.insert(key.clone());
                    failed += 1;
                    continue;
                }
            };

            writer.delete_term(Term::from_field_text(search_schema.key_raw, key));
            let text = String::from_utf8(body).ok();

            if let Some(text) = &text {
                writer.add_document(doc!(
                    search_schema.key => key.as_str(),
                    search_schema.key_raw => key.as_str(),
                    search_schema.content => text.as_str(),
                    search_schema.extension => ext.as_str(),
                    search_schema.size => size,
                    search_schema.last_modified => last_modified.as_str(),
                ))?;
                indexed += 1;
            } else {
                debug!("non-utf8, indexing filename only: {key}");
                writer.add_document(doc!(
                    search_schema.key => key.as_str(),
                    search_schema.key_raw => key.as_str(),
                    search_schema.extension => ext.as_str(),
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

    if let Some(searcher) = &lookup_searcher {
        removed = remove_deleted_keys(searcher, &search_schema, &seen_keys, &failed_keys, &mut writer);
    }

    writer.commit()?;
    let total = indexed + indexed_filename_only + unchanged + failed + skipped_large;
    info!("done: {total} files total");
    info!("  indexed (full):       {indexed}");
    info!("  indexed (filename):   {indexed_filename_only}");
    info!("  skipped (too large):  {skipped_large}");
    info!("  unchanged:            {unchanged}");
    info!("  removed:              {removed}");
    info!("  failed:               {failed}");

    let now = chrono::Utc::now().to_rfc3339();
    write_state(work_dir, backend.backend_name(), &bucket_id, Some(&now)).await?;

    Ok(())
}

async fn write_state(
    work_dir: &Path,
    backend_name: &str,
    bucket_id: &Option<String>,
    last_indexed: Option<&str>,
) -> anyhow::Result<()> {
    let mut state = serde_json::json!({
        "backend": backend_name,
    });
    if let Some(ts) = last_indexed {
        state["last_indexed"] = serde_json::json!(ts);
    }
    if let Some(id) = bucket_id {
        state["bucket_id"] = serde_json::json!(id);
    }
    tokio::fs::create_dir_all(work_dir).await?;
    tokio::fs::write(work_dir.join("state.json"), serde_json::to_string_pretty(&state)?).await?;
    Ok(())
}

fn remove_deleted_keys(
    searcher: &Searcher,
    schema: &search::SearchSchema,
    seen_keys: &HashSet<String>,
    failed_keys: &HashSet<String>,
    writer: &mut tantivy::IndexWriter,
) -> usize {
    let mut removed = 0;
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
                .get_first(schema.key_raw)
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !key.is_empty() && !seen_keys.contains(key) && !failed_keys.contains(key) {
                writer.delete_term(Term::from_field_text(schema.key_raw, key));
                removed += 1;
            }
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_text_files_by_extension_basename_and_dotfile() {
        for key in ["notes.MD", "src/main.rs", "docs/README", ".gitignore", ".env"] {
            assert!(is_text_by_name(key), "{key} should be detected as text");
        }

        for key in ["image.png", "archive.tar.gz", ".DS_Store"] {
            assert!(!is_text_by_name(key), "{key} should not be detected as text");
        }
    }

    #[test]
    fn extracts_lowercase_extensions_including_known_dotfiles() {
        assert_eq!(extract_extension("docs/Notes.MD"), "md");
        assert_eq!(extract_extension("Cargo.lock"), "lock");
        assert_eq!(extract_extension(".gitignore"), "gitignore");
        assert_eq!(extract_extension(".env"), "env");
        assert_eq!(extract_extension("README"), "");
    }

    #[test]
    fn detects_text_content_types_by_essence() {
        for content_type in [
            "text/plain; charset=utf-8",
            "Application/JSON",
            "application/xml; charset=UTF-8",
            " application/sql ",
        ] {
            assert!(
                is_text_content_type(content_type),
                "{content_type} should be detected as text"
            );
        }

        assert!(!is_text_content_type("application/octet-stream"));
    }

    async fn setup_index(keys: &[&str]) -> (tempfile::TempDir, tantivy::Index, search::SearchSchema) {
        let dir = tempfile::tempdir().unwrap();
        let index_path = dir.path().join("index");
        let schema = search::build_schema();
        let index = search::open_or_create_index(&index_path, &schema.schema)
            .await
            .unwrap();
        let mut writer = index.writer(15_000_000).unwrap();
        for key in keys {
            writer
                .add_document(doc!(
                    schema.key => *key,
                    schema.key_raw => *key,
                    schema.size => 0u64,
                    schema.last_modified => "2025-01-01T00:00:00Z",
                ))
                .unwrap();
        }
        writer.commit().unwrap();
        (dir, index, schema)
    }

    fn indexed_keys(index: &tantivy::Index, schema: &search::SearchSchema) -> HashSet<String> {
        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        let mut keys = HashSet::new();
        for segment_reader in searcher.segment_readers() {
            let store_reader = segment_reader.get_store_reader(0).unwrap();
            for doc_id in 0..segment_reader.max_doc() {
                if segment_reader.is_deleted(doc_id) {
                    continue;
                }
                let doc = store_reader.get::<TantivyDocument>(doc_id).unwrap();
                if let Some(k) = doc.get_first(schema.key_raw).and_then(|v| v.as_str()) {
                    keys.insert(k.to_string());
                }
            }
        }
        keys
    }

    #[tokio::test]
    async fn removes_keys_not_seen_in_bucket() {
        let (_dir, index, schema) = setup_index(&["a.txt", "b.txt", "c.txt"]).await;
        let reader = index.reader().unwrap();
        let searcher = reader.searcher();

        let seen = HashSet::from(["a.txt".to_string()]);
        let failed = HashSet::new();
        let mut writer = index.writer(15_000_000).unwrap();
        let removed = remove_deleted_keys(&searcher, &schema, &seen, &failed, &mut writer);
        writer.commit().unwrap();
        drop(searcher);
        drop(reader);

        assert_eq!(removed, 2);
        let remaining = indexed_keys(&index, &schema);
        assert_eq!(remaining, HashSet::from(["a.txt".to_string()]));
    }

    #[tokio::test]
    async fn preserves_failed_keys() {
        let (_dir, index, schema) = setup_index(&["a.txt", "b.txt", "c.txt"]).await;
        let reader = index.reader().unwrap();
        let searcher = reader.searcher();

        let seen = HashSet::from(["a.txt".to_string()]);
        let failed = HashSet::from(["b.txt".to_string()]);
        let mut writer = index.writer(15_000_000).unwrap();
        let removed = remove_deleted_keys(&searcher, &schema, &seen, &failed, &mut writer);
        writer.commit().unwrap();
        drop(searcher);
        drop(reader);

        assert_eq!(removed, 1);
        let remaining = indexed_keys(&index, &schema);
        assert_eq!(
            remaining,
            HashSet::from(["a.txt".to_string(), "b.txt".to_string()])
        );
    }

    #[tokio::test]
    async fn no_removal_when_all_seen() {
        let (_dir, index, schema) = setup_index(&["a.txt", "b.txt"]).await;
        let reader = index.reader().unwrap();
        let searcher = reader.searcher();

        let seen = HashSet::from(["a.txt".to_string(), "b.txt".to_string()]);
        let failed = HashSet::new();
        let mut writer = index.writer(15_000_000).unwrap();
        let removed = remove_deleted_keys(&searcher, &schema, &seen, &failed, &mut writer);
        writer.commit().unwrap();
        drop(searcher);
        drop(reader);

        assert_eq!(removed, 0);
        let remaining = indexed_keys(&index, &schema);
        assert_eq!(
            remaining,
            HashSet::from(["a.txt".to_string(), "b.txt".to_string()])
        );
    }

    #[test]
    fn bucket_id_no_state_no_marker() {
        assert!(check_bucket_id(false, None, None).unwrap().is_none());
    }

    #[test]
    fn bucket_id_no_state_with_marker() {
        let result = check_bucket_id(false, None, Some("abc123")).unwrap();
        assert_eq!(result.as_deref(), Some("abc123"));
    }

    #[test]
    fn bucket_id_state_no_local_no_marker() {
        assert!(check_bucket_id(true, None, None).unwrap().is_none());
    }

    #[test]
    fn bucket_id_state_no_local_with_marker() {
        let result = check_bucket_id(true, None, Some("abc123")).unwrap();
        assert_eq!(result.as_deref(), Some("abc123"));
    }

    #[test]
    fn bucket_id_state_has_local_no_marker() {
        let err = check_bucket_id(true, Some("abc123"), None).unwrap_err();
        assert!(err.to_string().contains("has no .minisearch-bucketid marker"));
    }

    #[test]
    fn bucket_id_matching() {
        let result = check_bucket_id(true, Some("abc123"), Some("abc123")).unwrap();
        assert_eq!(result.as_deref(), Some("abc123"));
    }

    #[test]
    fn bucket_id_mismatch() {
        let err = check_bucket_id(true, Some("abc123"), Some("xyz789")).unwrap_err();
        assert!(err.to_string().contains("mismatch"));
    }

    #[tokio::test]
    async fn all_failed_none_removed() {
        let (_dir, index, schema) = setup_index(&["a.txt", "b.txt"]).await;
        let reader = index.reader().unwrap();
        let searcher = reader.searcher();

        let seen = HashSet::new();
        let failed = HashSet::from(["a.txt".to_string(), "b.txt".to_string()]);
        let mut writer = index.writer(15_000_000).unwrap();
        let removed = remove_deleted_keys(&searcher, &schema, &seen, &failed, &mut writer);
        writer.commit().unwrap();
        drop(searcher);
        drop(reader);

        assert_eq!(removed, 0);
        let remaining = indexed_keys(&index, &schema);
        assert_eq!(
            remaining,
            HashSet::from(["a.txt".to_string(), "b.txt".to_string()])
        );
    }

    #[test]
    fn backend_consistency_ok_when_matching() {
        let state = crate::state::IndexState {
            last_indexed: Some("2025-01-01T00:00:00Z".to_string()),
            bucket_id: None,
            backend: Some("s3".to_string()),
        };
        validate_backend_consistency(Some(&state), "s3").unwrap();
    }

    #[test]
    fn backend_consistency_ok_when_no_state() {
        validate_backend_consistency(None, "webdav").unwrap();
    }

    #[test]
    fn backend_consistency_ok_when_state_has_no_backend() {
        let state = crate::state::IndexState {
            last_indexed: Some("2025-01-01T00:00:00Z".to_string()),
            bucket_id: None,
            backend: None,
        };
        validate_backend_consistency(Some(&state), "webdav").unwrap();
    }

    #[test]
    fn backend_consistency_fails_on_mismatch() {
        let state = crate::state::IndexState {
            last_indexed: Some("2025-01-01T00:00:00Z".to_string()),
            bucket_id: None,
            backend: Some("s3".to_string()),
        };
        let err = validate_backend_consistency(Some(&state), "webdav").unwrap_err();
        assert!(err.to_string().contains("backend mismatch"));
    }
}
