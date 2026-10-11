[private]
default:
    @just --list ansible::local

ansible_dir := source_directory()

# Run the whole local playbook: guests, unifi dns/clients, hosts
all *args:
    cd {{ansible_dir}} && ansible-playbook playbooks/local.yaml {{args}}

# Push the UniFi DNS records and client aliases only
unifi *args:
    cd {{ansible_dir}} && ansible-playbook playbooks/unifi-dns.yaml {{args}}

# Run an ansible playbook (e.g. just ansible local run media media)
run playbook group:
    cd {{ansible_dir}} && ansible-playbook playbooks/{{playbook}}.yaml -l {{group}},localhost

# Run an ansible role against a host group
role role group:
    cd {{ansible_dir}} && ansible {{group}} -m include_role -a "name={{role}}"

# Run an ad-hoc ansible command against a host group
cmd cmd group:
    cd {{ansible_dir}} && ansible {{group}} -a "{{cmd}}"

# Ping all ansible hosts
ping group:
    cd {{ansible_dir}} && ansible {{group}} -m ping

# Refresh the pacman mirrorlist with reflector, pacoloco cache first
mirrorlist mirror="http://mirror.internal/archlinux/$repo/os/$arch" *reflector_args="--country Australia --protocol https --latest 10 --sort rate":
    #!/usr/bin/env bash
    set -euo pipefail

    server='Server = {{mirror}}'
    list=/etc/pacman.d/mirrorlist

    if ! command -v reflector >/dev/null; then
        echo "reflector is not installed (pacman -S reflector)" >&2
        exit 1
    fi

    sudo cp "$list" "$list.bak"
    sudo reflector {{reflector_args}} --save "$list"

    if grep -qxF "$server" "$list"; then
        echo "$server already present in $list"
        exit 0
    fi

    printf '%s\n\n%s' "$server" "$(cat "$list")" | sudo tee "$list" >/dev/null

    echo "refreshed $list with $server first (backup at $list.bak)"
