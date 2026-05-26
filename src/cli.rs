use std::path::PathBuf;

use clap::{Parser, Subcommand};

fn default_config_path() -> PathBuf {
    let config_dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let mut home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
            home.push(".config");
            home
        });
    config_dir.join("minisearch").join("config.toml")
}

#[derive(Parser)]
#[command(name = "minisearch", version, about = "S3 file browser with full-text search")]
pub struct Cli {
    #[arg(short, long, env = "MINISEARCH_CONFIG", default_value_os_t = default_config_path())]
    pub config: PathBuf,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start the web server
    Serve,
    /// Index S3 bucket contents into Tantivy
    Index {
        /// Profile name to index
        #[arg(short, long)]
        profile: String,
    },
}
