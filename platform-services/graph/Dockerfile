ARG BINARY_NAME

FROM rust:1.99.1-slim-trixie AS builder
ARG BINARY_NAME

RUN apt-get update -y && apt-get install -y --no-install-recommends pkg-config nodejs npm

WORKDIR /app

COPY crates/subgraph ./crates/subgraph
COPY platform-services/graph ./platform-services/graph

RUN \
    --mount=type=cache,target=/app/platform-services/graph/target/ \
    --mount=type=cache,target=/usr/local/cargo/registry/ \
    cargo build --locked --release \
    --manifest-path platform-services/graph/Cargo.toml \
    --bin ${BINARY_NAME} && \
    cp platform-services/graph/target/release/${BINARY_NAME} /app/${BINARY_NAME}

FROM debian:trixie-slim AS final
ARG BINARY_NAME

RUN apt-get update -y && apt-get install -y --no-install-recommends ca-certificates
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
EXPOSE 4000/tcp
