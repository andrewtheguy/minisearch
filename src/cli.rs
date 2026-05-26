use std::path::PathBuf;
use std::time::Duration;

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
#[command(name = "minisearch", version, about = "S3/WebDAV file browser with full-text search")]
pub struct Cli {
    #[arg(short, long, env = "MINISEARCH_CONFIG", default_value_os_t = default_config_path())]
    pub config: PathBuf,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start the web server
    Serve {
        /// Profile name to serve
        #[arg(short, long)]
        profile: String,

        /// Port to listen on
        #[arg(long, default_value_t = 52378)]
        port: u16,
    },
    /// Index S3 bucket contents into Tantivy
    Index {
        /// Profile name to index
        #[arg(short, long)]
        profile: String,

        /// Run periodically (e.g. 30m, 1h, 2h30m)
        #[arg(long, value_parser = parse_duration)]
        every: Option<Duration>,
    },
    /// Show profile status (index state, last indexed time)
    Status {
        /// Profile name (shows all profiles if omitted)
        #[arg(short, long)]
        profile: Option<String>,
    },
}

fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("duration must not be empty".into());
    }
    let mut total_secs: u64 = 0;
    let mut current = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            current.push(c);
        } else {
            let n: u64 = current.parse().map_err(|_| format!("invalid number in duration: {s}"))?;
            current.clear();
            match c {
                'h' => total_secs += n * 3600,
                'm' => total_secs += n * 60,
                's' => total_secs += n,
                _ => return Err(format!("unknown duration unit '{c}', use h/m/s")),
            }
        }
    }
    if !current.is_empty() {
        return Err(format!("missing unit suffix in duration: {s} (use h/m/s)"));
    }
    if total_secs == 0 {
        return Err("duration must be greater than zero".into());
    }
    Ok(Duration::from_secs(total_secs))
}
