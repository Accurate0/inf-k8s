# Kubernetes infrastructure management

set shell := ["bash", "-euo", "pipefail", "-c"]

mod system   'system-components/mod.just'
mod platform 'platform-services/mod.just'
mod projects 'projects/mod.just'
mod selfhosted 'selfhosted/mod.just'
mod ansible  'ansible/mod.just'
mod tf       'terraform/mod.just'
mod cluster  'charts/mod.just'
mod ops      'scripts/mod.just'
mod nfs      'nfs-client/mod.just'

# List available recipes
default:
    @just --list --list-submodules

# Remove the target dir of every cargo workspace
clean-cargo:
    git ls-files '*Cargo.lock' | xargs -n1 dirname | xargs -I{} cargo clean --manifest-path {}/Cargo.toml
