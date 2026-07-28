# NFS client — media server

Mounts the local media server's NFS export (`/data/media`, provisioned by the
`nfs` role in [`ansible/local`](../ansible/local)) on this machine as an
on-demand **systemd automount**. The share is only mounted on first access and
unmounts after 10 min idle, so a rebooting/offline media server never blocks
boot.

## Install

```sh
just nfs install     # install + enable the automount
just nfs status      # show unit + mount state
just nfs uninstall   # remove it
```

Defaults: server `media.internal`, export `/data/media`, mounted at
`/media/nfs/media` (created automatically by `install`).
Override via env vars:

```sh
MEDIA_NFS_SERVER=media.internal \
MEDIA_NFS_EXPORT=/data/media \
MEDIA_NFS_MOUNTPOINT=/media/nfs/media \
just nfs install
```

Requires `nfs-utils` (`sudo pacman -S nfs-utils`).

## fstab equivalent

Prefer `/etc/fstab`? The same mount as a one-liner:

```
media.internal:/data/media  /media/nfs/media  nfs  rw,vers=4,_netdev,nofail,noauto,x-systemd.automount,x-systemd.idle-timeout=600,noatime,soft,timeo=150,retrans=3  0 0
```
