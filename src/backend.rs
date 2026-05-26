use std::time::Duration;

use anyhow::Context;
use aws_sdk_s3::presigning::PresigningConfig;
use log::debug;

use crate::webdav::WebDavClient;

pub fn content_disposition(key: &str) -> String {
    let filename = key.rsplit('/').next().unwrap_or(key);
    let encoded = urlencoding::encode(filename);
    format!("inline; filename*=UTF-8''{encoded}")
}

#[derive(Clone)]
pub enum Backend {
    S3 {
        client: aws_sdk_s3::Client,
        bucket: String,
    },
    WebDav(WebDavClient),
}

pub struct ObjectEntry {
    pub key: String,
    pub size: u64,
    pub last_modified: String,
    pub content_type: Option<String>,
}

pub struct BrowseFolder {
    pub key: String,
    pub name: String,
}

pub struct BrowseFile {
    pub key: String,
    pub name: String,
    pub size: u64,
    pub last_modified: String,
}

pub struct BrowseOutput {
    pub prefix: String,
    pub folders: Vec<BrowseFolder>,
    pub files: Vec<BrowseFile>,
    pub is_truncated: bool,
    pub next_continuation_token: Option<String>,
}

impl Backend {
    pub fn backend_name(&self) -> &str {
        match self {
            Backend::S3 { .. } => "s3",
            Backend::WebDav(_) => "webdav",
        }
    }

    pub fn supports_presign(&self) -> bool {
        true
    }

    pub async fn list_all_objects(&self) -> anyhow::Result<Vec<ObjectEntry>> {
        match self {
            Backend::S3 { client, bucket } => {
                let mut entries = Vec::new();
                let mut continuation_token: Option<String> = None;

                loop {
                    let mut req = client.list_objects_v2().bucket(bucket);
                    if let Some(token) = &continuation_token {
                        req = req.continuation_token(token);
                    }
                    let output = req.send().await.context("failed to list S3 objects")?;

                    for obj in output.contents() {
                        let key = match obj.key() {
                            Some(k) => k.to_string(),
                            None => continue,
                        };
                        let size = obj.size().unwrap_or(0) as u64;
                        let last_modified = obj
                            .last_modified()
                            .map(|dt| {
                                dt.fmt(aws_sdk_s3::primitives::DateTimeFormat::DateTime)
                                    .unwrap_or_default()
                            })
                            .unwrap_or_default();
                        entries.push(ObjectEntry {
                            key,
                            size,
                            last_modified,
                            content_type: None,
                        });
                    }

                    if output.is_truncated() == Some(true) {
                        continuation_token =
                            output.next_continuation_token().map(|s| s.to_string());
                    } else {
                        break;
                    }
                }
                Ok(entries)
            }
            Backend::WebDav(client) => {
                let resources = client.list_all_recursive().await?;
                let mut entries = Vec::new();
                for r in resources {
                    if let Some(key) = client.path_to_key(&r.href) {
                        entries.push(ObjectEntry {
                            key,
                            size: r.content_length.unwrap_or(0),
                            last_modified: r.last_modified.unwrap_or_default(),
                            content_type: r.content_type,
                        });
                    }
                }
                Ok(entries)
            }
        }
    }

    pub async fn head_content_type(&self, key: &str) -> anyhow::Result<Option<String>> {
        match self {
            Backend::S3 { client, bucket } => {
                let head = client
                    .head_object()
                    .bucket(bucket)
                    .key(key)
                    .send()
                    .await?;
                Ok(head.content_type().map(|s| s.to_string()))
            }
            Backend::WebDav(client) => client.head_content_type(key).await,
        }
    }

    pub async fn get_object_body(&self, key: &str) -> anyhow::Result<Vec<u8>> {
        match self {
            Backend::S3 { client, bucket } => {
                let output = client
                    .get_object()
                    .bucket(bucket)
                    .key(key)
                    .send()
                    .await
                    .context("failed to get S3 object")?;
                let bytes = output
                    .body
                    .collect()
                    .await
                    .context("failed to read S3 object body")?;
                Ok(bytes.into_bytes().into())
            }
            Backend::WebDav(client) => client.get(key).await,
        }
    }

    pub async fn get_marker_content(&self, marker_key: &str) -> anyhow::Result<Option<String>> {
        match self {
            Backend::S3 { client, bucket } => {
                let result = client
                    .get_object()
                    .bucket(bucket)
                    .key(marker_key)
                    .send()
                    .await;

                match result {
                    Ok(output) => {
                        let bytes = output
                            .body
                            .collect()
                            .await
                            .context("failed to read marker body")?;
                        let content = String::from_utf8(bytes.into_bytes().into())
                            .context("marker is not valid UTF-8")?;
                        let trimmed = content.trim().to_string();
                        if trimmed.is_empty() {
                            anyhow::bail!("marker {marker_key} exists but is empty");
                        }
                        Ok(Some(trimmed))
                    }
                    Err(err) => {
                        let is_not_found = err
                            .as_service_error()
                            .is_some_and(|e| e.is_no_such_key());
                        if is_not_found {
                            Ok(None)
                        } else {
                            Err(err).context("failed to check marker on backend")
                        }
                    }
                }
            }
            Backend::WebDav(client) => {
                let bytes = client
                    .get_optional(marker_key)
                    .await
                    .with_context(|| format!("failed to check marker '{marker_key}' on WebDAV"))?;
                match bytes {
                    Some(bytes) => {
                        let content =
                            String::from_utf8(bytes).context("marker is not valid UTF-8")?;
                        let trimmed = content.trim().to_string();
                        if trimmed.is_empty() {
                            anyhow::bail!("marker {marker_key} exists but is empty");
                        }
                        Ok(Some(trimmed))
                    }
                    None => Ok(None),
                }
            }
        }
    }

