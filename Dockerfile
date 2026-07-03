# syntax=docker/dockerfile:1.12

FROM rust:1.95.0-slim-bookworm AS builder
WORKDIR /src
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential cmake libssl-dev perl pkg-config \
    && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --locked --release

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 65532 pingate \
    && useradd --system --uid 65532 --gid 65532 --no-create-home --home-dir /nonexistent pingate
COPY --from=builder /src/target/release/pingate /usr/local/bin/pingate
USER 65532:65532
EXPOSE 6198 6197
ENV RUST_LOG=info
HEALTHCHECK --interval=10s --timeout=3s --start-period=10s --retries=3 CMD ["pingate", "healthcheck"]
ENTRYPOINT ["pingate"]

LABEL org.opencontainers.image.title="Pingate" \
      org.opencontainers.image.description="Docker-native Pingora reverse proxy and load balancer" \
      org.opencontainers.image.source="https://github.com/BardiaYaghmaie/pingate"
