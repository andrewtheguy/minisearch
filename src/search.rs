use std::path::Path;

use anyhow::Context;
use tantivy::schema::{Field, Schema, TextFieldIndexing, TextOptions, STORED, STRING};
use tantivy::Index;

const JIEBA_TOKENIZER_NAME: &str = "jieba";

#[derive(Clone)]
pub struct SearchSchema {
    pub schema: Schema,
    pub key: Field,
    pub key_raw: Field,
    pub content: Field,
    pub extension: Field,
    pub size: Field,
    pub last_modified: Field,
}

pub fn build_schema() -> SearchSchema {
    let mut builder = Schema::builder();
    let text_options = TextOptions::default()
        .set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(JIEBA_TOKENIZER_NAME)
                .set_index_option(tantivy::schema::IndexRecordOption::WithFreqsAndPositions),
        )
        .set_stored();
    let key = builder.add_text_field("key", text_options.clone());
    let key_raw = builder.add_text_field("key_raw", STRING | STORED);
    let content = builder.add_text_field("content", text_options);
    let extension = builder.add_text_field("extension", STRING | STORED);
    let size = builder.add_u64_field("size", STORED);
    let last_modified = builder.add_text_field("last_modified", STRING | STORED);
    SearchSchema {
        schema: builder.build(),
        key,
        key_raw,
        content,
        extension,
        size,
        last_modified,
    }
}

pub fn register_tokenizers(index: &Index) {
    index
        .tokenizers()
        .register(JIEBA_TOKENIZER_NAME, tantivy_jieba::JiebaTokenizer::new());
}

pub fn open_or_create_index(path: &Path, schema: &Schema) -> anyhow::Result<Index> {
    let index = if path.exists() {
        let index = Index::open_in_dir(path).context("failed to open existing index")?;
        if index.schema() != *schema {
            anyhow::bail!(
                "index schema mismatch at {path} — delete the index directory and re-run",
                path = path.display(),
            );
        }
        index
    } else {
        std::fs::create_dir_all(path).context("failed to create index directory")?;
        Index::create_in_dir(path, schema.clone()).context("failed to create new index")?
    };
    register_tokenizers(&index);
    Ok(index)
}

pub fn open_index(path: &Path) -> Option<Index> {
    if path.exists() {
        let index = Index::open_in_dir(path).ok()?;
        register_tokenizers(&index);
        Some(index)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_index_returns_none_for_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        let missing_path = dir.path().join("missing");

        assert!(open_index(&missing_path).is_none());
    }

    #[test]
    fn open_or_create_index_reopens_existing_index() {
        let dir = tempfile::tempdir().unwrap();
        let index_path = dir.path().join("index");
        let schema = build_schema();

        let created = open_or_create_index(&index_path, &schema.schema).unwrap();
        let reopened = open_or_create_index(&index_path, &schema.schema).unwrap();

        assert_eq!(created.schema(), reopened.schema());
    }

    #[test]
    fn open_or_create_index_rejects_schema_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let index_path = dir.path().join("index");
        let schema = build_schema();
        open_or_create_index(&index_path, &schema.schema).unwrap();

        let mut builder = Schema::builder();
        builder.add_text_field("different", STRING | STORED);
        let mismatched_schema = builder.build();
        let err = open_or_create_index(&index_path, &mismatched_schema).unwrap_err();

        assert!(format!("{err:#}").contains("index schema mismatch"));
    }
}
