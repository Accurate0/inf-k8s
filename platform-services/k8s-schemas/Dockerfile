FROM alpine:3.24

RUN apk add --no-cache bash git kubectl aws-cli curl python3 py3-yaml py3-pip \
  && python3 -m venv --system-site-packages /opt/venv \
  && /opt/venv/bin/pip install --no-cache-dir 'click<8.0' 'jsonref<0.3.0,>=0.2.0' colorama \
  && /opt/venv/bin/pip install --no-cache-dir --no-deps openapi2jsonschema

ENV PATH="/opt/venv/bin:$PATH"

COPY platform-services/k8s-schemas/entrypoint.sh /entrypoint.sh
COPY platform-services/k8s-schemas/reorganize-schemas.py /reorganize-schemas.py
COPY platform-services/k8s-schemas/build-index.py /build-index.py
COPY platform-services/k8s-schemas/patch-helmrelease-schema.py /patch-helmrelease-schema.py
COPY packages/ortzi-app/app.schema.json /schemas-src/app.json
COPY charts /schemas-src/charts
RUN chmod +x /entrypoint.sh /reorganize-schemas.py /build-index.py /patch-helmrelease-schema.py

USER 10001

ENTRYPOINT ["/entrypoint.sh"]
