# Multi-stage build for the clicord server. samoswallow builds this image,
# routes the container's port 8080 through Caddy and polls /health.

# ---- build stage ----
FROM rust:1-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# Build only the server binary (the TUI client is not deployed server-side).
RUN cargo build --release -p server

# ---- runtime stage ----
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /app/target/release/clicord-server /usr/local/bin/clicord-server

# Persist the SQLite database under /data (mount a volume here in production).
RUN mkdir -p /data
ENV CLICORD_LISTEN=0.0.0.0:8080 \
    CLICORD_DB=sqlite:///data/clicord.db

EXPOSE 8080
CMD ["clicord-server"]
