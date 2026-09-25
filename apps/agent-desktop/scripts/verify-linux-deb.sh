#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: verify-linux-deb.sh PATH_TO_DEB" >&2
  exit 2
fi

deb=$(realpath "$1")
[[ -f "$deb" ]] || { echo "package is not a regular file" >&2; exit 1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
dpkg-deb --extract "$deb" "$work/root"

[[ $(dpkg-deb --field "$deb" Package) == "enterprise-local-agent-desktop" ]] || { echo "unexpected Debian package name" >&2; exit 1; }
[[ $(dpkg-deb --field "$deb" Architecture) == "amd64" ]] || { echo "unexpected Debian architecture" >&2; exit 1; }

binary="$work/root/usr/bin/agent-desktop"
[[ -x "$binary" ]] || { echo "desktop executable is missing" >&2; exit 1; }
mapfile -t desktop_entries < <(find "$work/root/usr/share/applications" -maxdepth 1 -type f -name '*.desktop' -print)
[[ ${#desktop_entries[@]} -eq 1 ]] || { echo "expected exactly one desktop entry" >&2; exit 1; }
desktop-file-validate "${desktop_entries[0]}"
find "$work/root/usr/share/icons" -type f -name '*.png' -print -quit | grep -q . || { echo "package icon is missing" >&2; exit 1; }

depends=$(dpkg-deb --field "$deb" Depends)
grep -Eq 'libwebkit2gtk-4\.1|webkit2gtk-4\.1' <<<"$depends" || { echo "WebKitGTK runtime dependency is missing" >&2; exit 1; }
grep -Eq 'libgtk-3|gtk3' <<<"$depends" || { echo "GTK runtime dependency is missing" >&2; exit 1; }

if find "$work/root" \( -type f -o -type l \) \( \
  -name '*.map' -o -name '*.pdb' -o -name '*.pem' -o -name '*.key' -o \
  -name '.env' -o -name 'agent-service-daemon' -o -name 'agent-local-write-worker' -o \
  -name 'bwrap' \) -print -quit | grep -q .; then
  echo "package contains a prohibited debug, credential, service, or containment artifact" >&2
  exit 1
fi

if ldd "$binary" | grep -q 'not found'; then
  echo "desktop executable has unresolved runtime libraries on this verifier" >&2
  exit 1
fi

echo "Debian package verified: $(basename "$deb")"