    pub async fn browse(
        &self,
        prefix: &str,
        continuation_token: Option<&str>,
    ) -> anyhow::Result<BrowseOutput> {
        match self {
            Backend::S3 { client, bucket } => {
                let mut req = client
                    .list_objects_v2()
                    .bucket(bucket)
                    .delimiter("/")
                    .prefix(prefix);

                if let Some(token) = continuation_token {
                    req = req.continuation_token(token);
                }

                let output = req.send().await.context("failed to list S3 objects")?;

                let folders = output
                    .common_prefixes()
                    .iter()
                    .filter_map(|cp| {
                        let key = cp.prefix()?.to_string();
                        let name = key.strip_prefix(prefix).unwrap_or(&key).to_string();
                        Some(BrowseFolder { key, name })
                    })
                    .collect();

                let files = output
                    .contents()
                    .iter()
                    .filter_map(|obj| {
                        let key = obj.key()?.to_string();
                        if key == prefix {
                            return None;
                        }
                        let name = key.strip_prefix(prefix).unwrap_or(&key).to_string();
                        let size = obj.size().unwrap_or(0) as u64;
                        let last_modified = obj
                            .last_modified()
                            .map(|dt| {
                                dt.fmt(aws_sdk_s3::primitives::DateTimeFormat::DateTime)
                                    .unwrap_or_default()
                            })
                            .unwrap_or_default();
                        Some(BrowseFile {
                            key,
                            name,
                            size,
                            last_modified,
                        })
                    })
                    .collect();

                let is_truncated = output.is_truncated().unwrap_or(false);
                let next_continuation_token = if is_truncated {
                    output.next_continuation_token().map(|s| s.to_string())
                } else {
                    None
                };

                Ok(BrowseOutput {
                    prefix: prefix.to_string(),
                    folders,
                    files,
                    is_truncated,
                    next_continuation_token,
                })
            }
            Backend::WebDav(client) => {
                let resources = client.propfind(prefix, 1).await?;
                let mut folders = Vec::new();
                let mut files = Vec::new();

                for r in resources {
                    let Some(key) = client.path_to_key(&r.href) else {
                        continue;
                    };
                    let key_with_slash: String = if r.is_collection && !key.ends_with('/') {
                        format!("{key}/")
                    } else {
                        key.clone()
                    };

                    if r.is_collection {
                        if key_with_slash == prefix || key.is_empty() {
                            continue;
                        }
                        let name = key_with_slash
                            .strip_prefix(prefix)
                            .unwrap_or(&key_with_slash)
                            .to_string();
                        folders.push(BrowseFolder {
                            key: key_with_slash,
                            name,
                        });
                    } else {
                        let name = key.strip_prefix(prefix).unwrap_or(&key).to_string();
                        files.push(BrowseFile {
                            key,
                            name,
                            size: r.content_length.unwrap_or(0),
                            last_modified: r.last_modified.unwrap_or_default(),
                        });
                    }
                }

                Ok(BrowseOutput {
                    prefix: prefix.to_string(),
                    folders,
                    files,
                    is_truncated: false,
                    next_continuation_token: None,
                })
            }
        }
    }

    pub async fn presign_url(&self, key: &str) -> anyhow::Result<Option<String>> {
        match self {
            Backend::S3 { client, bucket } => {
                let mime = new_mime_guess::from_path(key).first_or_octet_stream();
                let content_type = if mime.type_() == "text" {
                    format!("{mime}; charset=utf-8")
                } else {
                    mime.to_string()
                };

                let presign_config = PresigningConfig::expires_in(Duration::from_secs(3600))
                    .context("presign config error")?;

                let presigned = client
                    .get_object()
                    .bucket(bucket)
                    .key(key)
                    .response_content_type(&content_type)
                    .response_content_disposition(content_disposition(key))
                    .presigned(presign_config)
                    .await
                    .context("presign failed")?;

                Ok(Some(presigned.uri().to_string()))
            }
            Backend::WebDav(_) => Ok(None),
        }
    }

    pub async fn get_stream(&self, key: &str) -> anyhow::Result<reqwest::Response> {
        match self {
            Backend::WebDav(client) => client.get_stream(key).await,
            Backend::S3 { .. } => anyhow::bail!(
                "get_stream not supported for Backend::S3; use presign_url to obtain a presigned URL for S3 objects instead of the fetch proxy"
            ),
        }
    }

    pub async fn check_connectivity(&self) -> anyhow::Result<()> {
        match self {
            Backend::S3 { client, bucket } => {
                client
                    .list_objects_v2()
                    .bucket(bucket)
                    .max_keys(1)
                    .send()
                    .await
                    .with_context(|| format!("failed to connect to S3 bucket '{bucket}'"))?;
                debug!("S3 connectivity verified for bucket '{bucket}'");
                Ok(())
            }
            Backend::WebDav(client) => {
                client.check_connectivity().await?;
                debug!("WebDAV connectivity verified");
                Ok(())
            }
        }
    }
}
