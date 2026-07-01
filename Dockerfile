# Multi-stage build for the clicord server. samoswallow builds this image,
# routes the container's port 8080 through Caddy and polls /health.

# ---- build stage ----
FROM rust:1-bookworm AS build
WORKDIR /app

# Some build hosts can't reach crates.io's CDN (CloudFront) — the index
# download just times out. Probe it once and, if unreachable, switch cargo to
# a mirror of both the sparse index and the .crate downloads. Safe against
# tampering: cargo verifies every package checksum from Cargo.lock either way.
ARG CRATES_MIRROR=https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/
RUN if ! curl -fsS --max-time 10 -o /dev/null https://index.crates.io/config.json; then \
        echo "crates.io unreachable — falling back to ${CRATES_MIRROR}"; \
        printf '[source.crates-io]\nreplace-with = "mirror"\n\n[source.mirror]\nregistry = "sparse+%s"\n\n[net]\nretry = 5\n' \
            "${CRATES_MIRROR}" >> "${CARGO_HOME}/config.toml"; \
    fi

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# Build only the server binary (the TUI client is not deployed server-side).
RUN cargo build --release --locked -p server

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
