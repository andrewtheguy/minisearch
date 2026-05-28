use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use tantivy::IndexReader;

use crate::backend::Backend;
use crate::search::SearchSchema;

#[derive(serde::Deserialize)]
pub struct IndexState {
    /// `None` until the first index run completes (key absent in state.json).
    pub last_indexed: Option<String>,
    pub bucket_id: Option<String>,
    pub backend: Option<String>,
}

pub async fn read_state(work_dir: &Path) -> Option<IndexState> {
    let state_path = work_dir.join("state.json");
    let contents = tokio::fs::read_to_string(&state_path).await.ok()?;
    serde_json::from_str::<IndexState>(&contents).ok()
}

pub async fn read_last_indexed(work_dir: &Path) -> String {
    let state_path = work_dir.join("state.json");
    match tokio::fs::read_to_string(&state_path).await {
        Ok(s) => match serde_json::from_str::<IndexState>(&s) {
            Ok(state) => state.last_indexed.unwrap_or_else(|| "indexing in progress".to_string()),
            Err(e) => format!("state.json parse error: {e}"),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "not indexed yet".to_string(),
        Err(e) => format!("failed to read state.json: {e}"),
    }
}

#[derive(Clone)]
pub struct AppState {
    pub profile: ProfileEntry,
    pub signing_secret: [u8; 32],
}

#[derive(Clone)]
pub struct ProfileEntry {
    pub name: String,
    pub description: String,
    pub state: ProfileState,
}

#[derive(Clone)]
pub struct ProfileState {
    pub backend: Backend,
    pub work_dir: PathBuf,
    pub search: Arc<RwLock<SearchState>>,
}

#[derive(Clone)]
pub struct SearchState {
    pub reader: IndexReader,
    pub schema: SearchSchema,
}
