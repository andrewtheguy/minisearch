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
use log::{error, info};
use state::{AppState, ProfileEntry, ProfileState, SearchState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let cli = Cli::parse();
    let config = config::AppConfig::load(&cli.config).await?;

    match cli.command {
        Commands::Index { profile: profile_name, every } => {
            let profile = config
                .profiles
                .iter()
                .find(|p| p.name == profile_name)
                .with_context(|| format!("profile not found: {profile_name}"))?;
            let work_dir = config.profile_work_dir(&profile_name);
            indexer::run_indexer(profile, &work_dir).await?;
            if let Some(interval) = every {
                loop {
                    info!("next index run in {}s", interval.as_secs());
                    tokio::time::sleep(interval).await;
                    if let Err(e) = indexer::run_indexer(profile, &work_dir).await {
                        error!("indexer error: {e:#}");
                    }
                }
            }
        }
        Commands::Status { profile: filter } => {
            let profiles: Vec<_> = if let Some(name) = &filter {
                config.profiles.iter()
                    .filter(|p| p.name == *name)
                    .collect()
            } else {
                config.profiles.iter().collect()
            };
            if profiles.is_empty() {
                if let Some(name) = &filter {
                    anyhow::bail!("profile not found: {name}");
                }
                anyhow::bail!("no profiles configured");
            }
            for profile in profiles {
                let work_dir = config.profile_work_dir(&profile.name);
                let index_path = work_dir.join(config::INDEX_DIR);
                let index_exists = index_path.exists();
                let last_indexed = state::read_last_indexed(&work_dir).await;

                println!("profile:      {}", profile.name);
                println!("description:  {}", profile.description);
                println!("index:        {}", if index_exists { "exists" } else { "not found" });
                println!("last indexed: {last_indexed}");
                println!();
            }
        }
        Commands::Serve { profile: profile_name, port } => {
            let profile_config = config
                .profiles
                .iter()
                .find(|p| p.name == profile_name)
                .with_context(|| format!("profile not found: {profile_name}"))?;

            let s3_client = profile_config.s3_client().await;
            let work_dir = config.profile_work_dir(&profile_name);
            let index_path = work_dir.join(config::INDEX_DIR);

            s3_client
                .list_objects_v2()
                .bucket(&profile_config.s3_bucket_name)
                .max_keys(1)
                .send()
                .await
                .with_context(|| format!("failed to connect to S3 bucket '{}'", profile_config.s3_bucket_name))?;
            info!("S3 connectivity verified for bucket '{}'", profile_config.s3_bucket_name);

            let index_state = state::read_state(&work_dir).await
                .ok_or_else(|| anyhow::anyhow!("state.json not found or not parseable at {work_dir:?} — run `minisearch index --profile {profile_name}` first"))?;
            info!("last indexed: {}, bucket_id: {:?}", index_state.last_indexed, index_state.bucket_id);

            let index = search::open_index(&index_path)
                .ok_or_else(|| anyhow::anyhow!("search index not found at {index_path:?} — run `minisearch index --profile {profile_name}` first"))?;
            let reader = index.reader().context("failed to create index reader")?;
            let schema = search::build_schema();
            let search = Arc::new(RwLock::new(SearchState { reader, schema }));
            info!("search index loaded from {index_path:?}");

            let state = AppState {
                profile: ProfileEntry {
                    name: profile_config.name.clone(),
                    description: profile_config.description.clone(),
                    state: ProfileState {
                        s3_client,
                        bucket_name: profile_config.s3_bucket_name.clone(),
                        work_dir,
                        search,
                    },
                },
            };

            let app = Router::new()
                .route("/", get(handlers::redirect_to_profile))
                .route("/api/health", get(|| async { "ok" }))
                .route("/api/p/{profile}/info", get(handlers::profile_info))
                .route("/api/p/{profile}/search", get(handlers::search))
                .route("/api/p/{profile}/presign", get(handlers::presign))
                .route("/api/p/{profile}/browse", get(handlers::browse))
                .with_state(state)
                .fallback(assets::static_handler);

            let addr_v6 = format!("[::1]:{port}");
            let addr_v4 = format!("127.0.0.1:{port}");
            let listener_v6 = tokio::net::TcpListener::bind(&addr_v6)
                .await
                .with_context(|| format!("failed to bind to {addr_v6}"))?;
            let listener_v4 = tokio::net::TcpListener::bind(&addr_v4)
                .await
                .with_context(|| format!("failed to bind to {addr_v4}"))?;
            info!("listening on http://localhost:{port}");

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
