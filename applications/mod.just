# List available recipes
[private]
default:
    @just --list apps

# Apply a single ArgoCD application manifest
apply app:
    kubectl apply -f applications/{{app}}.application.yaml

# Apply all ArgoCD application manifests
apply-all:
    kubectl apply -f applications/*.application.yaml

# Delete an ArgoCD application
delete app:
    kubectl delete -f applications/{{app}}.application.yaml

# Show diff for an application before applying
diff app:
    kubectl diff -f applications/{{app}}.application.yaml

# Diff a project's manifests
diff-project project:
    kubectl diff -k projects/{{project}}/manifests

# Show ArgoCD applications status
status:
    kubectl get applications -n argocd

# Sync an ArgoCD application
sync app:
    kubectl -n argocd patch application {{app}} --type merge -p '{"operation":{"initiatedBy":{"username":"just"},"sync":{"prune":true}}}'
