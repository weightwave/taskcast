# ── Stage 1: chef ─────────────────────────────────────────────
FROM rust:1-slim AS chef
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef
WORKDIR /app

# ── Stage 2: planner ─────────────────────────────────────────
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ── Stage 3: builder ─────────────────────────────────────────
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
# Place playground dist where rust-embed expects it (../../packages/playground/dist from taskcast-cli/)
RUN mkdir -p /packages/playground && \
    if [ -d /app/playground-dist ]; then cp -r /app/playground-dist /packages/playground/dist; \
    else echo "ERROR: /app/playground-dist not found. Build playground dist before building this image." >&2; exit 1; fi
RUN cargo build --release -p taskcast-cli

# ── Stage 4: runtime ─────────────────────────────────────────
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/taskcast /usr/local/bin/taskcast

EXPOSE 3721
CMD ["taskcast"]