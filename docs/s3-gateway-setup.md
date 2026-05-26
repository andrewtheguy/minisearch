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

Serve a local directory as a read-only S3 endpoint with [rclone serve s3](https://rclone.org/commands/rclone_serve_s3/):

```bash
rclone serve s3 \
  --addr :7070 \
  --auth-key myaccesskey,mysecretkey \
  --read-only \
  /path/to/your/files
```

- `--read-only` — only allows read operations; all write requests are rejected.
- `--auth-key` — comma-separated `access_key_id,secret_access_key` pair; use the same values in the MiniSearch config.
- `--addr` — address and port to listen on (default `127.0.0.1:8080`).

Like VersityGW, rclone treats subdirectories under the root as buckets and ignores files in the root. If you serve `/data` and your files are in `/data/documents`, the bucket name is `documents`.

## 2. Configure MiniSearch

Create a `config.toml`:

```toml
[[profiles]]
name = "documents"
description = "Local documents via S3 gateway"
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

The gateway can be read-only to S3 clients, but read-only mode only rejects S3 write APIs. It does not stop other local processes from changing files under the served directory, and it does not make local-file reads snapshot-isolated.

Behavior checked against VersityGW v1.4.1 and rclone v1.74.2:

- **VersityGW POSIX backend v1.4.1**: `ListObjectsV2` walks the local directory tree via `os.DirFS` + `backend.Walk`. `HeadObject` stats the local path (`os.Stat`). `GetObject` stats the path, opens the file (`os.Open`), then stats the open file descriptor to read size and modtime before streaming the body. None of these operations lock the file or retry when it changes mid-read. A file that is truncated, overwritten, or appended by another local process during indexing or download can therefore produce a partial, mixed, or failed read depending on filesystem behavior.
- **rclone serve s3 v1.74.2**: The S3 server reads through rclone's VFS layer backed by gofakes3. `GetObject` opens files read-only through VFS (`file.Open(os.O_RDONLY)`) and streams the handle; `HeadObject` only calls `vfs.Stat`. `--read-only` is a VFS-level flag that prevents S3 clients from writing, but listings and object metadata are still subject to the VFS directory cache (`--dir-cache-time`, default 5 minutes), so direct local changes may not appear until the cache expires or is refreshed. rclone's local backend has a `--local-no-check-updated` option that suppresses the "source file is being updated" abort and instead reads exactly the originally recorded size — useful for append-only files, but not a snapshot guarantee for arbitrary in-place modifications.

MiniSearch itself is read-only against S3. Search results reflect the last completed index, while browse listings and presigned downloads read from the gateway at request time. If files can change while MiniSearch is indexing or while users are downloading them, prefer one of these patterns:

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
