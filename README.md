# minisearch

Full-text search and browsing for S3 and WebDAV file contents, powered by Tantivy. Rust/Axum backend with an embedded React frontend. Supports multiple named profiles, each pointing to a different S3 bucket or WebDAV server with its own search index.

## Install (pre-built binary)

```bash
curl -fsSL https://raw.githubusercontent.com/andrewtheguy/minisearch/main/install.sh | bash
```

Or with options:

```bash
# Install a specific release
curl -fsSL https://raw.githubusercontent.com/andrewtheguy/minisearch/main/install.sh | bash -s -- v0.1.0

# Install latest prerelease
curl -fsSL https://raw.githubusercontent.com/andrewtheguy/minisearch/main/install.sh | bash -s -- --prerelease

# Download to current directory without installing
curl -fsSL https://raw.githubusercontent.com/andrewtheguy/minisearch/main/install.sh | bash -s -- --download-only
```

Supported platforms: Linux (x86_64, arm64), macOS (arm64).

## Building from source

### Prerequisites

### Backend

- [Rust](https://rustup.rs/) (stable)
- [mold](https://github.com/rui314/mold) linker (for fast incremental builds)
- [bacon](https://github.com/Canop/bacon) (file watcher)

```bash
sudo apt install mold clang
cargo install bacon
```

### Frontend

- [Bun](https://bun.sh/)

```bash
cd frontend
bun install
```

## Configuration

By default, minisearch looks for its config at `~/.config/minisearch/config.toml` (or `$XDG_CONFIG_HOME/minisearch/config.toml`). Override with `-c`/`--config` or the `MINISEARCH_CONFIG` env var.

Create a TOML config file with one or more `[[profiles]]` entries. Each profile defines a name, description, backend type (`s3` or `webdav`), and connection details:

```toml
# Optional — absolute path to the working directory for indexes and state.
# Defaults to "minisearch_workdir" in the same directory as this config file.
# work_dir = "/var/lib/minisearch/workdir"

# S3 backend
[[profiles]]
name = "my-bucket"
description = "My S3 bucket files"
backend = "s3"
aws_access_key_id = "your-access-key"
aws_secret_access_key = "your-secret-key"
aws_region = "us-east-1"
aws_endpoint_url = "https://your-s3-endpoint.example.com"
s3_bucket_name = "your-bucket"

# WebDAV backend
[[profiles]]
name = "my-webdav"
description = "My WebDAV server"
backend = "webdav"
webdav_url = "https://dav.example.com/remote.php/dav/files/user/"
webdav_username = "user"
webdav_password = "pass"
```

Profile names must be unique and contain only lowercase letters, digits, hyphens, and underscores. The optional top-level `work_dir` sets the base working directory (must be an absolute path; defaults to `minisearch_workdir` next to the config file). Each profile's data is stored under `<work_dir>/<profile_name>/`, with the Tantivy search index at `<work_dir>/<profile_name>/tantivy_index/` and indexer state at `<work_dir>/<profile_name>/state.json`.

WebDAV profiles do not support presigned URLs, so file links in the MiniSearch UI are not clickable for WebDAV backends.

## Usage

The binary has three subcommands:

```bash
# Show profile status (index state, last indexed time)
minisearch status

# Build the search index for a profile
minisearch index --profile my-bucket

# Start the web server for a profile (default: localhost:52378)
minisearch serve --profile my-bucket

# Or with an explicit config file
minisearch -c /path/to/config.toml serve --profile my-bucket
```

Run `index` first to download and index all files from the backend, then `serve` to start the web UI. The server validates backend connectivity and the search index on startup — if either is unavailable, it fails immediately with a clear error. The home page redirects to `/p/<profile>/browse/` which shows a folder browser with full-text search scoped to the current folder.

The server binds to localhost by default (`127.0.0.1` and `[::1]` dual-stack) and is not accessible from other machines. Use `--bind` to change the address (default: `localhost:52378`). Pass `--bind :PORT` to bind to all interfaces (`[::]`), or `--bind HOST:PORT` for a specific address. To expose it externally, put it behind a reverse proxy or use `--bind :PORT`.

## Development

Start the backend and frontend dev server in separate terminals:

```bash
# Terminal 1 — backend (port 52378)
cargo run -- -c tmp/config.toml serve --profile radio-show

# Terminal 2 — frontend (port 5173, proxies API to backend)
cd frontend
bun run dev
```

Open http://localhost:5173. API requests (`/api/*`) are proxied to the backend.

You can also use `bacon` for auto-rebuilding the backend on file changes.

## Production

Frontend assets are embedded into the Rust binary at compile time via `rust-embed`, producing a single self-contained executable.

```bash
cd frontend
bun run build           # outputs to frontend/dist/

cd ..
cargo build --release   # embeds frontend/dist/ into the binary
```

The resulting binary at `target/release/minisearch` serves the SPA with no external files needed.

## Guides

- [Gateway Setup](docs/gateway-setup.md) — use MiniSearch with a local filesystem or cloud drive via a gateway (rclone, VersityGW)

## Frontend Tooling

| Tool | Purpose | Command |
|---|---|---|
| Vite | Dev server & bundler | `bun run dev` |
| TypeScript | Type checking | `tsc -b` |
| Biome | Lint & format | `bun run check` |
| Biome fix | Auto-fix | `bun run check:fix` |
