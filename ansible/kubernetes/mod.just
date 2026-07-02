[private]
default:
    @just --list ansible::kubernetes

ansible_dir := justfile_directory()

# Run an ansible playbook (e.g. just ansible kubernetes run cluster all)
run playbook group:
    cd {{ansible_dir}} && ansible-playbook playbooks/{{playbook}}.yaml -l {{group}}

# Run an ansible role against a host group
role role group:
    cd {{ansible_dir}} && ansible {{group}} -m include_role -a "name={{role}}"

# Run an ad-hoc ansible command against a host group
cmd cmd group:
    cd {{ansible_dir}} && ansible {{group}} -a "{{cmd}}"

# Ping all ansible hosts
ping group:
    cd {{ansible_dir}} && ansible {{group}} -m ping
