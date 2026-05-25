use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
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

fn endpoint_index_host(endpoint_url: &str) -> anyhow::Result<String> {
    let url = url::Url::parse(endpoint_url).context("AWS_ENDPOINT_URL must be a valid URL")?;
    let host = url
        .host()
        .context("AWS_ENDPOINT_URL must include a host")?;
    match host {
        url::Host::Domain(host) => Ok(host.to_string()),
        url::Host::Ipv4(_) | url::Host::Ipv6(_) => {
            bail!("AWS_ENDPOINT_URL host must be a hostname, not an IP address")
        }
    }
}

pub struct IndexPathResult {
    pub path: PathBuf,
    pub bucket: String,
}

pub fn index_path(tantivy_index_path: &str, aws_endpoint_url: &str, s3_bucket_name: &str) -> anyhow::Result<IndexPathResult> {
    let base = PathBuf::from(tantivy_index_path);
    let host = endpoint_index_host(aws_endpoint_url)?;

    Ok(IndexPathResult {
        path: base.join(&host).join(s3_bucket_name),
        bucket: s3_bucket_name.to_string(),
    })
}

pub fn open_or_create_index(path: &Path, schema: &Schema) -> anyhow::Result<Index> {
    let index = if path.exists() {
        let index = Index::open_in_dir(path).context("failed to open existing index")?;
        if index.schema() != *schema {
            bail!(
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
    use super::endpoint_index_host;

    #[test]
    fn endpoint_index_host_uses_hostname_without_port() {
        assert_eq!(
            endpoint_index_host("https://minio.example.test:9000").unwrap(),
            "minio.example.test"
        );
    }

    #[test]
    fn endpoint_index_host_rejects_ipv4_addresses() {
        let err = endpoint_index_host("http://127.0.0.1:9000").unwrap_err();
        assert_eq!(
            err.to_string(),
            "AWS_ENDPOINT_URL host must be a hostname, not an IP address"
        );
    }

    #[test]
    fn endpoint_index_host_rejects_ipv6_addresses() {
        let err = endpoint_index_host("http://[::1]:9000").unwrap_err();
        assert_eq!(
            err.to_string(),
            "AWS_ENDPOINT_URL host must be a hostname, not an IP address"
        );
    }
}
