[private]
default:
    @just --list charts

# Template a local helm chart
helm-template chart:
    helm template charts/{{chart}}
