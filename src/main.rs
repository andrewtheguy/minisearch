mod assets;
mod cli;
mod config;
mod error;
mod handlers;
mod indexer;
mod search;
mod state;

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use anyhow::Context;
use axum::{routing::get, Router};
use clap::Parser;
use cli::{Cli, Commands};
use log::{error, info, warn};
use state::{AppState, ProfileEntry, ProfileState, SearchState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let cli = Cli::parse();
    let config = config::AppConfig::load(&cli.config)?;

    match cli.command {
        Commands::Index { profile: profile_name, every } => {
            let profile = config
                .profiles
                .iter()
                .find(|p| p.name == profile_name)
                .with_context(|| format!("profile not found: {profile_name}"))?;
            indexer::run_indexer(profile).await?;
            if let Some(interval) = every {
                loop {
                    info!("next index run in {}s", interval.as_secs());
                    tokio::time::sleep(interval).await;
                    if let Err(e) = indexer::run_indexer(profile).await {
                        error!("indexer error: {e:#}");
                    }
                }
            }
        }
        Commands::Serve => {
            let mut profiles = Vec::new();
            for profile_config in &config.profiles {
                let s3_client = profile_config.s3_client().await;
                let index_path = PathBuf::from(&profile_config.tantivy_index_path);
                let search = match search::open_index(&index_path) {
                    Some(index) => {
                        let reader = index
                            .reader()
                            .context("failed to create index reader")?;
                        let schema = search::build_schema();
                        Arc::new(RwLock::new(Some(SearchState { reader, schema })))
                    }
                    None => {
                        warn!(
                            "search index not found at {index_path:?} for profile '{}' — search will be unavailable until index is created",
                            profile_config.name
                        );
                        Arc::new(RwLock::new(None))
                    }
                };
                profiles.push(ProfileEntry {
                    name: profile_config.name.clone(),
                    description: profile_config.description.clone(),
                    state: ProfileState {
                        s3_client,
                        bucket_name: profile_config.s3_bucket_name.clone(),
                        index_path,
                        search,
                    },
                });
            }

            let state = AppState { profiles };

            let app = Router::new()
                .route("/api/health", get(|| async { "ok" }))
                .route("/api/profiles", get(handlers::profiles))
                .route("/api/p/{profile}/search", get(handlers::search))
                .route("/api/p/{profile}/presign", get(handlers::presign))
                .route("/api/p/{profile}/browse", get(handlers::browse))
                .with_state(state)
                .fallback(assets::static_handler);

            let listener_v6 = tokio::net::TcpListener::bind("[::1]:52378")
                .await
                .context("failed to bind to [::1]:52378")?;
            let listener_v4 = tokio::net::TcpListener::bind("127.0.0.1:52378")
                .await
                .context("failed to bind to 127.0.0.1:52378")?;
            info!("listening on http://localhost:52378");

            let app_clone = app.clone();
            tokio::spawn(async move {
                if let Err(e) = axum::serve(listener_v6, app_clone).await {
                    error!("IPv6 listener error: {e}");
                }
            });
            axum::serve(listener_v4, app).await.context("server error")?;
        }
    }
    Ok(())
}
