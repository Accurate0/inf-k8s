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
    ├── storage-teardown/   # retires the mergerfs pool and its member drives
    └── proxmox-*/ pbs/     # proxmox host config and backups
```

## Storage

Bulk storage lives on **unraid**, a VM on the Proxmox host at `10.0.2.25`
(`nas.internal`). Unraid manages its own network config — set the static
address in its webgui after first boot.

The two media drives are handed to it whole, as SCSI passthrough devices listed
in `proxmox_unraid_disks` by their `/dev/disk/by-id` paths. They are attached
with `scsiblock=1`, which builds them as QEMU `scsi-block` devices instead of
the default emulated `scsi-hd`: SCSI commands go straight to the drive, so
Unraid reads the real model, serial, and SMART data (ATA pass-through reaches
the disk via the kernel's SAT layer) rather than QEMU's synthesised answers. Proxmox no longer mounts them:
`storage-teardown` drops the `/data/disks/*` mounts and fstab entries before the
guests play runs, which has to happen first — passing through a disk the host
still has mounted invites two writers on one filesystem.

This is disk-level passthrough, not controller passthrough. Handing the HBA to
the VM with vfio would cut the host out completely, but it is not possible here:
both media drives and the `rpool` boot SSD sit behind the one chipset SATA
controller (`0000:00:1f.2`), so passing it through would take Proxmox's own root
disk with it. That needs a separate add-in HBA for the media drives.

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

The old mergerfs pool is gone. `roles/storage-teardown` runs against the
Proxmox host first on every `local.yaml` run: it unmounts any `fuse.mergerfs`
mount, drops the fstab entry and pool unit, uninstalls the packages, and
releases the `/data/disks/*` member drives so they can go to Unraid.

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
