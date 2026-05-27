# Build stage
FROM rust:1.91-slim-trixie AS builder
ARG TARGETARCH

# Install build dependencies and bun
RUN apt-get update && apt-get install -y \
    build-essential \
    clang \
    mold \
    pkg-config \
    curl \
    unzip \
    && curl -fsSL https://bun.sh/install | bash \
    && rm -rf /var/lib/apt/lists/*

ENV PATH="/root/.bun/bin:${PATH}"

# Set working directory
WORKDIR /build

# Copy source code
COPY . .

# Build frontend first (outside of cargo cache to ensure it always exists)
RUN cd frontend && bun install --frozen-lockfile && bun run build

# Build the release binary with architecture-specific cache mounts
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-v2-${TARGETARCH} \
    --mount=type=cache,target=/build/target,id=cargo-target-v2-${TARGETARCH} \
    cargo build --release --locked && \
    cp target/release/minisearch /minisearch

# Export stage - for extracting standalone binaries (used by docker-bake.hcl)
FROM scratch AS export
COPY --from=builder /minisearch /minisearch

# Runtime stage - minimal image for container deployment (builds from source)
FROM debian:trixie-slim AS runtime

LABEL org.opencontainers.image.source=https://github.com/andrewtheguy/minisearch

RUN apt-get update && apt-get install -y \
    ca-certificates \
    tini \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /minisearch /usr/local/bin/minisearch

EXPOSE 52378

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/minisearch"]

# Runtime stage for pre-built binary (used by CI to avoid double build)
FROM debian:trixie-slim AS runtime-prebuilt

LABEL org.opencontainers.image.source=https://github.com/andrewtheguy/minisearch

RUN apt-get update && apt-get install -y \
    ca-certificates \
    tini \
    && rm -rf /var/lib/apt/lists/*

# Binary must be passed via build context
COPY minisearch /usr/local/bin/minisearch

EXPOSE 52378

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/minisearch"]
