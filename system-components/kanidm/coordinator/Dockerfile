FROM docker.io/kanidm/server:1.10.3 AS kanidm

FROM docker.io/library/debian:trixie-slim

ARG KUBECTL_VERSION=v1.33.4

RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends ca-certificates curl; \
    curl -fsSLo /usr/local/bin/kubectl "https://dl.k8s.io/release/${KUBECTL_VERSION}/bin/linux/amd64/kubectl"; \
    chmod +x /usr/local/bin/kubectl; \
    apt-get purge -y curl; \
    apt-get autoremove -y; \
    rm -rf /var/lib/apt/lists/*

COPY --from=kanidm /sbin/kanidmd /sbin/kanidmd

USER 1000
