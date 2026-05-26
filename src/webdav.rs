use anyhow::{bail, Context};
use log::debug;
use quick_xml::Reader;
use quick_xml::events::Event;
use url::Url;

#[derive(Clone)]
pub struct WebDavClient {
    client: reqwest::Client,
    base_url: Url,
    username: String,
    password: String,
}

#[derive(Debug)]
pub struct DavResource {
    pub href: String,
    pub is_collection: bool,
    pub content_length: Option<u64>,
    pub last_modified: Option<String>,
    pub content_type: Option<String>,
}

impl WebDavClient {
    pub fn new(base_url: &str, username: &str, password: &str) -> anyhow::Result<Self> {
        let mut url = Url::parse(base_url).context("invalid WebDAV base URL")?;
        if !url.path().ends_with('/') {
            url.set_path(&format!("{}/", url.path()));
        }
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(30))
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .context("failed to build HTTP client")?;
        Ok(Self {
            client,
            base_url: url,
            username: username.to_string(),
            password: password.to_string(),
        })
    }

    fn resolve_url(&self, path: &str) -> String {
        let path = path.trim_start_matches('/');
        if path.is_empty() {
            self.base_url.to_string()
        } else {
            format!("{}{}", self.base_url, path)
        }
    }

    pub fn path_to_key(&self, href: &str) -> Option<String> {
        let base_path = self.base_url.path();
        let decoded = urlencoding::decode(href).ok()?;
        let trimmed = decoded.strip_prefix(base_path)?;
        let key = trimmed.trim_start_matches('/');
        if key.is_empty() {
            None
        } else {
            Some(key.to_string())
        }
    }

    pub async fn propfind(&self, path: &str, depth: u32) -> anyhow::Result<Vec<DavResource>> {
        let url = self.resolve_url(path);
        let body = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:resourcetype/>
    <D:getcontentlength/>
    <D:getlastmodified/>
    <D:getcontenttype/>
  </D:prop>
</D:propfind>"#;

        let resp = self
            .client
            .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), &url)
            .basic_auth(&self.username, Some(&self.password))
            .header("Depth", depth.to_string())
            .header("Content-Type", "application/xml")
            .body(body)
            .send()
            .await
            .context("PROPFIND request failed")?;

        let status = resp.status();
        if status != 207 {
            bail!("PROPFIND returned HTTP {status}");
        }

        let xml = resp.text().await.context("failed to read PROPFIND body")?;
        parse_multistatus(&xml)
    }

    pub async fn list_all_recursive(&self) -> anyhow::Result<Vec<DavResource>> {
        let mut all_resources = Vec::new();
        let mut queue: Vec<String> = vec![String::new()];

        while let Some(path) = queue.pop() {
            debug!("PROPFIND listing: {}", if path.is_empty() { "/" } else { &path });
            let resources = self.propfind(&path, 1).await?;

            for resource in resources {
                if resource.is_collection {
                    if let Some(key) = self.path_to_key(&resource.href) {
                        let dir_path = if key.ends_with('/') {
                            key
                        } else {
                            format!("{key}/")
                        };
                        if dir_path != path && !dir_path.is_empty() {
                            queue.push(dir_path);
                        }
                    }
                } else {
                    all_resources.push(resource);
                }
            }
        }

        Ok(all_resources)
    }

    pub async fn get(&self, path: &str) -> anyhow::Result<Vec<u8>> {
        let url = self.resolve_url(path);
        let resp = self
            .client
            .get(&url)
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .await
            .context("GET request failed")?;

        if !resp.status().is_success() {
            bail!("GET returned HTTP {}", resp.status());
        }

        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .context("failed to read GET body")
    }

    pub async fn get_optional(&self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let url = self.resolve_url(path);
        let resp = self
            .client
            .get(&url)
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .await
            .context("GET request failed")?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            bail!("GET returned HTTP {}", resp.status());
        }

        resp.bytes()
            .await
            .map(|b| Some(b.to_vec()))
            .context("failed to read GET body")
    }

    pub async fn head_content_type(&self, path: &str) -> anyhow::Result<Option<String>> {
        let url = self.resolve_url(path);
        let resp = self
            .client
            .head(&url)
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .await
            .context("HEAD request failed")?;

        if !resp.status().is_success() {
            bail!("HEAD returned HTTP {}", resp.status());
        }

        Ok(resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string()))
    }

    pub async fn check_connectivity(&self) -> anyhow::Result<()> {
        let url = self.base_url.to_string();
        let resp = self
            .client
            .request(reqwest::Method::OPTIONS, &url)
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .await
            .context("WebDAV connectivity check failed")?;

        if !resp.status().is_success() && resp.status() != 207 {
            bail!(
                "WebDAV server returned HTTP {} for OPTIONS",
                resp.status()
            );
        }
        Ok(())
    }
}

