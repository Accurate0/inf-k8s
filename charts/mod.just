[private]
default:
    @just --list charts

# Template a local helm chart
helm-template chart:
    helm template charts/{{chart}}

# Lint all local charts against their values schemas
lint:
    for c in charts/*/; do helm lint "$c"; done
