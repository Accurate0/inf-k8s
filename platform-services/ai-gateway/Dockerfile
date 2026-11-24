ARG BINARY_NAME

FROM rust:1.99.1-slim-trixie AS builder
ARG BINARY_NAME

RUN apt-get update -y && apt-get install -y --no-install-recommends pkg-config libssl-dev protobuf-compiler libprotobuf-dev

# Build context is the repository root: the server depends on crates/subgraph via a path
# dependency, so that tree must be present.
WORKDIR /app

COPY crates/subgraph ./crates/subgraph
COPY platform-services/ai-gateway ./platform-services/ai-gateway

RUN \
    --mount=type=cache,target=/app/platform-services/ai-gateway/target/ \
    --mount=type=cache,target=/usr/local/cargo/registry/ \
    cargo build --locked --release \
    --manifest-path platform-services/ai-gateway/Cargo.toml \
    --bin ${BINARY_NAME} && \
    cp platform-services/ai-gateway/target/release/${BINARY_NAME} /app/${BINARY_NAME}

FROM debian:trixie-slim AS final
ARG BINARY_NAME

RUN apt-get update -y && apt-get install -y --no-install-recommends libssl-dev ca-certificates
RUN update-ca-certificates
RUN useradd \
    --no-create-home \
    --home-dir "/nonexistent" \
    --shell "/sbin/nologin" \
    --uid "10001" \
    appuser

COPY --from=builder /app/${BINARY_NAME} /usr/local/bin/${BINARY_NAME}
RUN chown appuser /usr/local/bin/${BINARY_NAME}

USER appuser

WORKDIR /opt/${BINARY_NAME}

RUN ln -s /usr/local/bin/${BINARY_NAME} executable
ENTRYPOINT ["./executable"]
EXPOSE 3000/tcp
