ARG BINARY_NAME

FROM rust:1.99.1-slim-trixie AS builder
ARG BINARY_NAME

RUN apt-get update -y && apt-get install -y --no-install-recommends pkg-config libssl-dev protobuf-compiler libprotobuf-dev

# Build context is the repository root: the subgraph is served through crates/subgraph.
WORKDIR /app

COPY crates/subgraph ./crates/subgraph
COPY platform-services/janitor-bot ./platform-services/janitor-bot
RUN \
    --mount=type=cache,target=/app/platform-services/janitor-bot/target/ \
    --mount=type=cache,target=/usr/local/cargo/registry/ \
    cargo build --locked --release \
    --manifest-path platform-services/janitor-bot/Cargo.toml \
    --bin ${BINARY_NAME} && \
    cp platform-services/janitor-bot/target/release/${BINARY_NAME} /app/${BINARY_NAME}

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
RUN apt-get update && apt-get install -y --no-install-recommends curl
RUN curl -sSL -o /usr/local/bin/argocd https://github.com/argoproj/argo-cd/releases/latest/download/argocd-linux-amd64 && \
    chmod +x /usr/local/bin/argocd

USER appuser

WORKDIR /opt/${BINARY_NAME}

COPY --from=builder /app/platform-services/janitor-bot/static ./static

RUN ln -s /usr/local/bin/${BINARY_NAME} executable
ENTRYPOINT ["./executable"]
EXPOSE 3000/tcp
