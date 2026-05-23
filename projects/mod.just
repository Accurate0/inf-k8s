# List available recipes
[private]
default:
    @just --list projects

# Apply a single ArgoCD project application manifest
apply project:
    kubectl apply -f {{project}}/application.yaml

# Apply all ArgoCD project application manifests
apply-all:
    for f in */application.yaml; do kubectl apply -f "$f"; done

# Delete an ArgoCD project application
delete project:
    kubectl delete -f {{project}}/application.yaml

# Show diff for a project application before applying
diff project:
    kubectl diff -f {{project}}/application.yaml

# Diff a project's kustomize manifests
diff-manifests project:
    kubectl diff -k {{project}}/manifests

# Show ArgoCD applications status
status:
    kubectl get applications -n argocd

# Sync an ArgoCD project application
sync project:
    kubectl -n argocd patch application {{project}} --type merge -p '{"operation":{"initiatedBy":{"username":"just"},"sync":{"prune":true}}}'
