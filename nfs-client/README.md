# NFS client — NAS media share

Mounts the Unraid NAS export (`/mnt/user/media`, see
[`ansible/local`](../ansible/local) → `roles/proxmox-nfs`) on this machine as an
on-demand **systemd automount**. The share is only mounted on first access and
unmounts after 10 min idle, so a rebooting/offline NAS never blocks boot.

Note this is a *separate* mount from the one the media stack uses. The LXC
guests never mount NFS themselves — the Proxmox host mounts once at
`/mnt/pve/unraid` and bind-mounts it in. This unit is only for reaching the
share directly from a workstation.

## Export rule

The NAS only answers clients its export rule names, so this machine has to be
added on the Unraid side (Shares → `media` → NFS Security Settings) alongside
the Proxmox host:

```
10.0.2.249/32(sec=sys,rw,no_root_squash,no_subtree_check,fsid=1,anonuid=99,anongid=100)
192.168.0.0/24(sec=sys,rw,no_subtree_check,fsid=1,anonuid=99,anongid=100)
```

Two things to keep straight:

- **`fsid=1` must match across every rule for this export.** It pins the NFS
  filesystem id so file handles survive an array stop/start. Unraid's
  `/mnt/user` is a FUSE overlay (shfs) with an anonymous device number that
  changes on restart; without a fixed `fsid` every client gets `ESTALE` and has
  to remount. Never use `fsid=0` — in NFSv4 that designates the pseudo-root and
  would change the export path.
- **No `no_root_squash` for the workstation rule.** The Proxmox host needs it
  so Ansible can chown the media dirs to the container-offset uids; a
  workstation does not, and it is worth withholding.

Files land as uid/gid `101000` (the unprivileged-LXC offset for `media`), so
they will show as an unmapped numeric owner here. That is expected — read
access works via the `0775`/`0664` group bits.

## Install

```sh
just nfs install     # install + enable the automount
just nfs status      # show unit + mount state
just nfs uninstall   # remove it
```

Defaults: server `nas.internal` (`10.0.2.25`), export `/mnt/user/media`,
mounted at `/nfs/nas/media` (created automatically by `install`) — the path
reads `/nfs/<server>/<share>`, so a second share or a second NAS slots in
alongside without renaming anything. Override via env vars:

```sh
MEDIA_NFS_SERVER=nas.internal \
MEDIA_NFS_EXPORT=/mnt/user/media \
MEDIA_NFS_MOUNTPOINT=/nfs/nas/media \
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
nas.internal:/mnt/user/media  /nfs/nas/media  nfs  rw,vers=4.2,_netdev,nofail,noauto,x-systemd.automount,x-systemd.idle-timeout=600,noatime,hard,timeo=600,retrans=2,nconnect=4  0 0
```
