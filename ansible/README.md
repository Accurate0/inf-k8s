# Ansible

Provisions and maintains the inf-k8s nodes: base OS setup, Tailscale, firewall, k3s cluster, and the HAProxy edge.

## Layout

```
ansible/
├── ansible.cfg
├── inventory.yaml
├── group_vars/all.yml
├── playbooks/
│   ├── cluster.yaml       # full provision: base → tailscale → k3s → proxy
│   ├── k3s-upgrade.yaml   # rolling k3s channel/version bump
│   ├── proxy-only.yaml    # re-apply HAProxy edge
│   └── upgrade-all.yaml   # apt safe-upgrade across all hosts
└── roles/
    ├── base/        # apt packages, janitor user + sudoers, sshd hardening, ip-forward sysctl
    ├── tailscale/   # install + join (toggle with tailscale_install / tailscale_join_network)
    ├── firewall/    # nftables: https, cni, tailscale, binarylane vpc
    ├── k3s/         # preflight → multipathd (longhorn) → install → config
    └── edge-proxy/  # HAProxy + sshd port config for the edge node
```

`base` is run on every host. `k3s` and `firewall` apply to cluster nodes; `proxy` only to the edge.

## Environment variables

Playbooks read these via `lookup('env', ...)`. Export them before running (e.g. via direnv / a `.envrc` not in git).

| Variable                  | Used in                          | Purpose                                              |
| ------------------------- | -------------------------------- | ---------------------------------------------------- |
| `TAILSCALE_K8S_AUTH_KEY`  | `cluster.yaml`, `k3s-upgrade.yaml` | Tailscale auth key for the k8s tailnet (reusable, tagged) |
| `K3S_CLUSTER_TOKEN`       | `cluster.yaml`, `k3s-upgrade.yaml` | Shared k3s cluster token for server/agent join     |

Both are secrets — do not commit, do not echo. They're passed straight through to roles and end up in node config (`/etc/rancher/k3s/config.yaml`, Tailscale state).

## Running

The repo's `justfile` wraps the common invocations:

```sh
just ansible run cluster all        # full provision
just ansible run k3s-upgrade all    # rolling k3s upgrade
just ansible run proxy-only proxy   # re-apply edge
just ansible run upgrade-all all    # apt safe-upgrade
just ansible ping all               # connectivity check
```

Direct invocation works too: `ansible-playbook playbooks/cluster.yaml -l <group>`.
