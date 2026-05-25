# fts-everywhere

S3 file browser with a Rust/Axum backend and React frontend.

## Prerequisites

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
```

## Development

Start the backend and frontend dev server in separate terminals:

```bash
# Terminal 1 — backend (port 3000)
cargo run

# Terminal 2 — frontend (port 5173, proxies API to backend)
cd frontend
bun run dev
```

Open http://localhost:5173. API requests (`/files`, `/api/*`) are proxied to the backend.

You can also use `bacon` for auto-rebuilding the backend on file changes.

## Production

Build the frontend and run the backend:

```bash
cd frontend
bun run build    # outputs to frontend/dist/

cd ..
cargo run        # serves the SPA from frontend/dist/
```

Open http://localhost:3000.

## API

| Endpoint       | Method | Response                          |
|----------------|--------|-----------------------------------|
| `/files`       | GET    | JSON list of S3 objects           |
| `/api/health`  | GET    | `ok`                              |

## Frontend Tooling

| Tool       | Purpose              | Command              |
|------------|----------------------|----------------------|
| Vite       | Dev server & bundler | `bun run dev`        |
| TypeScript | Type checking        | `tsc -b`             |
| Biome      | Lint & format        | `bun run check`      |
| Biome fix  | Auto-fix             | `bun run check:fix`  |
