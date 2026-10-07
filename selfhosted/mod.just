# List available recipes
[private]
default:
    @just --list selfhosted

# Import an Amex activity CSV into Firefly III
[no-cd]
firefly-import-amex csv:
    test -f "{{csv}}"
    kubectl -n firefly exec -i deploy/firefly-importer -- sh -c 'cat > /var/www/html/storage/uploads/amex.csv' < "{{csv}}"
    kubectl -n firefly exec deploy/firefly-importer -- php artisan importer:import /var/www/html/storage/configurations/amex-au.json /var/www/html/storage/uploads/amex.csv
    kubectl -n firefly exec deploy/firefly-importer -- rm -f /var/www/html/storage/uploads/amex.csv
