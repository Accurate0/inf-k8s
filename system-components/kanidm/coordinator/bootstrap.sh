#!/bin/sh
set -eu

SVC_FQDN="${HEADLESS_SVC}.${NAMESPACE}.svc.cluster.local"
SELF_FQDN="${POD_NAME}.${SVC_FQDN}"
SOCK="/data/kanidmd.sock"
BASE="/shared/base.toml"
FINAL="/shared/server.toml"
SECRET="kanidm-repl-certs"

write_common() {
  cat > "$1" <<EOF
bindaddress = "[::]:8443"
ldapbindaddress = "[::]:3636"
db_path = "/data/kanidm.db"
domain = "${KANIDM_DOMAIN}"
origin = "${KANIDM_ORIGIN}"
tls_chain = "/certs/tls.crt"
tls_key = "/certs/tls.key"
adminbindpath = "${SOCK}"

[replication]
origin = "repl://${SELF_FQDN}:8444"
bindaddress = "[::]:8444"
EOF
}

peer_cert() {
  kubectl get secret "$SECRET" -n "$NAMESPACE" -o "jsonpath={.data.$1}" 2>/dev/null
}

write_common "$BASE"

rm -f "$SOCK"
kanidmd server -c "$BASE" >/shared/kanidmd-bootstrap.log 2>&1 &
SRV=$!
i=0
while [ ! -S "$SOCK" ]; do
  i=$((i + 1))
  if [ "$i" -gt 120 ]; then
    echo "timed out waiting for admin socket" >&2
    cat /shared/kanidmd-bootstrap.log >&2
    exit 1
  fi
  sleep 1
done

CERT="$(kanidmd show-replication-certificate -c "$BASE" 2>/dev/null | sed -n 's/.*certificate: *"\(.*\)".*/\1/p')"
kill "$SRV" 2>/dev/null || true
wait "$SRV" 2>/dev/null || true
rm -f "$SOCK"
if [ -z "$CERT" ]; then
  echo "failed to read replication certificate" >&2
  exit 1
fi

kubectl create secret generic "$SECRET" -n "$NAMESPACE" >/dev/null 2>&1 || true
kubectl patch secret "$SECRET" -n "$NAMESPACE" --type merge \
  -p "{\"data\":{\"${POD_NAME}\":\"$(printf %s "$CERT" | base64 -w0)\"}}"

while :; do
  have=0
  j=0
  while [ "$j" -lt "$KANIDM_REPLICAS" ]; do
    if [ -n "$(peer_cert "kanidm-$j")" ]; then
      have=$((have + 1))
    fi
    j=$((j + 1))
  done
  [ "$have" -ge "$KANIDM_REPLICAS" ] && break
  echo "waiting for peer certs ($have/$KANIDM_REPLICAS)"
  sleep 3
done

write_common "$FINAL"
j=0
while [ "$j" -lt "$KANIDM_REPLICAS" ]; do
  peer="kanidm-$j"
  if [ "$peer" != "$POD_NAME" ]; then
    pcert="$(peer_cert "$peer" | base64 -d)"
    {
      echo ""
      echo "[replication.\"repl://${peer}.${SVC_FQDN}:8444\"]"
      echo "type = \"mutual-pull\""
      echo "partner_cert = \"${pcert}\""
      if [ "$peer" = "kanidm-0" ] && [ "$POD_NAME" != "kanidm-0" ]; then
        echo "automatic_refresh = true"
      fi
    } >> "$FINAL"
  fi
  j=$((j + 1))
done

echo "rendered ${FINAL} with $((KANIDM_REPLICAS - 1)) peer(s)"
