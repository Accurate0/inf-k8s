[private]
default:
    @just --list nfs

dir := source_directory()

# NFS server + export to mount, and where to mount it locally.
server     := env_var_or_default("MEDIA_NFS_SERVER", "media.internal")
export     := env_var_or_default("MEDIA_NFS_EXPORT", "/data/media")
mountpoint := env_var_or_default("MEDIA_NFS_MOUNTPOINT", "/media/nfs/media")

# Install & enable the media NFS share as an on-demand systemd automount
install:
    #!/usr/bin/env bash
    set -euo pipefail
    command -v mount.nfs4 >/dev/null 2>&1 || { echo "nfs-utils not installed (Arch: sudo pacman -S nfs-utils)"; exit 1; }
    unit="$(systemd-escape -p --suffix=mount "{{mountpoint}}")"
    auto="$(systemd-escape -p --suffix=automount "{{mountpoint}}")"
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    sed -e 's#@SERVER@#{{server}}#g' -e 's#@EXPORT@#{{export}}#g' \
        -e 's#@MOUNTPOINT@#{{mountpoint}}#g' -e 's#@REPO@#{{justfile_directory()}}#g' \
        "{{dir}}/media.mount.tmpl" > "$tmp/$unit"
    sed -e 's#@MOUNTPOINT@#{{mountpoint}}#g' \
        "{{dir}}/media.automount.tmpl" > "$tmp/$auto"
    sudo mkdir -p "{{mountpoint}}"
    sudo install -m0644 "$tmp/$unit" "/etc/systemd/system/$unit"
    sudo install -m0644 "$tmp/$auto" "/etc/systemd/system/$auto"
    sudo systemctl daemon-reload
    sudo systemctl enable --now "$auto"
    echo "Mounted on demand at {{mountpoint}} (server {{server}}:{{export}})"

# Uninstall the media NFS mount/automount units
uninstall:
    #!/usr/bin/env bash
    set -euo pipefail
    unit="$(systemd-escape -p --suffix=mount "{{mountpoint}}")"
    auto="$(systemd-escape -p --suffix=automount "{{mountpoint}}")"
    sudo systemctl disable --now "$auto" "$unit" 2>/dev/null || true
    sudo rm -f "/etc/systemd/system/$unit" "/etc/systemd/system/$auto"
    sudo systemctl daemon-reload
    echo "Removed NFS units for {{mountpoint}}"

# Show status of the media NFS mount
status:
    #!/usr/bin/env bash
    set -euo pipefail
    auto="$(systemd-escape -p --suffix=automount "{{mountpoint}}")"
    systemctl status "$auto" --no-pager || true
    findmnt "{{mountpoint}}" || echo "not currently mounted (will auto-mount on first access)"
