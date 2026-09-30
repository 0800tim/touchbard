#!/bin/bash
# Go back to stock tiny-dfr. Run with sudo.
#
# Removes only files listed in the install manifest whose checksum still
# matches, so nothing touchbard didn't put there (or that you've changed
# since) is ever deleted.
set -euo pipefail
[[ $EUID -eq 0 ]] || { echo "Run with sudo" >&2; exit 1; }
share=/usr/local/share/touchbar
manifest=$share/installed.sha256

systemctl stop touchbard.service 2>/dev/null || true

if [[ -f $manifest ]]; then
  while read -r sum path; do
    [[ -f $path && ! -L $path ]] || continue
    if [[ $(sha256sum "$path" | cut -d' ' -f1) == "$sum" ]]; then
      rm -f "$path"
    else
      echo "Left in place (changed since install): $path" >&2
    fi
  done <"$manifest"
  rm -f "$manifest"
  find "$share" -depth -type d -empty -delete 2>/dev/null || true
else
  echo "No install manifest at $manifest, so no files were removed." >&2
  echo "Remove any leftover touchbard files by hand." >&2
fi
# The daemon's own state directory holds only its cached theme.
rm -f /var/lib/touchbard/theme.json
rmdir /var/lib/touchbard 2>/dev/null || true

systemctl daemon-reload
udevadm control --reload
systemctl unmask tiny-dfr.service
systemctl start tiny-dfr.service
echo "Back on tiny-dfr. Also run (as you): systemctl --user disable --now touchbar-agent"
