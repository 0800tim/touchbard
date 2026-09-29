#!/bin/bash
# Go back to stock tiny-dfr. Run with sudo.
set -euo pipefail
[[ $EUID -eq 0 ]] || { echo "Run with sudo" >&2; exit 1; }
systemctl stop touchbard.service || true
rm -f /etc/systemd/system/touchbard.service /etc/udev/rules.d/99-touchbard.rules
rm -f /usr/local/bin/touchbard /usr/local/bin/touchbar-agent
rm -rf /usr/local/share/touchbar /var/lib/touchbard
systemctl daemon-reload
udevadm control --reload
systemctl unmask tiny-dfr.service
systemctl start tiny-dfr.service
echo "Back on tiny-dfr. Also run (as you): systemctl --user disable --now touchbar-agent"
