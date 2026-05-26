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
    let config = config::AppConfig::load(&cli.config)?;

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
        Commands::Profiles => {
            for profile in &config.profiles {
                println!("{}\t{}", profile.name, profile.description);
            }
        }
        Commands::Serve { profile: profile_name } => {
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

            let index = search::open_index(&index_path)
                .ok_or_else(|| anyhow::anyhow!("search index not found at {index_path:?} — run `minisearch index --profile {profile_name}` first"))?;
            let reader = index.reader().context("failed to create index reader")?;
            let schema = search::build_schema();
            let search = Arc::new(RwLock::new(Some(SearchState { reader, schema })));
            info!("search index loaded from {index_path:?}");

            let state = AppState {
                profiles: vec![ProfileEntry {
                    name: profile_config.name.clone(),
                    description: profile_config.description.clone(),
                    state: ProfileState {
                        s3_client,
                        bucket_name: profile_config.s3_bucket_name.clone(),
                        work_dir,
                        search,
                    },
                }],
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
