[private]
default:
    @just --list ops

# Validate manifests against the self-hosted schemas (optionally scoped to paths)
validate *paths:
    ./validate-manifests.sh {{paths}}

# Show an ArgoCD health/drift report (use --unhealthy to filter)
argo-status *args:
    ./argo-status.sh {{args}}

# Run the cluster health preflight before reboots/upgrades
preflight *args:
    ./cluster-preflight.sh {{args}}
