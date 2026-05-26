use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use tantivy::IndexReader;

use crate::search::SearchSchema;

#[derive(serde::Deserialize)]
pub struct IndexState {
    pub last_indexed: String,
}

pub fn read_last_indexed(work_dir: &Path) -> String {
    let state_path = work_dir.join("state.json");
    match std::fs::read_to_string(&state_path) {
        Ok(s) => match serde_json::from_str::<IndexState>(&s) {
            Ok(state) => state.last_indexed,
            Err(e) => format!("state.json parse error: {e}"),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "not indexed yet".to_string(),
        Err(e) => format!("failed to read state.json: {e}"),
    }
}

#[derive(Clone)]
pub struct AppState {
    pub profiles: Vec<ProfileEntry>,
}

impl AppState {
    pub fn get_profile(&self, name: &str) -> Option<&ProfileEntry> {
        self.profiles.iter().find(|p| p.name == name)
    }
}

#[derive(Clone)]
pub struct ProfileEntry {
    pub name: String,
    pub description: String,
    pub state: ProfileState,
}

#[derive(Clone)]
pub struct ProfileState {
    pub s3_client: aws_sdk_s3::Client,
    pub bucket_name: String,
    pub work_dir: PathBuf,
    pub search: Arc<RwLock<SearchState>>,
}

#[derive(Clone)]
pub struct SearchState {
    pub reader: IndexReader,
    pub schema: SearchSchema,
}
