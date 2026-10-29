FROM ghcr.io/rust-cross/cargo-zigbuild:latest@sha256:eb5578cfc06e070a09deb7b5852474bd128a4e561f9f123261b5ed2405e478b1 AS zigbuild

RUN rm -rf /opt/MacOSX11.3.sdk/usr/share && \
    mv /usr/local/zig-* /opt/zig && \
    cd /opt/zig && rm -rf doc && \
    cd lib/libc && rm -rf mingw wasi musl freebsd netbsd openbsd && \
    rm -rf include/*windows* include/*wasi* include/*musl* include/*freebsd* include/*netbsd* include/*openbsd*

FROM rust:1.99.1-alpine AS base

RUN apk add --no-cache bash git nodejs cmake make perl protobuf-dev
RUN rustup target add aarch64-apple-darwin x86_64-unknown-linux-gnu

COPY --from=zigbuild /opt/MacOSX11.3.sdk /opt/MacOSX11.3.sdk
COPY --from=zigbuild /opt/zig /opt/zig
RUN ln -s /opt/zig/zig /usr/local/bin/zig
ENV SDKROOT=/opt/MacOSX11.3.sdk

FROM base AS build

WORKDIR /src
COPY crates/cli-auth ./crates/cli-auth
COPY crates/ortzi ./crates/ortzi
COPY platform-services/registry/api ./platform-services/registry/api
RUN \
    --mount=type=cache,target=/src/crates/ortzi/target/ \
    --mount=type=cache,target=/usr/local/cargo/registry/ \
    cargo install --locked cargo-zigbuild@0.24.0 && \
    cargo build --locked --release --manifest-path crates/ortzi/Cargo.toml && \
    cp crates/ortzi/target/release/ortzi /usr/local/bin/ortzi

FROM base

COPY --from=build /usr/local/cargo/bin/cargo-zigbuild /usr/local/cargo/bin/cargo-zigbuild
COPY --from=build /usr/local/bin/ortzi /usr/local/bin/ortzi
ENV ORTZI_REGISTRY_URL=http://api.registry.svc.cluster.local
