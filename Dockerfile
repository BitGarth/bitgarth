# syntax=docker/dockerfile:1

# ============================================================
# Stage 1: Builder
# Trixie (glibc 2.40) satisfies the prebuilt dx binary
# requirement of glibc 2.38+.
# ============================================================
FROM rust:1.93-trixie AS builder

ARG TARGETARCH

# Install wasm target for the client-side (WASM) build
RUN rustup target add wasm32-unknown-unknown

WORKDIR /app

# Copy Cargo.toml early so we can extract the dioxus version for the dx CLI download
COPY Cargo.toml Cargo.lock ./
COPY crates/bitgarth-cli/Cargo.toml crates/bitgarth-cli/Cargo.toml

# Install prebuilt dioxus-cli binary (version read from Cargo.toml)
RUN DX_VERSION="$(sed -n 's/^dioxus.*version *= *"\([^"]*\)".*/\1/p' Cargo.toml | head -1)" && \
    echo "Installing dx v${DX_VERSION}" && \
    case "${TARGETARCH}" in \
      amd64) RUST_TARGET="x86_64-unknown-linux-gnu" ;; \
      arm64) RUST_TARGET="aarch64-unknown-linux-gnu" ;; \
      *) echo "Unsupported architecture: ${TARGETARCH}" && exit 1 ;; \
    esac && \
    curl -fsSL "https://github.com/DioxusLabs/dioxus/releases/download/v${DX_VERSION}/dx-${RUST_TARGET}.tar.gz" \
      | tar xz -C /usr/local/bin/ && \
    chmod +x /usr/local/bin/dx && \
    dx --version

# --- Dependency cache layer ---
# Copy only files needed to resolve and compile dependencies.
# build.rs reads Dioxus.toml and migrations/ at compile time.
COPY build.rs ./
COPY Dioxus.toml ./
COPY migrations/ migrations/

# Create dummy targets so cargo can compile dependencies
RUN mkdir -p src crates/bitgarth-cli/src && \
    echo 'fn main() {}' > src/main.rs && \
    echo 'fn main() {}' > crates/bitgarth-cli/src/main.rs

# Pre-build dependencies (cached until Cargo.toml/Cargo.lock change)
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-${TARGETARCH} \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git-${TARGETARCH} \
    --mount=type=cache,target=/app/target,id=cargo-target-${TARGETARCH} \
    cargo build -p bitgarth-app --features web,server --release 2>&1 || true

# --- Full source build ---
COPY src/ src/
COPY assets/ assets/
COPY icons/ icons/

# Build identity for version-drift detection (.git is excluded via
# .dockerignore, so build.rs cannot run `git`; pass the short SHA in).
ARG GIT_SHORT_SHA
ENV GIT_SHORT_SHA=${GIT_SHORT_SHA}

RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-${TARGETARCH} \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git-${TARGETARCH} \
    --mount=type=cache,target=/app/target,id=cargo-target-${TARGETARCH} \
    test -n "${GIT_SHORT_SHA}" || { echo "GIT_SHORT_SHA build arg is required for Docker/Fly builds"; exit 1; } && \
    rm -rf target/release/build/bitgarth-app-* \
        target/wasm32-unknown-unknown/release/build/bitgarth-app-* \
        target/dx/bitgarth-app/release/web && \
    dx build --web --release --debug-symbols=false && \
    mkdir -p /build-output && \
    cp target/dx/bitgarth-app/release/web/server /build-output/bitgarth-app && \
    cp -r target/dx/bitgarth-app/release/web/public /build-output/public && \
    mkdir -p /build-output/assets/catalog && \
    cp assets/catalog/unsynced_asset_catalog.json /build-output/assets/catalog/unsynced_asset_catalog.json

# ============================================================
# Stage 2: Runtime (must match builder glibc)
# ============================================================
FROM debian:trixie-slim AS runtime

ARG IMAGE_VERSION
ARG GIT_SHA

LABEL org.opencontainers.image.version="${IMAGE_VERSION}" \
      org.opencontainers.image.revision="${GIT_SHA}"

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*

# Non-root user
RUN groupadd --gid 1000 bitgarth && \
    useradd --uid 1000 --gid bitgarth --create-home bitgarth

# Persistent data directory (mount a volume here)
RUN mkdir -p /data && chown bitgarth:bitgarth /data

# Copy server binary and static assets (co-located, matching dx build layout)
COPY --from=builder /build-output/bitgarth-app /srv/bitgarth-app
COPY --from=builder /build-output/public /srv/public
COPY --from=builder /build-output/assets /srv/assets

WORKDIR /srv

ENV IP=0.0.0.0 \
    PORT=8080 \
    RUST_LOG=info \
    BITGARTH_CHANNEL=docker \
    BITGARTH_PROJECT_DIR=/data

EXPOSE 8080
VOLUME /data

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

USER bitgarth

ENTRYPOINT ["/srv/bitgarth-app"]
