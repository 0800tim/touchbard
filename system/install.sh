#!/bin/bash
# Install touchbard in place of tiny-dfr. Run with sudo.
# Rolls back to tiny-dfr automatically if touchbard doesn't come up.
set -euo pipefail
[[ $EUID -eq 0 ]] || { echo "Run with sudo" >&2; exit 1; }
here="$(cd "$(dirname "$0")/.." && pwd)"

install -Dm755 "${TOUCHBARD_BIN:-$here/daemon/target/release/touchbard}" /usr/local/bin/touchbard
install -Dm755 "$here/agent/touchbar-agent" /usr/local/bin/touchbar-agent
install -Dm644 "$here/share/touchbar/config.toml" /usr/local/share/touchbar/config.toml
install -Dm644 "$here/README.md" /usr/local/share/touchbar/README.md
rm -rf /usr/local/share/touchbar/plugins
mkdir -p /usr/local/share/touchbar/plugins
for d in "$here"/plugins/*/; do
  name=$(basename "$d")
  install -d "/usr/local/share/touchbar/plugins/$name"
  find "$d" -maxdepth 1 -type f \( -name '*.py' -o -name '*.toml' -o -name '*.c' -o -name '*.md' \) \
    -exec install -m644 {} "/usr/local/share/touchbar/plugins/$name/" \;
  chmod 755 /usr/local/share/touchbar/plugins/"$name"/*.py 2>/dev/null || true
done
install -Dm644 "$here/system/touchbard.service" /etc/systemd/system/touchbard.service
install -Dm644 "$here/system/99-touchbard.rules" /etc/udev/rules.d/99-touchbard.rules

systemctl daemon-reload
udevadm control --reload
systemctl stop tiny-dfr.service || true
systemctl mask tiny-dfr.service
systemctl start touchbard.service

for _ in $(seq 20); do
  sleep 0.5
  if journalctl -u touchbard --since "-15s" --no-pager -q | grep -q "panel ready"; then
    echo "touchbard is running."
    exit 0
  fi
done

echo "touchbard did not come up; restoring tiny-dfr." >&2
journalctl -u touchbard --since "-30s" --no-pager -q | tail -20 >&2
systemctl stop touchbard.service || true
systemctl unmask tiny-dfr.service
systemctl start tiny-dfr.service
exit 1
