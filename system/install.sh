#!/bin/bash
# Install touchbard in place of tiny-dfr. Run with sudo.
#
# Every file placed is recorded with its checksum in a root-owned manifest.
# An existing file at a target path is only replaced if the manifest says we
# put it there (and it's unmodified), or, for installs made before the
# manifest existed, if it carries touchbard's own marker text. Anything else
# stops the install before a single file changes.
#
# Rolls back to tiny-dfr automatically if touchbard doesn't come up.
set -euo pipefail
[[ $EUID -eq 0 ]] || { echo "Run with sudo" >&2; exit 1; }
here="$(cd "$(dirname "$0")/.." && pwd)"
bin="${TOUCHBARD_BIN:-$here/daemon/target/release/touchbard}"
share=/usr/local/share/touchbar
manifest=$share/installed.sha256
[[ -x $bin ]] || { echo "Build first: (cd daemon && cargo build --release)" >&2; exit 1; }

# source|target|mode for everything we install.
plan=(
  "$bin|/usr/local/bin/touchbard|755"
  "$here/agent/touchbar-agent|/usr/local/bin/touchbar-agent|755"
  "$here/share/touchbar/config.toml|$share/config.toml|644"
  "$here/README.md|$share/README.md|644"
  "$here/system/touchbard.service|/etc/systemd/system/touchbard.service|644"
  "$here/system/99-touchbard.rules|/etc/udev/rules.d/99-touchbard.rules|644"
)
for f in "$here"/plugins/*/*; do
  [[ -f $f ]] || continue
  case $f in *.py) mode=755 ;; *.toml | *.c | *.md) mode=644 ;; *) continue ;; esac
  rel=${f#"$here"/plugins/}
  plan+=("$f|$share/plugins/$rel|$mode")
done

# Every source must exist before anything is touched.
for entry in "${plan[@]}"; do
  src=${entry%%|*}
  [[ -f $src ]] || { echo "Missing $src; nothing installed." >&2; exit 1; }
done

declare -A recorded=()
if [[ -f $manifest ]]; then
  while read -r sum path; do recorded[$path]=$sum; done <"$manifest"
fi

# Is the file at $1 ours to replace or remove? ($2: the file we'd install there.)
ours() {
  local path=$1 src=${2:-}
  [[ -e $path || -L $path ]] || return 0
  [[ -f $path && ! -L $path ]] || return 1
  # Identical to what we'd write: replacing it changes nothing.
  [[ -n $src ]] && cmp -s "$src" "$path" && return 0
  if [[ -n ${recorded[$path]:-} ]]; then
    [[ $(sha256sum "$path" | cut -d' ' -f1) == "${recorded[$path]}" ]]
    return
  fi
  # No manifest entry: adopt only a pre-manifest touchbard install.
  [[ ! -f $manifest ]] && grep -aqE 'touchbard|touchbar-agent|Touch Bar' "$path"
}

conflicts=()
for entry in "${plan[@]}"; do
  IFS='|' read -r src target _ <<<"$entry"
  ours "$target" "$src" || conflicts+=("$target")
done
# Old files we installed that this version no longer ships.
stale=()
for path in "${!recorded[@]}"; do
  wanted=0
  for entry in "${plan[@]}"; do [[ ${entry#*|} == "$path|"* ]] && wanted=1 && break; done
  (( wanted )) || stale+=("$path")
done
if (( ${#conflicts[@]} )); then
  echo "Not installing: these files exist and weren't installed by touchbard (or were changed since):" >&2
  printf '  %s\n' "${conflicts[@]}" >&2
  echo "Move them aside and run this again." >&2
  exit 1
fi

new_manifest=$(mktemp)
for entry in "${plan[@]}"; do
  IFS='|' read -r src target mode <<<"$entry"
  install -Dm"$mode" "$src" "$target"
  printf '%s  %s\n' "$(sha256sum "$target" | cut -d' ' -f1)" "$target" >>"$new_manifest"
done
for path in "${stale[@]}"; do
  if ours "$path"; then rm -f "$path"; else echo "Left in place (changed since install): $path" >&2; fi
done
find "$share/plugins" -mindepth 1 -type d -empty -delete 2>/dev/null || true
install -Dm644 "$new_manifest" "$manifest"
rm -f "$new_manifest"

systemctl daemon-reload
udevadm control --reload
systemctl stop tiny-dfr.service || true
systemctl mask tiny-dfr.service
systemctl restart touchbard.service

for _ in $(seq 20); do
  sleep 0.5
  if journalctl -u touchbard --since "-15s" --no-pager -q | grep -q "panel ready"; then
    echo "touchbard is running."
    # The session agent is a user service: restart it too, or the old one
    # keeps running against the new daemon.
    if [[ -n ${SUDO_USER:-} ]] && systemctl --user -M "$SUDO_USER@" is-enabled touchbar-agent &>/dev/null; then
      systemctl --user -M "$SUDO_USER@" restart touchbar-agent && echo "touchbar-agent restarted."
    fi
    exit 0
  fi
done

echo "touchbard did not come up; restoring tiny-dfr." >&2
journalctl -u touchbard --since "-30s" --no-pager -q | tail -20 >&2
systemctl stop touchbard.service || true
systemctl unmask tiny-dfr.service
systemctl start tiny-dfr.service
exit 1
