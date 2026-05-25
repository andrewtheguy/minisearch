mod assets;
mod cli;
mod config;
mod error;
mod handlers;
mod indexer;
mod search;
mod state;

use std::sync::{Arc, RwLock};

use anyhow::Context;
use axum::{routing::get, Router};
use clap::Parser;
use cli::{Cli, Commands};
use log::{info, warn};
use state::{AppState, SearchState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let cli = Cli::parse();
    let config = config::AppConfig::load(&cli.config)?;

    match cli.command {
        Commands::Index => {
            indexer::run_indexer(&config).await?;
        }
        Commands::Serve => {
            let s3_client = config.s3_client().await;

            let search::IndexPathResult { path: index_path, bucket: bucket_name } =
                search::index_path(&config.tantivy_index_path, &config.aws_endpoint_url, &config.s3_bucket_name)?;
            let search = match search::open_index(&index_path) {
                Some(index) => {
                    let reader = index
                        .reader()
                        .context("failed to create index reader")?;
                    let schema = search::build_schema();
                    Arc::new(RwLock::new(Some(SearchState { reader, schema })))
                }
                None => {
                    warn!("search index not found at {index_path:?} — search will be unavailable until index is created");
                    Arc::new(RwLock::new(None))
                }
            };

            let state = AppState {
                s3_client,
                bucket_name,
                index_path,
                search,
            };

            let app = Router::new()
                .route("/api/health", get(|| async { "ok" }))
                .route("/api/search", get(handlers::search))
                .route("/api/presign", get(handlers::presign))
                .with_state(state)
                .fallback(assets::static_handler);

            let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
                .await
                .context("failed to bind to port 3000")?;
            info!("listening on http://localhost:3000");
            axum::serve(listener, app)
                .await
                .context("server error")?;
        }
    }
    Ok(())
}
