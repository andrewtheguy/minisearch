use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const INDEX_DIR: &str = "tantivy_index";

use anyhow::{bail, Context};
use aws_sdk_s3::config::Credentials;

use crate::backend::Backend;
use crate::webdav::WebDavClient;

#[derive(Debug, serde::Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum BackendType {
    S3,
    Webdav,
}

impl std::fmt::Display for BackendType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendType::S3 => write!(f, "s3"),
            BackendType::Webdav => write!(f, "webdav"),
        }
    }
}

#[derive(serde::Deserialize, Clone)]
pub struct ProfileConfig {
    pub name: String,
    pub description: String,
    pub backend: BackendType,
    pub aws_access_key_id: Option<String>,
    pub aws_secret_access_key: Option<String>,
    pub aws_region: Option<String>,
    pub aws_endpoint_url: Option<String>,
    pub s3_bucket_name: Option<String>,
    pub webdav_url: Option<String>,
    pub webdav_username: Option<String>,
    pub webdav_password: Option<String>,
}

impl ProfileConfig {
    pub async fn build_backend(&self) -> anyhow::Result<Backend> {
        match self.backend {
            BackendType::S3 => {
                let access_key = self
                    .aws_access_key_id
                    .as_deref()
                    .context("aws_access_key_id required for s3 backend")?;
                let secret_key = self
                    .aws_secret_access_key
                    .as_deref()
                    .context("aws_secret_access_key required for s3 backend")?;
                let region = self
                    .aws_region
                    .as_deref()
                    .context("aws_region required for s3 backend")?;
                let endpoint = self
                    .aws_endpoint_url
                    .as_deref()
                    .context("aws_endpoint_url required for s3 backend")?;
                let bucket = self
                    .s3_bucket_name
                    .as_deref()
                    .context("s3_bucket_name required for s3 backend")?;

                let creds = Credentials::new(access_key, secret_key, None, None, "toml-config");
                let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
                    .credentials_provider(creds)
                    .region(aws_config::Region::new(region.to_string()))
                    .endpoint_url(endpoint)
                    .load()
                    .await;
                let client = aws_sdk_s3::Client::new(&config);

                Ok(Backend::S3 {
                    client,
                    bucket: bucket.to_string(),
                })
            }
            BackendType::Webdav => {
                let url = self
                    .webdav_url
                    .as_deref()
                    .context("webdav_url required for webdav backend")?;
                let username = self
                    .webdav_username
                    .as_deref()
                    .context("webdav_username required for webdav backend")?;
                let password = self
                    .webdav_password
                    .as_deref()
                    .context("webdav_password required for webdav backend")?;

                let client = WebDavClient::new(url, username, password)?;
                Ok(Backend::WebDav(client))
            }
        }
    }
}

#[derive(serde::Deserialize)]
struct RawAppConfig {
    work_dir: Option<String>,
    #[serde(default)]
    profiles: Vec<ProfileConfig>,
}

pub struct AppConfig {
    pub work_dir: PathBuf,
    pub profiles: Vec<ProfileConfig>,
}

impl AppConfig {
    pub fn profile_work_dir(&self, profile_name: &str) -> PathBuf {
        self.work_dir.join(profile_name)
    }

