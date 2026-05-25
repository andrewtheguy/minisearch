use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use tantivy::IndexReader;

use crate::search::SearchSchema;

#[derive(Clone)]
pub struct AppState {
    pub s3_client: aws_sdk_s3::Client,
    pub bucket_name: String,
    pub index_path: PathBuf,
    pub search: Arc<RwLock<Option<SearchState>>>,
}

#[derive(Clone)]
pub struct SearchState {
    pub reader: IndexReader,
    pub schema: SearchSchema,
}
