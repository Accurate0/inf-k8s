FROM alpine:3.22

RUN apk add --no-cache bash git kubectl aws-cli

RUN git clone --depth 1 https://github.com/datreeio/CRDs-catalog.git /opt/CRDs-catalog

COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

ENTRYPOINT ["/entrypoint.sh"]
