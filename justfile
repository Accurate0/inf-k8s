# Kubernetes infrastructure management

set shell := ["bash", "-euo", "pipefail", "-c"]

mod apps     'applications/mod.just'
mod ansible  'ansible/mod.just'
mod tf       'terraform/mod.just'
mod cluster  'charts/mod.just'

# List available recipes
default:
    @just --list --list-submodules
