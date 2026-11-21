# List available recipes
[private]
default:
    @just --list flux

# Show every flux kustomization and helm release that is not ready (--all for everything)
status *args:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "{{args}}" = "--all" ]; then
        ortzi flux get all -A
    else
        ortzi flux get kustomizations -A --status-selector ready=false
        ortzi flux get helmreleases -A --status-selector ready=false
    fi

# Fetch the cluster artifact and reconcile an app now
sync app:
    ortzi flux reconcile kustomization {{app}} --with-source

# Stop flux from changing an app, or let it again
suspend app:
    ortzi app autosync {{app}} off --yes

resume app:
    ortzi app autosync {{app}} on --yes

# Recover when harbor is down and flux cannot pull its artifact: apply flux and the registry from this checkout
bootstrap:
    kustomize build flux-system | kubectl apply --server-side --force-conflicts -f -
    kustomize build registry | kubectl apply --server-side --force-conflicts -f -
