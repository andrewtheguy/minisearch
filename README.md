# minisearch

Full-text search over S3 file contents, powered by Tantivy. Rust/Axum backend with an embedded React frontend.

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

## Environment

Copy `.env.example` or create `.env` in the project root:

```
AWS_ACCESS_KEY_ID=
AWS_SECRET_ACCESS_KEY=
AWS_REGION=
AWS_ENDPOINT_URL=
S3_BUCKET_NAME=
TANTIVY_INDEX_PATH=./tantivy_index  # optional, defaults to ./tantivy_index
```

## Usage

The binary has two subcommands:

```bash
# Build the search index from S3 bucket contents
cargo run -- index

# Start the web server (port 3000)
cargo run -- serve
```

Run `index` first to download and index all files from the S3 bucket, then `serve` to start the web UI. The server works without an index (search returns 503), so you can start serving immediately while building the index separately.

## Development

Start the backend and frontend dev server in separate terminals:

```bash
# Terminal 1 — backend (port 3000)
cargo run -- serve

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

## API

| Endpoint             | Method | Success Response                                                                                          | Errors                                                                 |
|----------------------|--------|-----------------------------------------------------------------------------------------------------------|------------------------------------------------------------------------|
| `/api/search?q=`     | GET    | JSON search results with structured snippet text segments, byte offsets, and highlight flags               | `400` for missing/invalid query; `503` when no index exists; `500` generic "internal server error" (details logged server-side) |
| `/api/presign?key=`  | GET    | Temporary redirect to a time-limited S3 presigned URL | `400` for missing key; `500` generic "internal server error" (details logged server-side) |
| `/api/health`        | GET    | `ok`                                                                                                      | -                                                                      |

## Guides

- [S3 Gateway Setup](docs/s3-gateway-setup.md) — use MiniSearch with a local filesystem via an S3 gateway (VersityGW, rclone)

## Frontend Tooling

| Tool       | Purpose              | Command              |
|------------|----------------------|----------------------|
| Vite       | Dev server & bundler | `bun run dev`        |
| TypeScript | Type checking        | `tsc -b`             |
| Biome      | Lint & format        | `bun run check`      |
| Biome fix  | Auto-fix             | `bun run check:fix`  |
