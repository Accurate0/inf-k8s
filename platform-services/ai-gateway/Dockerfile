ARG BINARY_NAME

FROM rust:1.99.1-slim-trixie AS builder
ARG BINARY_NAME

RUN apt-get update -y && apt-get install -y --no-install-recommends pkg-config libssl-dev protobuf-compiler libprotobuf-dev

# Build context is the repository root: the aig CLI depends on crates/cli-auth via a
# path dependency, so both trees must be present.
WORKDIR /app

COPY crates/cli-auth ./crates/cli-auth
COPY platform-services/ai-gateway ./platform-services/ai-gateway

RUN \
    --mount=type=cache,target=/app/platform-services/ai-gateway/target/ \
    --mount=type=cache,target=/usr/local/cargo/registry/ \
    cargo build --locked --release \
    --manifest-path platform-services/ai-gateway/Cargo.toml \
    --bin ${BINARY_NAME} --bin aig && \
    cp platform-services/ai-gateway/target/release/${BINARY_NAME} /app/${BINARY_NAME} && \
    cp platform-services/ai-gateway/target/release/aig /app/aig

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
COPY --from=builder /app/aig /usr/local/bin/aig
RUN chown appuser /usr/local/bin/${BINARY_NAME} /usr/local/bin/aig

USER appuser

WORKDIR /opt/${BINARY_NAME}

RUN ln -s /usr/local/bin/${BINARY_NAME} executable
ENTRYPOINT ["./executable"]
EXPOSE 3000/tcp
