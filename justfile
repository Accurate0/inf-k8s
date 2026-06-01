# Kubernetes infrastructure management

set shell := ["bash", "-euo", "pipefail", "-c"]

mod system   'system-components/mod.just'
mod platform 'platform-services/mod.just'
mod projects 'projects/mod.just'
mod ansible  'ansible/mod.just'
mod tf       'terraform/mod.just'
mod cluster  'charts/mod.just'
mod ops      'scripts/mod.just'

# List available recipes
default:
    @just --list --list-submodules
