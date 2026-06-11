# Build context is the repository root: the service is a standalone crate but depends
# on crates/openfeature-provider (and its transitive feature-flag-* crates) via path
# dependencies, so both trees must be present.
FROM rust:1.96.0-slim-bookworm AS builder

# libprotobuf-dev ships the well-known .proto includes (google/protobuf/*) that
# protobuf-compiler alone omits; prost-build resolves the struct.proto import from there.
RUN apt-get update -y && apt-get install -y --no-install-recommends pkg-config libssl-dev protobuf-compiler libprotobuf-dev

ENV SQLX_OFFLINE=true
WORKDIR /app

COPY crates ./crates
COPY platform-services/ai-gateway ./platform-services/ai-gateway

RUN \
    --mount=type=cache,target=/app/platform-services/ai-gateway/target/ \
    --mount=type=cache,target=/usr/local/cargo/registry/ \
    cargo build --locked --release \
    --manifest-path platform-services/ai-gateway/Cargo.toml \
    --bin ai-gateway && \
    cp platform-services/ai-gateway/target/release/ai-gateway /app/ai-gateway

FROM debian:bookworm-slim AS final

RUN apt-get update -y && apt-get install -y --no-install-recommends libssl-dev ca-certificates
RUN update-ca-certificates
RUN adduser \
    --disabled-password \
    --gecos "" \
    --home "/nonexistent" \
    --shell "/sbin/nologin" \
    --no-create-home \
    --uid "10001" \
    appuser

COPY --from=builder /app/ai-gateway /usr/local/bin/ai-gateway
RUN chown appuser /usr/local/bin/ai-gateway

USER appuser
WORKDIR /opt/ai-gateway

RUN ln -s /usr/local/bin/ai-gateway executable
ENTRYPOINT ["./executable"]
