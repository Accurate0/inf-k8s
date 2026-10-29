FROM ghcr.io/rust-cross/cargo-zigbuild:latest@sha256:eb5578cfc06e070a09deb7b5852474bd128a4e561f9f123261b5ed2405e478b1 AS base

RUN apt-get update -y && \
    apt-get install -y --no-install-recommends nodejs git cmake protobuf-compiler libprotobuf-dev && \
    rm -rf /var/lib/apt/lists/*
RUN rustup toolchain install 1.99.1 --profile minimal \
        --target aarch64-apple-darwin --target x86_64-unknown-linux-gnu && \
    rustup default 1.99.1

FROM base AS ortzi

WORKDIR /src
COPY crates/cli-auth ./crates/cli-auth
COPY crates/ortzi ./crates/ortzi
COPY platform-services/registry/api ./platform-services/registry/api
RUN \
    --mount=type=cache,target=/src/crates/ortzi/target/ \
    --mount=type=cache,target=/usr/local/cargo/registry/ \
    cargo build --locked --release --manifest-path crates/ortzi/Cargo.toml && \
    cp crates/ortzi/target/release/ortzi /usr/local/bin/ortzi

FROM base

COPY --from=ortzi /usr/local/bin/ortzi /usr/local/bin/ortzi
ENV ORTZI_REGISTRY_URL=http://api.registry.svc.cluster.local
