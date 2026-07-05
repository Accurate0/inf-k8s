# Ansible — local

Provisions non-cluster home devices. The first is `media`, an Arch Linux media
server running its apps as Docker containers behind an nginx reverse proxy.

## Layout

```
local/
├── ansible.cfg
├── inventory.yaml
├── group_vars/all.yml
├── playbooks/
│   └── media.yaml        # base → storage → docker → media → nginx
└── roles/
    ├── base/       # pacman packages, janitor user + sudoers, sshd hardening
    ├── storage/    # mount media drives + pool them with mergerfs, dir tree
    ├── docker/     # docker engine + compose
    ├── media/      # docker compose stack (7 containers)
    └── nginx/      # reverse proxy for *.media.internal
```

## Storage

`media_drives` (in `inventory.yaml`) lists the physical media drives. Each is
mounted at `/data/disks/<name>`, then all are pooled with **mergerfs** into a
single `media_pool_mount` (`/data/media`) that the containers bind-mount. This
supports 1..N drives — a single-drive host just lists one entry. Use
`by-label`/`by-uuid` device paths so drive identification survives reboots.

## Dependencies

Requires these collections on the control machine:

```sh
ansible-galaxy collection install community.general ansible.posix community.docker
```

## Running

```sh
just ansible local run media media   # provision the media server
just ansible local ping media        # connectivity check
```

Direct invocation works too: `ansible-playbook playbooks/media.yaml -l media`.
