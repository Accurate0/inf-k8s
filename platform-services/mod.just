# List available recipes
[private]
default:
    @just --list platform

# Apply a single platform service application manifest
apply service:
    kubectl apply -f {{service}}/application.yaml

# Apply all platform service application manifests
apply-all:
    kubectl apply -f */application.yaml

# Delete a platform service application
delete service:
    kubectl delete -f {{service}}/application.yaml

# Show diff for a service application before applying
diff service:
    kubectl diff -f {{service}}/application.yaml

# Diff a service's kustomize manifests
diff-manifests service:
    kubectl diff -k {{service}}/manifests

# Show ArgoCD applications status
status:
    kubectl get applications -n argocd

# Sync a platform service application
sync service:
    kubectl -n argocd patch application {{service}} --type merge -p '{"operation":{"initiatedBy":{"username":"just"},"sync":{"prune":true}}}'
