use std::path::{Path, PathBuf};

use tantivy::schema::{Field, Schema, TextFieldIndexing, TextOptions, STORED, STRING};
use tantivy::Index;

const JIEBA_TOKENIZER_NAME: &str = "jieba";

#[derive(Clone)]
pub struct SearchSchema {
    pub schema: Schema,
    pub key: Field,
    pub content: Field,
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
    let content = builder.add_text_field("content", text_options);
    let size = builder.add_u64_field("size", STORED);
    let last_modified = builder.add_text_field("last_modified", STRING | STORED);
    SearchSchema {
        schema: builder.build(),
        key,
        content,
        size,
        last_modified,
    }
}

pub fn register_tokenizers(index: &Index) {
    index
        .tokenizers()
        .register(JIEBA_TOKENIZER_NAME, tantivy_jieba::JiebaTokenizer::new());
}

pub fn index_path() -> PathBuf {
    let base = std::env::var("TANTIVY_INDEX_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./tantivy_index"));

    let host = std::env::var("AWS_ENDPOINT_URL")
        .ok()
        .and_then(|u| url::Url::parse(&u).ok())
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_else(|| "default".to_string());

    let bucket = std::env::var("S3_BUCKET_NAME").unwrap_or_else(|_| "default".to_string());

    base.join(host).join(bucket)
}

pub fn open_or_create_index(path: &Path, schema: &Schema) -> tantivy::Result<Index> {
    let index = if path.exists() {
        Index::open_in_dir(path)?
    } else {
        std::fs::create_dir_all(path)?;
        Index::create_in_dir(path, schema.clone())?
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
