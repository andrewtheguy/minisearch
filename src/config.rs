use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const INDEX_DIR: &str = "tantivy_index";

use anyhow::{bail, Context};
use aws_sdk_s3::config::Credentials;

#[derive(serde::Deserialize, Clone)]
pub struct ProfileConfig {
    pub name: String,
    pub description: String,
    pub aws_access_key_id: String,
    pub aws_secret_access_key: String,
    pub aws_region: String,
    pub aws_endpoint_url: String,
    pub s3_bucket_name: String,
}

impl ProfileConfig {
    pub async fn s3_client(&self) -> aws_sdk_s3::Client {
        let creds = Credentials::new(
            &self.aws_access_key_id,
            &self.aws_secret_access_key,
            None,
            None,
            "toml-config",
        );
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .credentials_provider(creds)
            .region(aws_config::Region::new(self.aws_region.clone()))
            .endpoint_url(&self.aws_endpoint_url)
            .load()
            .await;
        aws_sdk_s3::Client::new(&config)
    }
}

#[derive(serde::Deserialize)]
pub struct AppConfig {
    pub work_dir: String,
    #[serde(default)]
    pub profiles: Vec<ProfileConfig>,
}

impl AppConfig {
    pub fn profile_work_dir(&self, profile_name: &str) -> PathBuf {
        PathBuf::from(&self.work_dir).join(profile_name)
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config file: {}", path.display()))?;
        Self::from_toml_str(&contents)
            .with_context(|| format!("failed to load config file: {}", path.display()))
    }

    fn from_toml_str(contents: &str) -> anyhow::Result<Self> {
        let config = Self::parse_toml(contents)?;
        config.validate()?;
        Ok(config)
    }

    fn parse_toml(contents: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(contents)
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.work_dir.is_empty() {
            bail!("work_dir must not be empty");
        }
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
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_toml(name: &str) -> String {
        format!(
            r#"
[[profiles]]
name = "{name}"
description = "Test profile"
aws_access_key_id = "access"
aws_secret_access_key = "secret"
aws_region = "us-east-1"
aws_endpoint_url = "http://localhost:9000"
s3_bucket_name = "bucket"
"#
        )
    }

    fn config_toml(profiles: &str) -> String {
        format!("work_dir = \"tmp/test-workdir\"\n{profiles}")
    }

    fn parse_error(contents: &str) -> String {
        match AppConfig::from_toml_str(contents) {
            Ok(_) => panic!("expected config parse to fail"),
            Err(err) => err.to_string(),
        }
    }

    #[test]
    fn parses_valid_profiles() {
        let config = AppConfig::from_toml_str(&config_toml(&format!(
            "{}{}",
            profile_toml("docs"),
            profile_toml("media_2026")
        )))
        .unwrap();

        assert_eq!(config.profiles.len(), 2);
        assert_eq!(config.profiles[0].name, "docs");
        assert_eq!(config.profiles[1].name, "media_2026");
    }

    #[test]
    fn derives_profile_work_dir() {
        let config = AppConfig::from_toml_str(&config_toml(&profile_toml("docs"))).unwrap();

        assert_eq!(
            config.profile_work_dir("docs"),
            PathBuf::from("tmp/test-workdir/docs")
        );
    }

    #[test]
    fn rejects_config_without_work_dir() {
        let err = parse_error(&profile_toml("docs"));

        assert!(err.contains("work_dir"), "expected work_dir error, got: {err}");
    }

    #[test]
    fn rejects_empty_work_dir() {
        let err = parse_error(&format!("work_dir = \"\"\n{}", profile_toml("docs")));

        assert_eq!(err, "work_dir must not be empty");
    }

    #[test]
    fn rejects_config_without_profiles() {
        let err = parse_error("work_dir = \"tmp/workdir\"");

        assert_eq!(
            err,
            "config must contain at least one [[profiles]] entry"
        );
    }

    #[test]
    fn rejects_empty_profile_name() {
        let err = parse_error(&config_toml(&profile_toml("")));

        assert_eq!(err, "profile name must not be empty");
    }

    #[test]
    fn rejects_invalid_profile_name() {
        let err = parse_error(&config_toml(&profile_toml("Docs")));

        assert_eq!(
            err,
            "profile name 'Docs' must contain only lowercase letters, digits, hyphens, and underscores"
        );
    }

    #[test]
    fn rejects_duplicate_profile_names() {
        let contents = config_toml(&format!(
            "{}{}",
            profile_toml("docs"),
            profile_toml("docs")
        ));
        let err = parse_error(&contents);

        assert_eq!(err, "duplicate profile name: 'docs'");
    }
}
