FROM alpine:3.23

RUN apk add --no-cache bash git kubectl aws-cli curl python3 py3-yaml py3-pip \
  && python3 -m venv --system-site-packages /opt/venv \
  && /opt/venv/bin/pip install --no-cache-dir 'click<8.0' 'jsonref<0.3.0,>=0.2.0' colorama \
  && /opt/venv/bin/pip install --no-cache-dir --no-deps openapi2jsonschema

ENV PATH="/opt/venv/bin:$PATH"

COPY entrypoint.sh /entrypoint.sh
COPY reorganize-schemas.py /reorganize-schemas.py
COPY build-index.py /build-index.py
RUN chmod +x /entrypoint.sh /reorganize-schemas.py /build-index.py

USER 10001

ENTRYPOINT ["/entrypoint.sh"]
