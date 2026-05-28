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

        /// Address to bind to (host:port, :port for all interfaces, or localhost:port for dual-stack)
        #[arg(long, default_value = "localhost:52378", value_parser = parse_bind)]
        bind: BindTarget,
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
    /// Check the search index is ready to serve, skipping the backend connectivity check (for container readiness probes)
    CheckIndexReady {
        /// Profile name to check
        #[arg(short, long)]
        profile: String,
    },
}

const DEFAULT_PORT: u16 = 52378;

#[derive(Clone, Debug)]
pub enum BindTarget {
    Localhost(u16),
    AllInterfaces(u16),
    Explicit(String, u16),
}

fn parse_port(s: &str) -> Result<u16, String> {
    let port: u16 = s.parse().map_err(|_| format!("invalid port: '{s}'"))?;
    if port == 0 {
        return Err(format!("invalid port: '{s}'"));
    }
    Ok(port)
}

fn parse_bind(s: &str) -> Result<BindTarget, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("bind address must not be empty".into());
    }

    let (host, port) = if s.starts_with('[') {
        let close = s.find(']').ok_or("missing closing ']' in IPv6 address")?;
        let host = &s[1..close];
        let rest = &s[close + 1..];
        if rest.is_empty() {
            (host.to_string(), DEFAULT_PORT)
        } else if let Some(port_str) = rest.strip_prefix(':') {
            (host.to_string(), parse_port(port_str)?)
        } else {
            return Err(format!("unexpected characters after ']': '{rest}' (expected ':PORT' or nothing)"));
        }
    } else if let Some(colon_pos) = s.rfind(':') {
        let host = &s[..colon_pos];
        let port_str = &s[colon_pos + 1..];
        if port_str.is_empty() {
            (host.to_string(), DEFAULT_PORT)
        } else {
            (host.to_string(), parse_port(port_str)?)
        }
    } else {
        (s.to_string(), DEFAULT_PORT)
    };

    if host.is_empty() {
        Ok(BindTarget::AllInterfaces(port))
    } else if host.eq_ignore_ascii_case("localhost") {
        Ok(BindTarget::Localhost(port))
    } else {
        Ok(BindTarget::Explicit(host, port))
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_localhost_default() {
        assert!(matches!(parse_bind("localhost:52378").unwrap(), BindTarget::Localhost(52378)));
    }

    #[test]
    fn bind_localhost_custom_port() {
        assert!(matches!(parse_bind("localhost:8080").unwrap(), BindTarget::Localhost(8080)));
    }

    #[test]
    fn bind_localhost_no_port() {
        assert!(matches!(parse_bind("localhost").unwrap(), BindTarget::Localhost(52378)));
    }

    #[test]
    fn bind_localhost_case_insensitive() {
        assert!(matches!(parse_bind("LOCALHOST:9090").unwrap(), BindTarget::Localhost(9090)));
    }

    #[test]
    fn bind_all_interfaces() {
        assert!(matches!(parse_bind(":8080").unwrap(), BindTarget::AllInterfaces(8080)));
    }

    #[test]
    fn bind_explicit_ipv4() {
        let b = parse_bind("192.168.1.5:3000").unwrap();
        assert!(matches!(b, BindTarget::Explicit(ref h, 3000) if h == "192.168.1.5"));
    }

    #[test]
    fn bind_explicit_ipv6() {
        let b = parse_bind("[::1]:8080").unwrap();
        assert!(matches!(b, BindTarget::Explicit(ref h, 8080) if h == "::1"));
    }

    #[test]
    fn bind_ipv6_no_port() {
        let b = parse_bind("[::1]").unwrap();
        assert!(matches!(b, BindTarget::Explicit(ref h, 52378) if h == "::1"));
    }

    #[test]
    fn bind_port_zero_rejected() {
        assert!(parse_bind(":0").is_err());
    }

    #[test]
    fn bind_invalid_port() {
        assert!(parse_bind("localhost:abc").is_err());
    }

    #[test]
    fn bind_empty_rejected() {
        assert!(parse_bind("").is_err());
    }

    #[test]
    fn bind_trailing_colon_uses_default_port() {
        assert!(matches!(parse_bind("localhost:").unwrap(), BindTarget::Localhost(52378)));
    }
}
