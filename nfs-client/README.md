# NFS client — NAS shares

Mounts the Unraid NAS exports (`/mnt/user/media` and `/mnt/user/storage`, see
[`ansible/local`](../ansible/local) → `roles/proxmox-nfs`) on this machine as
on-demand **systemd automounts**. A share is only mounted on first access and
unmounts after 10 min idle, so a rebooting/offline NAS never blocks boot.

Note this is a *separate* mount from the one the media stack uses. The LXC
guests never mount NFS themselves — the Proxmox host mounts once at
`/mnt/pve/unraid` and bind-mounts it in. This unit is only for reaching the
share directly from a workstation.

## Export rule

The NAS only answers clients its export rule names, so this machine has to be
added on the Unraid side (Shares → `media` / `storage` → NFS Security
Settings) alongside the Proxmox host:

```
10.0.2.249/32(sec=sys,rw,all_squash,no_subtree_check,fsid=1,anonuid=99,anongid=100)
192.168.0.0/24(sec=sys,rw,all_squash,no_subtree_check,fsid=1,anonuid=99,anongid=100)
```

Three things to keep straight:

- **`fsid` must match across every rule for a given export, and differ
  between exports** (`media` uses `fsid=1`, `storage` `fsid=2`). It pins the NFS
  filesystem id so file handles survive an array stop/start. Unraid's
  `/mnt/user` is a FUSE overlay (shfs) with an anonymous device number that
  changes on restart; without a fixed `fsid` every client gets `ESTALE` and has
  to remount. Never use `fsid=0` — in NFSv4 that designates the pseudo-root and
  would change the export path.
- **`all_squash` has to be on every rule.** It maps every request, root
  included, to `anonuid`/`anongid` — Unraid's native `nobody:users` — so this
  workstation, the Proxmox host, and Unraid's own SMB all write as the same
  identity. Miss it on one rule and that client goes back to stamping its own
  uid on files, which is the split this replaced. See
  [`ansible/local`](../ansible/local) → *One identity for every writer*.
- **`no_root_squash` is gone.** It was on the Proxmox rule only so Ansible
  could chown the media dirs to the container-offset uids; `all_squash`
  supersedes it and removes the need.

Files land as `99:100` no matter who wrote them, so they show as an unmapped
numeric owner here. That is expected, and read *and write* both work: the
server checks access against the squashed credentials, which own the tree. The
one thing a client can no longer do is `chown` — run that on the NAS itself.

## Install

```sh
just nfs install           # install + enable every share's automount
just nfs install storage   # just one share
just nfs status            # show unit + mount state
just nfs uninstall         # remove them
```

Defaults: server `nas.internal` (`10.0.2.25`), shares `media` and `storage`,
exported from `/mnt/user/<share>` and mounted at `/nfs/nas/<share>` (created
automatically by `install`) — the path reads `/nfs/<server>/<share>`, so
another share or another NAS slots in alongside without renaming anything.
Override via env vars:

```sh
NFS_SERVER=nas.internal \
NFS_SHARES="media storage" \
NFS_EXPORT_DIR=/mnt/user \
NFS_MOUNT_DIR=/nfs/nas \
just nfs install
```

`install` removes the units for any previous mountpoint listed in
`legacy_mountpoints`, so upgrading from `/media/nfs/nas` will not leave a
second automount behind.

Requires `nfs-utils` (`sudo pacman -S nfs-utils`).

## Mount options

`rw,vers=4.2,_netdev,nofail,noatime,hard,timeo=600,retrans=2,nconnect=4`

- `hard` — a timeout retries forever instead of failing the syscall. On a `rw`
  mount `soft` can return short writes that silently truncate files, so the
  hang is the safer failure. `nofail` plus the automount already cover the
  "NAS is offline at boot" case, which is what `soft` would otherwise buy.
- `nconnect=4` — four TCP connections instead of one, which matters for
  multi-GB media reads. Matches what the Proxmox host uses.
- `vers=4.2` — same version the host mounts with; NFSv4 needs no rpcbind.

## fstab equivalent

Prefer `/etc/fstab`? The same mount as a one-liner:

```
nas.internal:/mnt/user/storage  /nfs/nas/storage  nfs  rw,vers=4.2,_netdev,nofail,noauto,x-systemd.automount,x-systemd.idle-timeout=600,noatime,hard,timeo=600,retrans=2,nconnect=4  0 0
```
