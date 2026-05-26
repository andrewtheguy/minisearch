# Using MiniSearch with an S3 Gateway

MiniSearch is designed for real S3 buckets, where object storage guarantees read-after-write consistency and objects are immutable once written. It can also be used creatively to index and search other sources — such as a local filesystem fronted by an S3-compatible gateway — with the caveat that file consistency depends on the gateway and underlying storage (see [Live file consistency caveat](#live-file-consistency-caveat) below).

MiniSearch only performs read-only S3 operations (ListObjectsV2, HeadObject, GetObject, presigned GET), so the gateway never needs write permissions.

This guide covers [VersityGW](https://github.com/versity/versitygw) and [rclone serve s3](https://rclone.org/commands/rclone_serve_s3/), but any S3-compatible gateway works.

## Prerequisites

- MiniSearch binary ([install instructions](../README.md#install-pre-built-binary))
- [VersityGW](https://github.com/versity/versitygw/releases) or [rclone](https://rclone.org/install/)
- A directory of files you want to index

## 1. Start an S3 gateway

### Option A: VersityGW

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

### Option B: rclone

rclone can serve any configured rclone remote as a read-only S3 endpoint with [rclone serve s3](https://rclone.org/commands/rclone_serve_s3/).

For a local directory:

```bash
rclone serve s3 \
  --addr :7070 \
  --auth-key myaccesskey,mysecretkey \
  --read-only \
  /path/to/your/files
```

For cloud-drive remotes that do not normally expose an S3 API, use the remote name instead:

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

Like VersityGW, rclone treats subdirectories under the served root as buckets and ignores files in the root. If you serve `/data` and your files are in `/data/documents`, the bucket name is `documents`. For cloud-drive remotes, create a top-level folder such as `documents` and use that folder name as the MiniSearch bucket.

If the source already has an S3-compatible endpoint, prefer configuring MiniSearch directly against that endpoint. rclone is most useful here as a bridge for providers such as Dropbox, OneDrive, Google Drive, Box, pCloud, and Mega.

## 2. Configure MiniSearch

Create a `config.toml`:

```toml
[[profiles]]
name = "documents"
description = "Documents via S3 gateway"
aws_access_key_id = "myaccesskey"
aws_secret_access_key = "mysecretkey"
aws_region = "us-east-1"
aws_endpoint_url = "http://127.0.0.1:7070"
s3_bucket_name = "documents"
tantivy_index_path = "./tantivy_index/documents"
```

`aws_region` is required but not meaningful for a local gateway — any valid region string works.

## 3. Index and serve

```bash
# Build the search index
minisearch -c config.toml index --profile documents

# Start the web server
minisearch -c config.toml serve --profile documents
```

Open http://localhost:52378 to search your files.

## Live file consistency caveat

The gateway can be read-only to S3 clients, but read-only mode only rejects S3 write APIs. It does not make the underlying source immutable or snapshot-isolated. The exact risk depends on the backend behind the gateway.

Behavior checked against full source checkouts of VersityGW v1.4.1 and rclone v1.74.2:

- **VersityGW POSIX backend v1.4.1**: `ListObjectsV2` walks the local directory tree via `os.DirFS` + `backend.Walk`. `HeadObject` stats the local path (`os.Stat`). `GetObject` stats the path, opens the file (`os.Open`), then stats the open file descriptor to read size and modtime before streaming the body. None of these operations lock the file or retry when it changes mid-read. A file that is truncated, overwritten, or appended by another local process during indexing or download can therefore produce a partial, mixed, or failed read depending on filesystem behavior.
- **rclone serve s3 v1.74.2**: The S3 server is a gofakes3 front end over rclone's VFS. `ListBucket` traverses VFS directories (`VFS.Stat` + `Dir.ReadDirAll`). `HeadObject` stats the bucket and object through VFS. `GetObject` stats the bucket and object, records the VFS node size and metadata, opens the VFS file read-only (`file.Open(os.O_RDONLY)`), then streams that handle. `--read-only` is the VFS `read_only` option; it prevents writes, removals, and modtime changes through S3 requests. Listings and object metadata are subject to the VFS directory cache (`--dir-cache-time`, default 5 minutes), so source changes may not appear until the cache expires or is refreshed.
  - With filesystem-like rclone backends (`local`, `sftp`, `ftp`, `smb`, and WebDAV servers that expose a live mutable file tree), the VFS handle ultimately reads from a mutable file source. This has the same general risk as the VersityGW POSIX case: an actively truncated, overwritten, or appended file can produce stale metadata with live bytes, a partial read, or a read/checksum error. The local backend's `--local-no-check-updated` option freezes recorded size/modtime metadata and caps unranged local opens at that recorded size, which is useful for append-only files, but it is not a snapshot guarantee for arbitrary in-place modifications.
  - With cloud-drive rclone backends (`dropbox`, `onedrive`, `drive`, `box`, `pcloud`, `mega`), reads usually target a provider-side file object rather than an open local file descriptor. In rclone v1.74.2, Dropbox downloads by file ID, OneDrive downloads an item ID through Microsoft Graph `/content`, Google Drive downloads by file ID/download URL or export endpoint, Box downloads `/files/{id}/content`, pCloud fetches a download link for a file ID, and Mega opens a download for a Mega node. These backends are better candidates when you want an S3 facade without worrying about half-read local file writes. They still do not provide a MiniSearch-wide snapshot: listings can be stale because of the VFS cache, a file replaced between list/head/get may resolve to the old or new provider-side object depending on backend behavior, and provider-specific exports such as Google Docs are not equivalent to immutable binary objects.

MiniSearch itself is read-only against S3. Search results reflect the last completed index, while browse listings and presigned downloads read from the gateway at request time. If a filesystem-like source can change while MiniSearch is indexing or while users are downloading files, prefer one of these patterns:

- Write files to a temporary path, close them, then atomically rename them into the served tree.
- Point the gateway at a filesystem snapshot or copy when building an index.
- Run indexing during a quiet period and re-index after writers finish.
- For rclone, keep `--vfs-cache-mode off` for the most direct reads, or set a short `--dir-cache-time` if faster visibility of new/deleted files matters. These settings improve freshness, not consistency of an actively modified file.

## Security notes

- MiniSearch never writes to S3. Even if the gateway allows writes, MiniSearch will not modify your files.
- Both VersityGW (`--readonly`) and rclone (`--read-only`) enforce read-only access at the gateway level as an additional safeguard.
- Presigned URLs point back to the gateway endpoint. For the browser to follow them, the gateway must be reachable from the client machine at the configured endpoint URL.
- For production deployments, consider running the gateway behind a reverse proxy with TLS.

## Troubleshooting

| Error | Cause | Fix |
|---|---|---|
| `failed to connect to S3 bucket '{name}'` on startup | Gateway not running, bucket does not exist, or wrong credentials | Verify the gateway is running, the subdirectory matching your bucket name exists, and the access/secret keys match |
| `search index not found at {path} — run 'minisearch index ...' first` | Server won't start without an index | Run `minisearch -c config.toml index --profile <name>` before serving |
| `failed to list S3 objects` during browse or indexing | Gateway became unreachable or bucket was removed after startup | Check that the gateway is still running and the bucket directory still exists |
| Presigned URLs return errors in the browser | Gateway not reachable from the browser | Ensure the endpoint URL is reachable from the client machine |
