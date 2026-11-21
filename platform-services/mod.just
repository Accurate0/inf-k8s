# List available recipes
[private]
default:
    @just --list platform

# Diff a service's kustomize manifests
diff-manifests service:
    kubectl diff -k {{service}}/manifests

# Install ortzi to ~/.local/bin; afterwards it updates itself with `ortzi self-update`
install:
    cargo build --locked --release --manifest-path ../crates/ortzi/Cargo.toml
    mkdir -p ~/.local/bin && install -m 755 ../crates/ortzi/target/release/ortzi ~/.local/bin/ortzi

# Run ortzi from source
ortzi *args:
    cargo run --manifest-path ../crates/ortzi/Cargo.toml -- {{args}}

# Bump the inf-k8s feature-flag crates in every consumer service's Cargo.lock
update-flag-deps:
    for svc in $(grep -rl 'inf-k8s' */Cargo.lock | xargs -rn1 dirname | sort -u); do \
        echo "==> $svc"; \
        cargo update --manifest-path "$svc/Cargo.toml" \
            -p feature-flag-proto -p feature-flag-engine \
            -p feature-flag-client -p openfeature-provider; \
    done
