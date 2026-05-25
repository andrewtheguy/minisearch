mod assets;
mod cli;
mod error;
mod handlers;
mod indexer;
mod search;
mod state;

use anyhow::Context;
use axum::{routing::get, Router};
use clap::Parser;
use cli::{Cli, Commands};
use state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();

    match cli.command {
        Commands::Index => {
            indexer::run_indexer().await?;
        }
        Commands::Serve => {
            let aws_config = aws_config::load_from_env().await;
            let s3_client = aws_sdk_s3::Client::new(&aws_config);

            let search::IndexPathResult { path: index_path, bucket: bucket_name } = search::index_path()?;
            let (search_reader, search_schema) = match search::open_index(&index_path) {
                Some(index) => {
                    let reader = index
                        .reader()
                        .context("failed to create index reader")?;
                    let schema = search::build_schema();
                    (Some(reader), Some(schema))
                }
                None => {
                    eprintln!("warning: search index not found at {index_path:?} — search will be unavailable");
                    (None, None)
                }
            };

            let state = AppState {
                s3_client,
                bucket_name,
                search_reader,
                search_schema,
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
            println!("listening on http://localhost:3000");
            axum::serve(listener, app)
                .await
                .context("server error")?;
        }
    }
    Ok(())
}
