#!/usr/bin/env bash
# Creates and configures a sabnzbd LXC on Proxmox VE. Run on the host as root.
set -euo pipefail
VMID="${1:?Usage: $0 <vmid> [options]}"
# TODO: pct create / config / install sabnzbd. Mirror jellyfin/lxc/provision.sh.
echo "[provision] sabnzbd LXC $VMID — not yet implemented"
