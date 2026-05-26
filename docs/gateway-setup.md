# Using MiniSearch with a Gateway

MiniSearch natively supports S3 and WebDAV backends. It can also be used to index and search other sources — such as a local filesystem or cloud drive — by fronting them with a compatible gateway.

This guide covers [rclone](https://rclone.org/) (recommended) and [VersityGW](https://github.com/versity/versitygw). Any S3-compatible or WebDAV-compatible server works.

## Prerequisites

- MiniSearch binary ([install instructions](../README.md#install-pre-built-binary))
- [rclone](https://rclone.org/install/) or [VersityGW](https://github.com/versity/versitygw/releases)
- A directory of files (or cloud-drive remote) you want to index

## 1. Start a gateway

### Option A: rclone (recommended)

rclone can serve any configured rclone remote over WebDAV or S3.

#### WebDAV (recommended)

`rclone serve webdav` is stable and has been available since rclone v1.39. It serves any rclone remote as a read-only WebDAV endpoint.

For a local directory:

```bash
rclone serve webdav \
  --addr :7070 \
  --user myuser \
  --pass mypassword \
  --read-only \
  /path/to/your/files
```

For cloud-drive remotes:

```bash
rclone serve webdav --addr :7070 --user myuser --pass mypassword --read-only dropbox:
rclone serve webdav --addr :7070 --user myuser --pass mypassword --read-only onedrive:
rclone serve webdav --addr :7070 --user myuser --pass mypassword --read-only drive:
rclone serve webdav --addr :7070 --user myuser --pass mypassword --read-only box:
rclone serve webdav --addr :7070 --user myuser --pass mypassword --read-only pcloud:
rclone serve webdav --addr :7070 --user myuser --pass mypassword --read-only mega:
```

- `--read-only` — only allows read operations; all write requests are rejected.
- `--user` / `--pass` — basic auth credentials; use the same values in the MiniSearch config.
- `--addr` — address and port to listen on (default `127.0.0.1:8080`).

The served remote appears at the root URL. If you serve `/data`, all files under `/data` are accessible at `http://127.0.0.1:7070/`.

WebDAV file links are not clickable in the MiniSearch UI because there is no presigned URL equivalent. Browse and search work normally.

If the source already has a WebDAV endpoint (Nextcloud, ownCloud, etc.), prefer configuring MiniSearch directly against that endpoint instead of running rclone as an intermediary.

#### S3 (experimental)

`rclone serve s3` is marked **Experimental** as of rclone v1.74.2. It uses a gofakes3 front end over rclone's VFS.

```bash
rclone serve s3 \
  --addr :7070 \
  --auth-key myaccesskey,mysecretkey \
  --read-only \
  /path/to/your/files
```

For cloud-drive remotes:

```bash
rclone serve s3 --addr :7070 --auth-key myaccesskey,mysecretkey --read-only dropbox:
rclone serve s3 --addr :7070 --auth-key myaccesskey,mysecretkey --read-only onedrive:
rclone serve s3 --addr :7070 --auth-key myaccesskey,mysecretkey --read-only drive:
rclone serve s3 --addr :7070 --auth-key myaccesskey,mysecretkey --read-only box:
rclone serve s3 --addr :7070 --auth-key myaccesskey,mysecretkey --read-only pcloud:
rclone serve s3 --addr :7070 --auth-key myaccesskey,mysecretkey --read-only mega:
```

- `--read-only` — only allows read operations; all write requests are rejected.
- `--auth-key` — comma-separated `access_key_id,secret_access_key` pair; use the same values in the MiniSearch config.
- `--addr` — address and port to listen on (default `127.0.0.1:8080`).

rclone treats subdirectories under the served root as buckets and ignores files in the root. If you serve `/data` and your files are in `/data/documents`, the bucket name is `documents`. For cloud-drive remotes, create a top-level folder such as `documents` and use that folder name as the MiniSearch bucket.

### Option B: VersityGW (S3 only)

Point VersityGW at a local directory using the POSIX backend in read-only mode:

```bash
versitygw posix \
  --port 7070 \
  --access myaccesskey \
  --secret mysecretkey \
  --readonly \
  --nometa \
  /path/to/your/files
```

- `--readonly` — enforces strictly read-only access at the gateway level. All write API calls (create, upload, delete) return `AccessDenied` (HTTP 403). Can also be set via env var `VGW_READ_ONLY=true`.
- `--nometa` — disables xattr metadata storage. Useful for read-only setups since there is no metadata to write, and removes the xattr filesystem requirement.
- `--access` / `--secret` — arbitrary credential strings; use the same values in the MiniSearch config.

With the POSIX backend, the S3 bucket name corresponds to a subdirectory under the root path. For example, if you point VersityGW at `/data` and your files are in `/data/documents`, the bucket name is `documents`.

## 2. Configure MiniSearch

### WebDAV backend (rclone serve webdav)

```toml
[[profiles]]
name = "documents"
description = "Documents via rclone WebDAV"
backend = "webdav"
webdav_url = "http://127.0.0.1:7070/"
webdav_username = "myuser"
webdav_password = "mypassword"
```

### S3 backend (rclone serve s3 or VersityGW)

```toml
[[profiles]]
name = "documents"
description = "Documents via S3 gateway"
backend = "s3"
aws_access_key_id = "myaccesskey"
aws_secret_access_key = "mysecretkey"
aws_region = "us-east-1"
aws_endpoint_url = "http://127.0.0.1:7070"
s3_bucket_name = "documents"
```

`aws_region` is required for S3 but not meaningful for a local gateway — any valid region string works.

## 3. Index and serve

```bash
# Build the search index
minisearch -c config.toml index --profile documents

# Start the web server
minisearch -c config.toml serve --profile documents
```

Open http://localhost:52378 to search your files.

## Live file consistency caveat

The gateway can be read-only to clients, but read-only mode only rejects write APIs. It does not make the underlying source immutable or snapshot-isolated. The exact risk depends on the backend behind the gateway.

Behavior checked against full source checkouts of VersityGW v1.4.1 and rclone v1.74.2:

- **rclone v1.74.2**: Both `rclone serve s3` and `rclone serve webdav` use rclone's VFS layer. Listings and object metadata are subject to the VFS directory cache (`--dir-cache-time`, default 5 minutes), so source changes may not appear until the cache expires or is refreshed.
  - With filesystem-like rclone backends (`local`, `sftp`, `ftp`, `smb`, and WebDAV servers that expose a live mutable file tree), the VFS handle ultimately reads from a mutable file source. An actively truncated, overwritten, or appended file can produce stale metadata with live bytes, a partial read, or a read/checksum error. The local backend's `--local-no-check-updated` option freezes recorded size/modtime metadata and caps unranged local opens at that recorded size, which is useful for append-only files, but it is not a snapshot guarantee for arbitrary in-place modifications.
  - With cloud-drive rclone backends (`dropbox`, `onedrive`, `drive`, `box`, `pcloud`, `mega`), reads usually target a provider-side file object rather than an open local file descriptor. In rclone v1.74.2, Dropbox downloads by file ID, OneDrive downloads an item ID through Microsoft Graph `/content`, Google Drive downloads by file ID/download URL or export endpoint, Box downloads `/files/{id}/content`, pCloud fetches a download link for a file ID, and Mega opens a download for a Mega node. These backends are better candidates when you want a gateway facade without worrying about half-read local file writes. They still do not provide a MiniSearch-wide snapshot: listings can be stale because of the VFS cache, a file replaced between list/head/get may resolve to the old or new provider-side object depending on backend behavior, and provider-specific exports such as Google Docs are not equivalent to immutable binary objects.
- **VersityGW POSIX backend v1.4.1**: `ListObjectsV2` walks the local directory tree via `os.DirFS` + `backend.Walk`. `HeadObject` stats the local path (`os.Stat`). `GetObject` stats the path, opens the file (`os.Open`), then stats the open file descriptor to read size and modtime before streaming the body. None of these operations lock the file or retry when it changes mid-read. A file that is truncated, overwritten, or appended by another local process during indexing or download can therefore produce a partial, mixed, or failed read depending on filesystem behavior.

MiniSearch itself is read-only. Search results reflect the last completed index, while browse listings and file downloads read from the backend at request time. If a filesystem-like source can change while MiniSearch is indexing or while users are downloading files, prefer one of these patterns:

- Write files to a temporary path, close them, then atomically rename them into the served tree.
- Point the gateway at a filesystem snapshot or copy when building an index.
- Run indexing during a quiet period and re-index after writers finish.
- For rclone, keep `--vfs-cache-mode off` for the most direct reads, or set a short `--dir-cache-time` if faster visibility of new/deleted files matters. These settings improve freshness, not consistency of an actively modified file.

## Security notes

- MiniSearch never writes to the backend. Even if the gateway allows writes, MiniSearch will not modify your files.
- All three options (VersityGW `--readonly`, rclone `--read-only`) enforce read-only access at the gateway level as an additional safeguard.
- For S3 backends, presigned URLs point back to the gateway endpoint. For the browser to follow them, the gateway must be reachable from the client machine at the configured endpoint URL.
- WebDAV backends do not generate presigned URLs — file links are not clickable in the MiniSearch UI.
- For production deployments, consider running the gateway behind a reverse proxy with TLS. Note that Windows clients require HTTPS for WebDAV basic auth by default.

## Troubleshooting

| Error | Cause | Fix |
|---|---|---|
| `failed to connect to S3 bucket '{name}'` on startup | S3 gateway not running, bucket does not exist, or wrong credentials | Verify the gateway is running, the subdirectory matching your bucket name exists, and the access/secret keys match |
| `WebDAV connectivity check failed` on startup | WebDAV server not running or wrong credentials | Verify the rclone serve webdav command is running and the username/password match |
| `search index not found at {path} — run 'minisearch index ...' first` | Server won't start without an index | Run `minisearch -c config.toml index --profile <name>` before serving |
| `failed to list S3 objects` during browse or indexing | S3 gateway became unreachable or bucket was removed after startup | Check that the gateway is still running and the bucket directory still exists |
| `PROPFIND returned HTTP 401` during indexing | Wrong WebDAV credentials | Check `webdav_username` and `webdav_password` in config match the `--user`/`--pass` flags |
| Presigned URLs return errors in the browser | S3 gateway not reachable from the browser | Ensure the endpoint URL is reachable from the client machine |
