# List available recipes
[private]
default:
    @just --list platform

# Apply a single platform service application manifest
apply service:
    kubectl apply -f {{service}}/application.yaml

# Apply all platform service application manifests
apply-all:
    for f in */application.yaml; do kubectl apply -f "$f"; done

# Delete a platform service application
delete service:
    kubectl delete -f {{service}}/application.yaml

# Show diff for a service application before applying
diff service:
    kubectl diff -f {{service}}/application.yaml

# Diff a service's kustomize manifests
diff-manifests service:
    kubectl diff -k {{service}}/manifests

# Install all platform service binaries with cargo
install:
    cargo install --path ai-gateway
    cargo install --path janitor-bot
    cargo install --path feature-flags

# Run the ai-gateway aig binary
aig *args:
    cargo run --manifest-path ai-gateway/Cargo.toml --bin aig -- {{args}}

# Run the janitor-bot binary
janitor *args:
    cargo run --manifest-path janitor-bot/Cargo.toml --bin janitor-bot -- {{args}}

# Run the ffctl binary
ffctl *args:
    cd feature-flags && cargo run --bin ffctl -- {{args}}

# Show ArgoCD applications status
status:
    kubectl get applications -n argocd

# Sync a platform service application
sync service:
    kubectl -n argocd patch application {{service}} --type merge -p '{"operation":{"initiatedBy":{"username":"just"},"sync":{"prune":true}}}'

# Bump the inf-k8s feature-flag crates in every consumer service's Cargo.lock
update-flag-deps:
    for svc in $(grep -rl 'inf-k8s' */Cargo.lock | xargs -rn1 dirname | sort -u); do \
        echo "==> $svc"; \
        cargo update --manifest-path "$svc/Cargo.toml" \
            -p feature-flag-proto -p feature-flag-engine \
            -p feature-flag-client -p openfeature-provider; \
    done
