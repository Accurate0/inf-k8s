ARG BINARY_NAME

FROM rust:1.96.0-slim-bookworm AS builder
ARG BINARY_NAME

RUN apt-get update -y && apt-get install -y --no-install-recommends pkg-config libssl-dev protobuf-compiler libprotobuf-dev

WORKDIR /app/${BINARY_NAME}-build

COPY . .
RUN \
    --mount=type=cache,target=/app/${BINARY_NAME}-build/target/ \
    --mount=type=cache,target=/usr/local/cargo/registry/ \
    cargo build --locked --release --bin ${BINARY_NAME} --bin aig && \
    cp ./target/release/${BINARY_NAME} /app && \
    cp ./target/release/aig /app/aig

FROM debian:bookworm-slim AS final
ARG BINARY_NAME

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

COPY --from=builder /app/${BINARY_NAME} /usr/local/bin/${BINARY_NAME}
COPY --from=builder /app/aig /usr/local/bin/aig
RUN chown appuser /usr/local/bin/${BINARY_NAME} /usr/local/bin/aig

USER appuser

WORKDIR /opt/${BINARY_NAME}

RUN ln -s /usr/local/bin/${BINARY_NAME} executable
ENTRYPOINT ["./executable"]
EXPOSE 3000/tcp