    pub async fn load(path: &Path) -> anyhow::Result<Self> {
        let contents = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("failed to read config file: {}", path.display()))?;
        let abs_config = std::fs::canonicalize(path)
            .with_context(|| format!("failed to resolve config path: {}", path.display()))?;
        let config_dir = abs_config.parent()
            .context("config path has no parent directory")?;
        Self::from_toml_str(&contents, config_dir)
            .with_context(|| format!("failed to load config file: {}", path.display()))
    }

    fn from_toml_str(contents: &str, config_dir: &Path) -> anyhow::Result<Self> {
        let raw: RawAppConfig = toml::from_str(contents)?;
        let work_dir = match raw.work_dir {
            Some(dir) => {
                let p = PathBuf::from(&dir);
                if !p.is_absolute() {
                    bail!("work_dir must be an absolute path, got: {dir}");
                }
                p
            }
            None => config_dir.join("minisearch_workdir"),
        };
        let config = Self {
            work_dir,
            profiles: raw.profiles,
        };
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.profiles.is_empty() {
            bail!("config must contain at least one [[profiles]] entry");
        }

        let mut seen = HashSet::new();
        for profile in &self.profiles {
            if profile.name.is_empty() {
                bail!("profile name must not be empty");
            }
            if !profile
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
            {
                bail!(
                    "profile name '{}' must contain only lowercase letters, digits, hyphens, and underscores",
                    profile.name
                );
            }
            if !seen.insert(&profile.name) {
                bail!("duplicate profile name: '{}'", profile.name);
            }

            match profile.backend {
                BackendType::S3 => {
                    for (field, value) in [
                        ("aws_access_key_id", &profile.aws_access_key_id),
                        ("aws_secret_access_key", &profile.aws_secret_access_key),
                        ("aws_region", &profile.aws_region),
                        ("aws_endpoint_url", &profile.aws_endpoint_url),
                        ("s3_bucket_name", &profile.s3_bucket_name),
                    ] {
                        if value.is_none() {
                            bail!(
                                "profile '{}': {field} is required for s3 backend",
                                profile.name
                            );
                        }
                    }
                }
                BackendType::Webdav => {
                    for (field, value) in [
                        ("webdav_url", &profile.webdav_url),
                        ("webdav_username", &profile.webdav_username),
                        ("webdav_password", &profile.webdav_password),
                    ] {
                        if value.is_none() {
                            bail!(
                                "profile '{}': {field} is required for webdav backend",
                                profile.name
                            );
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s3_profile_toml(name: &str) -> String {
        format!(
            r#"
[[profiles]]
name = "{name}"
description = "Test profile"
backend = "s3"
aws_access_key_id = "access"
aws_secret_access_key = "secret"
aws_region = "us-east-1"
aws_endpoint_url = "http://localhost:9000"
s3_bucket_name = "bucket"
"#
        )
    }

    fn webdav_profile_toml(name: &str) -> String {
        format!(
            r#"
[[profiles]]
name = "{name}"
description = "Test WebDAV profile"
backend = "webdav"
webdav_url = "https://dav.example.com/files/"
webdav_username = "user"
webdav_password = "pass"
"#
        )
    }

    fn test_config_dir() -> &'static Path {
        Path::new("/tmp/test-config-dir")
    }

    fn config_toml(profiles: &str) -> String {
        format!("work_dir = \"/tmp/test-workdir\"\n{profiles}")
    }

    fn parse(contents: &str) -> anyhow::Result<AppConfig> {
        AppConfig::from_toml_str(contents, test_config_dir())
    }

    fn parse_error(contents: &str) -> String {
        match parse(contents) {
            Ok(_) => panic!("expected config parse to fail"),
            Err(err) => err.to_string(),
        }
    }

    #[test]
    fn parses_s3_profile() {
        let config = parse(&config_toml(&s3_profile_toml("docs"))).unwrap();
        assert_eq!(config.profiles.len(), 1);
        assert_eq!(config.profiles[0].name, "docs");
        assert_eq!(config.profiles[0].backend, BackendType::S3);
    }

    #[test]
    fn parses_webdav_profile() {
        let config = parse(&config_toml(&webdav_profile_toml("mydav"))).unwrap();
        assert_eq!(config.profiles.len(), 1);
        assert_eq!(config.profiles[0].name, "mydav");
        assert_eq!(config.profiles[0].backend, BackendType::Webdav);
    }

    #[test]
    fn parses_mixed_profiles() {
        let config = parse(&config_toml(&format!(
            "{}{}",
            s3_profile_toml("docs"),
            webdav_profile_toml("mydav")
        )))
        .unwrap();
        assert_eq!(config.profiles.len(), 2);
    }

    #[test]
    fn derives_profile_work_dir() {
        let config = parse(&config_toml(&s3_profile_toml("docs"))).unwrap();
        assert_eq!(
            config.profile_work_dir("docs"),
            PathBuf::from("/tmp/test-workdir/docs")
        );
    }

    #[test]
    fn defaults_work_dir_to_config_dir() {
        let config = parse(&s3_profile_toml("docs")).unwrap();
        assert_eq!(
            config.work_dir,
            PathBuf::from("/tmp/test-config-dir/minisearch_workdir")
        );
    }

    #[test]
    fn rejects_relative_work_dir() {
        let err = parse_error(&format!("work_dir = \"relative/path\"\n{}", s3_profile_toml("docs")));
        assert_eq!(err, "work_dir must be an absolute path, got: relative/path");
    }

    #[test]
    fn rejects_empty_work_dir() {
        let err = parse_error(&format!("work_dir = \"\"\n{}", s3_profile_toml("docs")));
        assert_eq!(err, "work_dir must be an absolute path, got: ");
    }

    #[test]
    fn rejects_config_without_profiles() {
        let err = parse_error("work_dir = \"/tmp/workdir\"");
        assert_eq!(
            err,
            "config must contain at least one [[profiles]] entry"
        );
    }

    #[test]
    fn rejects_empty_profile_name() {
        let err = parse_error(&config_toml(&s3_profile_toml("")));
        assert_eq!(err, "profile name must not be empty");
    }

    #[test]
    fn rejects_invalid_profile_name() {
        let err = parse_error(&config_toml(&s3_profile_toml("Docs")));
        assert_eq!(
            err,
            "profile name 'Docs' must contain only lowercase letters, digits, hyphens, and underscores"
        );
    }

    #[test]
    fn rejects_duplicate_profile_names() {
        let contents = config_toml(&format!(
            "{}{}",
            s3_profile_toml("docs"),
            s3_profile_toml("docs")
        ));
        let err = parse_error(&contents);
        assert_eq!(err, "duplicate profile name: 'docs'");
    }

    #[test]
    fn rejects_s3_missing_fields() {
        let toml = format!("work_dir = \"/tmp/workdir\"\n{}", r#"[[profiles]]
name = "bad"
description = "missing s3 fields"
backend = "s3"
"#);
        let err = parse_error(&toml);
        assert!(err.contains("aws_access_key_id is required for s3 backend"));
    }

    #[test]
    fn rejects_webdav_missing_fields() {
        let toml = format!("work_dir = \"/tmp/workdir\"\n{}", r#"[[profiles]]
name = "bad"
description = "missing webdav fields"
backend = "webdav"
"#);
        let err = parse_error(&toml);
        assert!(err.contains("webdav_url is required for webdav backend"));
    }
}