fn parse_multistatus(xml: &str) -> anyhow::Result<Vec<DavResource>> {
    let mut reader = Reader::from_str(xml);
    let mut resources = Vec::new();

    let mut in_response = false;
    let mut in_propstat = false;
    let mut in_prop = false;
    let mut in_resourcetype = false;

    let mut current_href: Option<String> = None;
    let mut current_is_collection = false;
    let mut current_content_length: Option<u64> = None;
    let mut current_last_modified: Option<String> = None;
    let mut current_content_type: Option<String> = None;

    #[derive(PartialEq)]
    enum Reading {
        None,
        Href,
        ContentLength,
        LastModified,
        ContentType,
    }
    let mut reading = Reading::None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e) | Event::Empty(ref e)) => {
                let name_bytes = e.name().into_inner().to_vec();
                let local = local_name(&name_bytes);
                match local {
                    b"response" => {
                        in_response = true;
                        current_href = None;
                        current_is_collection = false;
                        current_content_length = None;
                        current_last_modified = None;
                        current_content_type = None;
                    }
                    b"propstat" if in_response => in_propstat = true,
                    b"prop" if in_propstat => in_prop = true,
                    b"resourcetype" if in_prop => in_resourcetype = true,
                    b"collection" if in_resourcetype => current_is_collection = true,
                    b"href" if in_response => reading = Reading::Href,
                    b"getcontentlength" if in_prop => reading = Reading::ContentLength,
                    b"getlastmodified" if in_prop => reading = Reading::LastModified,
                    b"getcontenttype" if in_prop => reading = Reading::ContentType,
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                let text = e.decode().unwrap_or_default().to_string();
                match reading {
                    Reading::Href => current_href = Some(text),
                    Reading::ContentLength => {
                        current_content_length = text.trim().parse().ok();
                    }
                    Reading::LastModified => current_last_modified = Some(text),
                    Reading::ContentType => current_content_type = Some(text),
                    Reading::None => {}
                }
                reading = Reading::None;
            }
            Ok(Event::End(ref e)) => {
                let name_bytes = e.name().into_inner().to_vec();
                let local = local_name(&name_bytes);
                match local {
                    b"response" if in_response => {
                        if let Some(href) = current_href.take() {
                            resources.push(DavResource {
                                href,
                                is_collection: current_is_collection,
                                content_length: current_content_length,
                                last_modified: current_last_modified.take(),
                                content_type: current_content_type.take(),
                            });
                        }
                        in_response = false;
                    }
                    b"propstat" => in_propstat = false,
                    b"prop" => in_prop = false,
                    b"resourcetype" => in_resourcetype = false,
                    b"href" | b"getcontentlength" | b"getlastmodified" | b"getcontenttype" => {
                        reading = Reading::None;
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => bail!("XML parse error: {e}"),
            _ => {}
        }
    }

    Ok(resources)
}

fn local_name(name: &[u8]) -> &[u8] {
    match name.iter().position(|&b| b == b':') {
        Some(pos) => &name[pos + 1..],
        None => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multistatus_xml() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/dav/files/</D:href>
    <D:propstat>
      <D:prop>
        <D:resourcetype><D:collection/></D:resourcetype>
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>
  <D:response>
    <D:href>/dav/files/readme.txt</D:href>
    <D:propstat>
      <D:prop>
        <D:resourcetype/>
        <D:getcontentlength>1234</D:getcontentlength>
        <D:getlastmodified>Mon, 01 Jan 2024 00:00:00 GMT</D:getlastmodified>
        <D:getcontenttype>text/plain</D:getcontenttype>
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>
</D:multistatus>"#;

        let resources = parse_multistatus(xml).unwrap();
        assert_eq!(resources.len(), 2);

        assert_eq!(resources[0].href, "/dav/files/");
        assert!(resources[0].is_collection);

        assert_eq!(resources[1].href, "/dav/files/readme.txt");
        assert!(!resources[1].is_collection);
        assert_eq!(resources[1].content_length, Some(1234));
        assert_eq!(
            resources[1].last_modified.as_deref(),
            Some("Mon, 01 Jan 2024 00:00:00 GMT")
        );
        assert_eq!(
            resources[1].content_type.as_deref(),
            Some("text/plain")
        );
    }

    #[test]
    fn parses_without_namespace_prefix() {
        let xml = r#"<?xml version="1.0"?>
<multistatus xmlns="DAV:">
  <response>
    <href>/files/doc.pdf</href>
    <propstat>
      <prop>
        <resourcetype/>
        <getcontentlength>5678</getcontentlength>
      </prop>
    </propstat>
  </response>
</multistatus>"#;

        let resources = parse_multistatus(xml).unwrap();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].href, "/files/doc.pdf");
        assert!(!resources[0].is_collection);
        assert_eq!(resources[0].content_length, Some(5678));
    }

    #[test]
    fn path_to_key_strips_base() {
        let client = WebDavClient::new("https://dav.example.com/files/user/", "u", "p").unwrap();
        assert_eq!(
            client.path_to_key("/files/user/docs/readme.txt"),
            Some("docs/readme.txt".to_string())
        );
        assert_eq!(client.path_to_key("/files/user/"), None);
        assert_eq!(client.path_to_key("/other/path"), None);
    }

    #[test]
    fn path_to_key_handles_encoded_paths() {
        let client = WebDavClient::new("https://dav.example.com/files/", "u", "p").unwrap();
        assert_eq!(
            client.path_to_key("/files/my%20doc.txt"),
            Some("my doc.txt".to_string())
        );
    }

    #[test]
    fn resolve_url_joins_paths() {
        let client = WebDavClient::new("https://dav.example.com/files/", "u", "p").unwrap();
        assert_eq!(
            client.resolve_url("docs/readme.txt"),
            "https://dav.example.com/files/docs/readme.txt"
        );
        assert_eq!(
            client.resolve_url(""),
            "https://dav.example.com/files/"
        );
    }
}
