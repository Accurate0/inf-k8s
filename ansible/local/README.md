# Ansible — local

Provisions non-cluster home devices: the Proxmox host, its LXC guests running
the media stack behind an nginx reverse proxy, and the unraid storage VM.

## Layout

```
local/
├── ansible.cfg
├── inventory.yaml
├── group_vars/all.yml
├── playbooks/
│   ├── local.yaml           # everything: guests, unifi dns/clients, hosts
│   ├── proxmox-guests.yaml  # LXC guests + the unraid VM
│   └── unifi-dns.yaml       # dns records + client aliases
└── roles/
    ├── base/               # packages, janitor user + sudoers, sshd hardening
    ├── docker/             # docker engine + compose
    ├── jellyfin/ arr/ ...  # the media stack, one role per app
    ├── proxmox-nfs/        # unraid media share as proxmox-managed storage
    └── proxmox-*/ pbs/     # proxmox host config and backups
```

## The k3s guest

`k8s-pve-1` (vmid 210, `10.0.2.18`) is the one guest this tree creates but does not
configure. `proxmox-guests.yaml` builds it — Ubuntu 24.04 rather than the Debian
default, matching the rest of the cluster's nodes, with `swap: 0` because the
kubelet refuses to start otherwise, and `keyctl=1`/`fuse=1` on top of the usual
`nesting=1` so containerd works in an unprivileged container. Everything after that
belongs to `ansible/kubernetes`: it is an entry in that tree's `agent` group and is
configured by its `playbooks/cluster.yaml`. Deliberately no roles here, and no place
in the `local.yaml` guest pipeline.

It is excluded from the nightly PBS job (`proxmox_pbs_exclude_vmids`) on purpose —
it holds no state worth restoring, and is rebuilt from `proxmox-guests.yaml` plus
`cluster.yaml`.

## Storage

Bulk storage lives on **unraid**, a VM on the Proxmox host at `10.0.2.25`
(`nas.internal`). Unraid manages its own network config — set the static
address in its webgui after first boot.

The SAS HBAs are handed to it whole. `roles/proxmox-vfio` blacklists `mpt3sas`
on the host and binds the controllers to `vfio-pci` by PCI id
(`proxmox_vfio_ids`), so the host never enumerates the drives behind them and
Unraid drives the controllers directly — real models, serials, and SMART data,
no QEMU layer in between. This replaced the earlier per-disk `scsiblock=1`
passthrough, which was only necessary while the media drives shared the chipset
SATA controller with the `rpool` boot SSD; the add-in HBAs are on their own
IOMMU groups, so the host keeps its root disk.

`intel_iommu=on,sp_off` on the kernel command line is load-bearing: without
`sp_off` this platform's IOMMU superpage support corrupts the mappings and the
HBAs fail under the guest.

It uses **internal boot** (7.3+): the VM boots from `scsi0`, a virtual disk on
`local-zfs`, rather than reading the OS off the flash every boot. First boot
still comes off the flash — `boot: order=scsi0;usb0` falls through while
`scsi0` is empty — then *Settings → Onboarding Wizard → internal boot* copies
the config across.

The USB flash still has to stay attached, so `UNRAID_USB_ID` (the stick's
`vendor:product` id from `lsusb` on `pve`) remains required. Boot method and
licensing are separate choices, and the TPM-based licensing that would let you
pull the stick is bare-metal only — a VM cannot use a vTPM for it, so this VM
stays on flash licensing with the stick as a licence anchor only. Back up
`/boot` from within Unraid: it holds the array config, and PBS backups of the
VM will not capture the passed-through flash.

The old mergerfs pool is gone — its member drives were released to Unraid, and
the role that retired it has been removed now that the migration is done.

The share comes back from Unraid over NFS, mounted by the Proxmox host rather
than by the guests — unprivileged LXC cannot mount NFS in its own namespace, so
the host mounts once and the guests keep their existing `mp0` bind. `roles/
proxmox-nfs` registers it as Proxmox-managed storage (`pvesm add nfs`), so
Proxmox owns the mount, reconnects it, and surfaces its state in the GUI
instead of it being a hand-written fstab line. It mounts at
`/mnt/pve/<proxmox_nfs_id>` with `vers=4.2,hard,noatime`, and sets `mkdir 0`
and `create-subdirs 0` so Proxmox never writes its own `dump/`, `images/` or
`template/` directories into the media share.

