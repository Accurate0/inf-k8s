[private]
default:
    @just --list ops

# Run the cluster health preflight before reboots/upgrades
preflight *args:
    ./cluster-preflight.sh {{args}}

# Benchmark the Unraid array from pve and from here (--tier 2|3|all)
unraid-bench *args:
    ./unraid-bench.sh {{args}}
