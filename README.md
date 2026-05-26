# minisearch

Full-text search and browsing for S3 file contents, powered by Tantivy. Rust/Axum backend with an embedded React frontend. Supports multiple named profiles, each pointing to a different S3 bucket with its own search index.

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

Create a TOML config file with one or more `[[profiles]]` entries. Each profile defines a name, description, S3 connection details, and a unique Tantivy index path:

```toml
[[profiles]]
name = "my-bucket"
description = "My S3 bucket files"
aws_access_key_id = "your-access-key"
aws_secret_access_key = "your-secret-key"
aws_region = "us-east-1"
aws_endpoint_url = "https://your-s3-endpoint.example.com"
s3_bucket_name = "your-bucket"
tantivy_index_path = "./tantivy_index/my-bucket"
```

Profile names must be unique and contain only lowercase letters, digits, hyphens, and underscores. The `tantivy_index_path` is used directly as the index directory — ensure each profile has a unique path.

## Usage

The binary has two subcommands:

```bash
# Build the search index for a profile
minisearch index --profile my-bucket

# Start the web server (port 52378)
minisearch serve

# Or with an explicit config file
minisearch -c /path/to/config.toml serve
```

Run `index` first to download and index all files from the S3 bucket, then `serve` to start the web UI. The default view is an S3 folder browser at `/p/<profile>/browse/` with a search bar for full-text search scoped to the current folder. The server works without an index (browsing works, search returns 503), so you can start serving immediately while building the index separately.

## Development

Start the backend and frontend dev server in separate terminals:

```bash
# Terminal 1 — backend (port 52378)
cargo run -- -c tmp/config.toml serve

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

- [S3 Gateway Setup](docs/s3-gateway-setup.md) — use MiniSearch with a local filesystem via an S3 gateway (VersityGW, rclone)

## Frontend Tooling

| Tool | Purpose | Command |
|---|---|---|
| Vite | Dev server & bundler | `bun run dev` |
| TypeScript | Type checking | `tsc -b` |
| Biome | Lint & format | `bun run check` |
| Biome fix | Auto-fix | `bun run check:fix` |
