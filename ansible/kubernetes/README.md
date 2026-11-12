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
│   ├── edge-proxy.yaml    # re-apply HAProxy edge
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
| `TAILSCALE_K8S_AUTH_KEY`  | `cluster.yaml`                     | Tailscale auth key for the k8s tailnet (reusable, tagged) |
| `K3S_CLUSTER_TOKEN`       | `cluster.yaml`, `k3s-upgrade.yaml` | Shared k3s cluster token for server/agent join     |
| `HARBOR_NODE_PULL_PASSWORD` | `cluster.yaml`, `k3s-upgrade.yaml` | Harbor `robot$node-pull` secret for the ghcr/docker.io/quay.io pull-through mirrors (`ortzi secrets get harbor/node-pull PASSWORD --reveal`) |
| `EDGE_PROXY_TRUSTED_IP`   | `cluster.yaml`, `edge-proxy.yaml`  | Comma-separated source IPs exempt from the HAProxy connection limits |

`TAILSCALE_K8S_AUTH_KEY` and `K3S_CLUSTER_TOKEN` are secrets — do not commit, do not echo. They're passed straight through to roles and end up in node config (`/etc/rancher/k3s/config.yaml`, Tailscale state). `EDGE_PROXY_TRUSTED_IP` is not a secret, but it lives here because the addresses change with the ISP lease.

## Running

The repo's `justfile` wraps the common invocations:

```sh
just ansible run cluster all        # full provision
just ansible run k3s-upgrade all    # rolling k3s upgrade
just ansible run edge-proxy proxy   # re-apply edge
just ansible run upgrade-all all    # apt safe-upgrade
just ansible ping all               # connectivity check
```

Direct invocation works too: `ansible-playbook playbooks/cluster.yaml -l <group>`.

## Restoring etcd

k3s uploads etcd snapshots to the `k3s-etcd-backup` R2 bucket every 6 hours and keeps 28 (`platform-services/etcd-backup`). The server token is in the `k3s-server-token` secret in the `k8s-shared-vault` Key Vault.

The S3 config secret can't be used during a restore, and OpenBao may be gone, so mint a temporary R2 token for the bucket in the Cloudflare dashboard. Then on the server:

```sh
systemctl stop k3s
k3s server --cluster-reset \
  --cluster-reset-restore-path=<snapshot-file-name> \
  --token=<server-token> \
  --etcd-s3 \
  --etcd-s3-endpoint=<account-id>.r2.cloudflarestorage.com \
  --etcd-s3-region=auto \
  --etcd-s3-bucket=k3s-etcd-backup \
  --etcd-s3-access-key=<access-key> \
  --etcd-s3-secret-key=<secret-key>
systemctl start k3s
```

Afterwards, delete any nodes in the snapshot that no longer exist.
