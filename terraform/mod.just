[private]
default:
    @just --list tf

terraform_dir := justfile_directory() / "terraform"

# Run terraform plan
plan:
    cd {{terraform_dir}} && tofu plan

# Run terraform apply
apply:
    cd {{terraform_dir}} && tofu apply

# Run terraform init
init:
    cd {{terraform_dir}} && tofu init

# Write the ESO Azure service principal creds from terraform outputs into the cluster
seed-eso-creds:
    kubectl -n external-secrets create secret generic k8s-shared-vault-creds \
        --from-literal=clientId="$(cd {{terraform_dir}} && tofu output -raw eso_shared_vault_client_id)" \
        --from-literal=clientSecret="$(cd {{terraform_dir}} && tofu output -raw eso_shared_vault_client_secret)" \
        --dry-run=client -o yaml | kubectl apply -f -

# Print the Kanidm client secret for OpenBao OIDC (TF_VAR_OPENBAO_OIDC_CLIENT_SECRET)
openbao-oidc-secret:
    @kubectl -n openbao get secret kanidm-openbao-oidc -o jsonpath='{.data.clientSecret}' | base64 -d

# Create the terraform AppRole in OpenBao and print a role_id/secret_id (needs BAO_TOKEN)
openbao-approle:
    #!/usr/bin/env bash
    set -euo pipefail
    : "${BAO_TOKEN:?BAO_TOKEN must be set}"
    pod=$(kubectl -n openbao get pod -l openbao-active=true -o name)
    bao() { kubectl -n openbao exec -i "$pod" -- env BAO_TOKEN="$BAO_TOKEN" bao "$@"; }

    bao auth list -format=json | jq -e '."approle/"' >/dev/null || bao auth enable approle

    bao policy write terraform - <<'EOF'
    path "sys/mounts" { capabilities = ["read"] }
    path "sys/mounts/*" { capabilities = ["create", "read", "update", "delete", "list", "sudo"] }
    path "sys/auth" { capabilities = ["read"] }
    path "sys/auth/*" { capabilities = ["create", "read", "update", "delete", "list", "sudo"] }
    path "sys/policies/acl/*" { capabilities = ["create", "read", "update", "delete", "list"] }
    path "auth/*" { capabilities = ["create", "read", "update", "delete", "list"] }
    path "identity/*" { capabilities = ["create", "read", "update", "delete", "list"] }
    EOF

    bao write auth/approle/role/terraform token_policies=terraform token_ttl=1h secret_id_ttl=0

    role_id=$(bao read -field=role_id auth/approle/role/terraform/role-id)
    secret_id=$(bao write -f -field=secret_id auth/approle/role/terraform/secret-id)

    echo "TF_VAR_OPENBAO_ROLE_ID=$role_id"
    echo "TF_VAR_OPENBAO_SECRET_ID=$secret_id"
