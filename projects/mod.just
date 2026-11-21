# List available recipes
[private]
default:
    @just --list projects

# Diff a project's kustomize manifests
diff-manifests project:
    kubectl diff -k {{project}}/manifests
