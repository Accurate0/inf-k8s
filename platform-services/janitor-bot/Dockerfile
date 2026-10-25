ARG BINARY_NAME

FROM rust:1.99.1-slim-trixie AS builder
ARG BINARY_NAME

RUN apt-get update -y && apt-get install -y --no-install-recommends pkg-config libssl-dev protobuf-compiler libprotobuf-dev

WORKDIR /app/${BINARY_NAME}-build

COPY . .
RUN \
    --mount=type=cache,target=/app/${BINARY_NAME}-build/target/ \
    --mount=type=cache,target=/usr/local/cargo/registry/ \
    cargo build --locked --release --bin ${BINARY_NAME} && \
    cp ./target/release/${BINARY_NAME} /app

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

COPY --from=builder /app/${BINARY_NAME}-build/static ./static

RUN ln -s /usr/local/bin/${BINARY_NAME} executable
ENTRYPOINT ["./executable"]
EXPOSE 3000/tcp
