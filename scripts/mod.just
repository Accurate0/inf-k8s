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

# Copy Infisical project(s) into one OpenBao KV secret at kv/<namespace>/<name> (needs BAO_TOKEN)
infisical-to-bao namespace name +sources:
    ./infisical-to-bao.sh {{namespace}} {{name}} {{sources}}

# Benchmark the Unraid array from pve and from here (--tier 2|3|all)
unraid-bench *args:
    ./unraid-bench.sh {{args}}
