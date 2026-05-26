# Architecture

MiniSearch is a full-text search application for S3 objects. It indexes file contents and metadata from S3-compatible buckets into [Tantivy](https://github.com/quickwit-oss/tantivy) search indices, then serves a web UI for querying and browsing results. The server runs a single profile at a time, validating S3 connectivity and the search index on startup.

## High-level overview

```
                  ┌──────────────┐
                  │  S3 Buckets  │
                  └──────┬───────┘
                         │
              ┌──────────┼──────────┐
              │          │          │
        ┌─────▼─────┐ ┌─▼───────┐ ┌▼─────────┐
        │  Indexer   │ │ Browse  │ │  Presign  │
        │ (CLI mode) │ │(runtime)│ │ (runtime) │
        └─────┬──────┘ └─────────┘ └───────────┘
              │
        ┌─────▼──────┐
        │  Tantivy   │
        │  Indices   │
        └─────┬──────┘
              │
        ┌─────▼──────┐       ┌────────────┐
        │ Axum HTTP  │◄──────│  React SPA │
        │  Server    │       │ (embedded) │
        └────────────┘       └────────────┘
```

The application ships as a single binary. The React frontend is compiled and embedded into the binary at build time via `rust-embed`, so no separate static file hosting is needed.

## CLI modes

The binary has three subcommands:

- **`index`** — Scans an S3 bucket, downloads text files, and builds/updates the Tantivy index on disk. Requires a `--profile` flag to specify which profile to index.
- **`serve`** — Starts the Axum web server on port 52378 for a single profile. Requires `--profile` flag. Validates S3 connectivity and search index on startup.
- **`status`** — Shows profile status: name, description, whether the index exists, and last indexed time. Accepts optional `--profile` to filter to a single profile.

Configuration is loaded from a TOML file (`-c`/`--config` flag or `MINISEARCH_CONFIG` env var).

## Backend (Rust)

### Module layout

| Module | Responsibility |
|---|---|
| `main.rs` | Entry point — parses CLI args, loads config, dispatches to indexer or server |
| `cli.rs` | Clap-based CLI definition (`Serve` / `Index` commands, `--profile` flag) |
| `config.rs` | TOML config parsing with multi-profile support, profile name validation, S3 client construction |
| `state.rs` | Per-profile shared state (`ProfileEntry`, `ProfileState`) organized in `AppState` |
| `search.rs` | Tantivy schema definition, tokenizer registration, index open/create |
| `indexer.rs` | S3 object listing, content downloading, incremental index updates |
| `handlers.rs` | Axum request handlers for profile listing, search, browse, presign, and health endpoints |
| `error.rs` | `AppError` enum — maps error variants to HTTP status codes |
| `assets.rs` | Embedded frontend asset serving with SPA fallback |

### Tantivy schema

| Field | Type | Indexed | Stored | Notes |
|---|---|---|---|---|
| `key` | Text | Yes (Jieba) | Yes | S3 object key |
| `content` | Text | Yes (Jieba) | Yes | File body (text files only) |
| `size` | u64 | No | Yes | File size in bytes |
| `last_modified` | String | No | Yes | ISO 8601 timestamp |

The [Jieba](https://github.com/nickel-org/tantivy-jieba) tokenizer handles both Chinese and English text segmentation.

Each profile's working directory is derived as `<work_dir>/<profile_name>/` (where `work_dir` is the top-level config setting). The Tantivy index is stored under `<work_dir>/<profile_name>/tantivy_index/`. After a successful indexing run, the indexer writes `<work_dir>/<profile_name>/state.json` with a `last_indexed` timestamp.

### Indexing pipeline

1. Lists all objects in the S3 bucket (paginated with continuation tokens).
2. For each object, checks whether it has already been indexed with the same `last_modified` timestamp — if so, skips it.
3. Determines if the file is text based on file extension (`.txt`, `.md`, `.json`, `.py`, etc.) or HTTP `Content-Type` header.
4. Text files: downloads body and indexes both key and content. Non-text files: indexes key only.
5. Removes index entries for S3 objects that no longer exist.
6. Commits to the Tantivy index every 100 documents.

### Frontend routes

| Path | Description |
|---|---|
| `/` | Redirects to `/p/<name>/browse/` (server-side) |
| `/p/<name>` | Redirects to `/p/<name>/browse/` |
| `/p/<name>/browse/*` | Browse and search UI — S3 folder browser with inline search |

### API endpoints

| Endpoint | Method | Success Response | Errors |
|---|---|---|---|
| `/` | GET | Redirects to `/p/:profile/browse/` | - |
| `/api/p/:profile/info` | GET | JSON `{ name, description, last_indexed }` — `last_indexed` is read from `state.json` and contains either an ISO 8601 timestamp or a status message (e.g. "not indexed yet") | `404` for unknown profile |
| `/api/p/:profile/search?q=&prefix=` | GET | JSON search results with structured snippet text segments, byte offsets, and highlight flags. Optional `prefix` scopes results to keys under that S3 prefix. | `400` for missing/invalid query; `404` for unknown profile; `503` when no index exists; `500` generic "internal server error" (details logged server-side) |
| `/api/p/:profile/browse?prefix=&continuation_token=` | GET | JSON listing of folders and files at the given S3 prefix | `404` for unknown profile; `500` generic "internal server error" (details logged server-side) |
| `/api/p/:profile/presign?key=` | GET | Temporary redirect to a time-limited S3 presigned URL | `400` for missing key; `404` for unknown profile; `500` generic "internal server error" (details logged server-side) |
| `/api/health` | GET | `ok` | - |
| `/*` | GET | Serves embedded frontend assets (SPA fallback to `index.html`) | - |

### Search response

Results include structured snippet segments that indicate which portions of the text matched the query:

```json
{
  "query": "search term",
  "count": 42,
  "limit": 10,
  "page": 1,
  "total_pages": 3,
  "results": [
    {
      "key": "path/to/file.md",
      "snippet": [
        { "text": "some ", "highlighted": false, "start": 0, "end": 5 },
        { "text": "search", "highlighted": true, "start": 5, "end": 11 },
        { "text": " context", "highlighted": false, "start": 11, "end": 19 }
      ],
      "score": 3.5,
      "size": 12345,
      "last_modified": "2025-05-25T12:34:56Z"
    }
  ]
}
```

Snippet generation selects the best 150-character fragment with the most query term matches and splits it into highlighted/non-highlighted segments.

### Browse response

The browse endpoint lists S3 objects at a given prefix using delimiter-based folder navigation (S3 `list_objects_v2` with `Delimiter=/`). Folders come from `CommonPrefixes`, files from `Contents`.

```json
{
  "prefix": "transcripts/rthk-radio1/",
  "folders": [
    { "key": "transcripts/rthk-radio1/2026/", "name": "2026/" }
  ],
  "files": [
    {
      "key": "transcripts/rthk-radio1/file.json",
      "name": "file.json",
      "size": 51973,
      "last_modified": "2026-05-14T03:53:44.384Z"
    }
  ],
  "is_truncated": false,
  "next_continuation_token": null
}
```

Pagination uses S3 continuation tokens. Each request returns up to 1000 items; when `is_truncated` is true, the frontend can fetch the next page by passing `next_continuation_token`.

### Shared state

```
AppState
└── profiles: Vec<ProfileEntry>
     └── ProfileEntry
         ├── name: String
         ├── description: String
         └── state: ProfileState
             ├── s3_client: aws_sdk_s3::Client
             ├── bucket_name: String
             ├── work_dir: PathBuf
             └── search: Arc<RwLock<Option<SearchState>>>
                  └── SearchState
                      ├── reader: IndexReader
                      └── schema: SearchSchema
```

The search index and S3 connectivity are validated on startup. The server refuses to start if the index doesn't exist or S3 is unreachable.

### Error handling

- `anyhow` for application-level errors (main, indexer, search internals).
- `thiserror` for typed API errors (`AppError` in `error.rs`):
  - `BadRequest` → 400
  - `NotFound` → 404
  - `ServiceUnavailable` → 503
  - `Internal` → 500 (generic message to client, full error chain logged to stderr)

## Frontend (React + TypeScript)

### Stack

- React 19 with TypeScript
- React Router 7 (client-side routing)
- Vite (bundler)
- Tailwind CSS (styling)
- @base-ui/react (headless UI primitives)
- lucide-react (icons)
- Biome (linting/formatting)

### Key behaviors

- **Profile routing**: root path (`/`) redirects to `/p/<name>/browse/` (server-side). The header displays the profile name, description, and last indexed time from `/api/p/<name>/info`.
- **Browse view**: the default view is an S3 folder browser with breadcrumb navigation. Folders are navigable via URL path segments (`/p/<name>/browse/transcripts/rthk-radio1/`). Clicking a file opens it in a new window via the presign endpoint. Uses S3's default batch size (1000 items per page) with Previous/Next navigation via continuation tokens.
- **Inline search**: a search bar is always visible above the browse listing. Submitting a search replaces the folder listing with search results inline (scoped to the current prefix); a "Clear" button returns to browse mode. Search state (`q`, `page`, `mode`, `ext`) is synced to URL query parameters.
- **Request cancellation**: uses `AbortController` to cancel in-flight searches and browse requests.
- **Snippet rendering**: displays search result snippets with `<mark>` tags on highlighted terms.
- **File access**: result links and file rows point to `/api/p/<profile>/presign?key=...`, which redirects to a temporary S3 URL.
- **Search pagination**: 10 results per page. First/previous/next/last page controls with scroll-to-top on page change.

## Build and deployment

### Single binary build

```
frontend/src/ ──► vite build ──► frontend/dist/ ──► rust-embed ──► cargo build ──► minisearch binary
```

The frontend is built first, then `rust-embed` bundles the `frontend/dist/` directory into the Rust binary at compile time.

### Development

```bash
# Backend (port 52378)
cargo run -- -c config.toml serve --profile my-bucket

# Frontend dev server (port 5173, proxies /api to :52378)
cd frontend && bun run dev
```

### CI/CD

GitHub Actions builds release binaries for:
- Linux x86_64
- Linux arm64
- macOS arm64

### Configuration

All configuration is in a single TOML file with a top-level `work_dir` and `[[profiles]]` array entries:

```toml
work_dir = "./workdir"

[[profiles]]
name = "my-bucket"
description = "My S3 bucket files"
aws_access_key_id = "..."
aws_secret_access_key = "..."
aws_region = "us-east-1"
aws_endpoint_url = "https://s3.amazonaws.com"
s3_bucket_name = "my-bucket"
```

Profile names must be unique and URL-safe (lowercase letters, digits, hyphens, underscores). Each profile's working directory is derived as `<work_dir>/<profile_name>/`.

## Deployment patterns

### Direct S3

The default setup: point MiniSearch at an AWS S3 or S3-compatible object store (MinIO, Garage, etc.).

### S3 gateway over local filesystem

Use an S3 gateway like [VersityGW](https://github.com/versity/versitygw) to expose a local directory as an S3-compatible endpoint. MiniSearch only performs read-only S3 operations, so the gateway never needs write permissions. See [S3 Gateway Setup](s3-gateway-setup.md) for a step-by-step guide.
