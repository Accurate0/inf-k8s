# Build context is the repository root: the service is a standalone crate but depends
# on crates/feature-flag-proto via a path dependency, so both trees must be present.
FROM rust:1.96.0-slim-bookworm AS builder

# libprotobuf-dev ships the well-known .proto includes (google/protobuf/*) that
# protobuf-compiler alone omits; prost-build resolves the struct.proto import from there.
RUN apt-get update -y && apt-get install -y --no-install-recommends pkg-config libssl-dev protobuf-compiler libprotobuf-dev

ENV SQLX_OFFLINE=true
WORKDIR /app

COPY crates ./crates
COPY platform-services/feature-flags ./platform-services/feature-flags

RUN \
    --mount=type=cache,target=/app/platform-services/feature-flags/target/ \
    --mount=type=cache,target=/usr/local/cargo/registry/ \
    cargo build --locked --release \
    --manifest-path platform-services/feature-flags/Cargo.toml \
    --bin feature-flags && \
    cp platform-services/feature-flags/target/release/feature-flags /app/feature-flags

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

COPY --from=builder /app/feature-flags /usr/local/bin/feature-flags
RUN chown appuser /usr/local/bin/feature-flags

USER appuser
WORKDIR /opt/feature-flags

EXPOSE 50051
EXPOSE 9090

RUN ln -s /usr/local/bin/feature-flags executable
ENTRYPOINT ["./executable"]
