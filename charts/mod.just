[private]
default:
    @just --list cluster

# Show cluster nodes
nodes:
    kubectl get nodes -o wide

# Show all pods
pods ns="--all-namespaces":
    kubectl get pods {{ns}}

# Template a local helm chart
helm-template chart:
    helm template charts/{{chart}}
