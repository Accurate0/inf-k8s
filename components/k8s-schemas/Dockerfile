FROM alpine:3.22

RUN apk add --no-cache bash git kubectl aws-cli curl python3 py3-yaml py3-pip \
  && pip3 install --break-system-packages --no-cache-dir openapi2jsonschema

RUN git clone --depth 1 https://github.com/datreeio/CRDs-catalog.git /opt/CRDs-catalog

COPY entrypoint.sh /entrypoint.sh
COPY reorganize-schemas.py /reorganize-schemas.py
RUN chmod +x /entrypoint.sh /reorganize-schemas.py

ENTRYPOINT ["/entrypoint.sh"]
