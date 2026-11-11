#!/bin/bash
# Installs the safe host updater on a Linux computer that runs sirus-host (ADR-206).
# Run as root from a checkout: sudo scripts/vps/install-updater.sh
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
install -m 755 "$HERE/sirus-host-update" /usr/local/sbin/sirus-host-update
cat > /etc/systemd/system/sirus-host-update.service <<'UNIT'
[Unit]
Description=Update sirus-host from GitHub main (safe install with rollback, ADR-206)
[Service]
Type=oneshot
ExecStart=/usr/local/sbin/sirus-host-update
UNIT
cat > /etc/systemd/system/sirus-host-update.timer <<'UNIT'
[Unit]
Description=Check for a new sirus-host every 10 minutes
[Timer]
OnBootSec=3min
OnUnitActiveSec=10min
[Install]
WantedBy=timers.target
UNIT
systemctl daemon-reload
systemctl enable --now sirus-host-update.timer
echo "Installed. Log: /var/log/sirus-host-update.log"
