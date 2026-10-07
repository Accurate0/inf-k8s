# List available recipes
[private]
default:
    @just --list selfhosted

# Import an Amex activity CSV into Firefly III
[no-cd]
firefly-import-amex csv:
    #!/usr/bin/env bash
    set -euo pipefail

    test -f "{{csv}}"

    importer() { kubectl -n firefly exec "$@" deploy/firefly-importer --; }
    trap 'importer rm -f /var/www/html/storage/uploads/amex.csv' EXIT

    importer -i sh -c 'cat > /var/www/html/storage/uploads/amex.csv' < "{{csv}}"
    importer php artisan importer:import /var/www/html/storage/configurations/amex-au.json /var/www/html/storage/uploads/amex.csv