The role no-ops with a message until the export actually exists, so it is safe
to run before the array is built. `proxmox_media_source` selects what the
guests bind; it points at `/mnt/pve/unraid`.

The `mp0` binds deliberately carry no `idmap=passthrough`. Idmapped mounts are
a filesystem-level kernel feature and NFS does not implement them, so the LXC
mount hook rejects the bind with `EINVAL` (`run_buffer: Script exited with
status 22`) and the container will not start. Without idmap the guests use the
standard unprivileged offset, so their UID 1000 lands on the share as 101000 —
which is what the media directories must be owned by:

```sh
chown -R 101000:101000 /mnt/pve/unraid/{downloads,tv,movies}
```

Inside the containers those directories then read as `1000:media`, matching
`media_puid`/`media_pgid`. Keep downloads and the library on this one share so
the *arr apps can hardlink imports instead of copying them.

## Metrics

The Proxmox host exports to the cluster's Prometheus. Two exporters run on `pve`,
installed by `roles/proxmox-node-exporter` and `roles/proxmox-pve-exporter`:

| Port | Exporter | What it covers |
|---|---|---|
| 9100 | `prometheus-node-exporter` | CPU, memory, ZFS ARC, disk IO, filesystems, hwmon temperatures, and SMART via the `smartmon` textfile collector |
| 9221 | `prometheus-pve-exporter` | The PVE API — guest state, per-storage usage, node and cluster status |

`prometheus-pve-exporter` is not packaged for Debian, so the role installs it
from PyPI into a virtualenv at `/opt/prometheus-pve-exporter` — pinned by
`proxmox_pve_exporter_version` — and ships its own systemd unit. The node
exporter is a plain Debian package.

Both bind to the host's LAN address, not `0.0.0.0`. Prometheus runs on
`k8s-pve-1`, an LXC on this same host and subnet, so the scrape is a plain LAN
hop — no Tailscale, unlike the haproxy job. The two jobs live in
`additionalScrapeConfigs` in `system-components/monitoring.application.yaml`, and
`system-components/monitoring/proxmox-dashboard.configmap.yaml` provisions the
Grafana dashboard.

`proxmox-pve-exporter` issues its own read-only API token (`prometheus@pve`,
`PVEAuditor`) the first time it runs and writes it to `/etc/prometheus/pve.yml`.
The secret is only returned at creation, so the role keys off that file: delete
it and the next run reissues the token. Nothing needs to be passed in through
the environment.

## Dependencies

Requires these collections on the control machine:

```sh
ansible-galaxy collection install community.general ansible.posix community.docker
```

## Environment

`playbooks/local.yaml` reads secrets from the environment:

| Variable | Used by |
|---|---|
| `PROMOX_API_KEY`, `PROXMOX_PASSWORD` | `playbooks/proxmox-guests.yaml` |
| `UNRAID_USB_ID` | `playbooks/proxmox-guests.yaml` |
| `UNIFI_API_KEY` | `playbooks/unifi-dns.yaml` |
| `SONARR_API_KEY`, `RADARR_API_KEY` | `roles/recyclarr` |
| `PVE_OIDC_CLIENT_SECRET` | `roles/proxmox-oidc` |

The Proxmox OIDC client secret is generated by kanidm-sync in the cluster:

```sh
export PVE_OIDC_CLIENT_SECRET=$(kubectl -n kanidm get secret kanidm-pve-oidc \
  -o jsonpath='{.data.clientSecret}' | base64 -d)
```

## Running

```sh
just ansible local all                        # everything
just ansible local run proxmox-guests proxmox # guests + the unraid VM
just ansible local ping media_servers         # connectivity check
```

Direct invocation works too: `ansible-playbook playbooks/local.yaml`.
