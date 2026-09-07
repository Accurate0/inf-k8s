[private]
default:
    @just --list nfs

dir := source_directory()

# NFS server, and the export/mountpoint roots the shares hang off.
server     := env_var_or_default("NFS_SERVER", "nas.internal")
export_dir := env_var_or_default("NFS_EXPORT_DIR", "/mnt/user")
mount_dir  := env_var_or_default("NFS_MOUNT_DIR", "/nfs/nas")

# Shares to install when a recipe is called without a name.
shares := env_var_or_default("NFS_SHARES", "media storage")

# Mountpoints from earlier revisions, removed by `install` so a rename does not
# leave a second automount pointing at the old server.
legacy_mountpoints := "/media/nfs/media /media/nfs/nas"

# Install & enable the NFS shares as on-demand systemd automounts
install *names:
    #!/usr/bin/env bash
    set -euo pipefail
    command -v mount.nfs4 >/dev/null 2>&1 || { echo "nfs-utils not installed (Arch: sudo pacman -S nfs-utils)"; exit 1; }
    for old in {{legacy_mountpoints}}; do
        oldunit="$(systemd-escape -p --suffix=mount "$old")"
        oldauto="$(systemd-escape -p --suffix=automount "$old")"
        if [ -e "/etc/systemd/system/$oldunit" ] || [ -e "/etc/systemd/system/$oldauto" ]; then
            echo "Removing legacy mount at $old"
            sudo systemctl disable --now "$oldauto" "$oldunit" 2>/dev/null || true
            sudo rm -f "/etc/systemd/system/$oldunit" "/etc/systemd/system/$oldauto"
            sudo rmdir "$old" 2>/dev/null || true
        fi
    done
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    for share in {{ if names == "" { shares } else { names } }}; do
        export="{{export_dir}}/$share"
        mountpoint="{{mount_dir}}/$share"
        unit="$(systemd-escape -p --suffix=mount "$mountpoint")"
        auto="$(systemd-escape -p --suffix=automount "$mountpoint")"
        sed -e 's#@SERVER@#{{server}}#g' -e "s#@EXPORT@#$export#g" \
            -e "s#@SHARE@#$share#g" -e "s#@MOUNTPOINT@#$mountpoint#g" \
            -e 's#@REPO@#{{justfile_directory()}}#g' \
            "{{dir}}/nas.mount.tmpl" > "$tmp/$unit"
        sed -e "s#@SHARE@#$share#g" -e "s#@MOUNTPOINT@#$mountpoint#g" \
            "{{dir}}/nas.automount.tmpl" > "$tmp/$auto"
        sudo mkdir -p "$mountpoint"
        sudo install -m0644 "$tmp/$unit" "/etc/systemd/system/$unit"
        sudo install -m0644 "$tmp/$auto" "/etc/systemd/system/$auto"
        sudo systemctl daemon-reload
        sudo systemctl enable --now "$auto"
        echo "Mounted on demand at $mountpoint (server {{server}}:$export)"
    done

# Uninstall the NFS mount/automount units
uninstall *names:
    #!/usr/bin/env bash
    set -euo pipefail
    for share in {{ if names == "" { shares } else { names } }}; do
        mountpoint="{{mount_dir}}/$share"
        unit="$(systemd-escape -p --suffix=mount "$mountpoint")"
        auto="$(systemd-escape -p --suffix=automount "$mountpoint")"
        sudo systemctl disable --now "$auto" "$unit" 2>/dev/null || true
        sudo rm -f "/etc/systemd/system/$unit" "/etc/systemd/system/$auto"
        sudo systemctl daemon-reload
        echo "Removed NFS units for $mountpoint"
    done

# Show status of the NFS mounts
status *names:
    #!/usr/bin/env bash
    set -euo pipefail
    for share in {{ if names == "" { shares } else { names } }}; do
        mountpoint="{{mount_dir}}/$share"
        auto="$(systemd-escape -p --suffix=automount "$mountpoint")"
        systemctl status "$auto" --no-pager || true
        findmnt "$mountpoint" || echo "not currently mounted (will auto-mount on first access)"
    done
