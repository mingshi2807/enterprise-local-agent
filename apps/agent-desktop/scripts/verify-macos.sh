#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]] || [[ $1 != "signed" && $1 != "unsigned" ]]; then
  echo "usage: verify-macos.sh signed|unsigned PATH_TO_APP PATH_TO_DMG" >&2
  exit 2
fi

mode=$1
app=$(realpath "$2")
dmg=$(realpath "$3")
[[ -d "$app" && -f "$dmg" ]] || { echo "macOS app or DMG is missing" >&2; exit 1; }

plist="$app/Contents/Info.plist"
[[ $(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$plist") == "com.enterprise-local-agent.desktop" ]] || { echo "unexpected bundle identifier" >&2; exit 1; }
[[ $(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$plist") == "12.0" ]] || { echo "unexpected minimum macOS version" >&2; exit 1; }
file "$app/Contents/MacOS/agent-desktop" | grep -q 'arm64' || { echo "application is not Apple Silicon" >&2; exit 1; }
hdiutil verify "$dmg"

if find "$app" \( -type f -o -type l \) \( -name '*.map' -o -name '*.pdb' -o -name '*.pem' -o -name '*.key' -o -name '.env' -o -name 'agent-service-daemon' -o -name 'agent-local-write-worker' -o -name 'bwrap' \) -print -quit | grep -q .; then
  echo "application contains a prohibited debug, credential, service, or containment artifact" >&2
  exit 1
fi

if [[ $mode == "signed" ]]; then
  codesign --verify --deep --strict --verbose=2 "$app"
  spctl --assess --type execute --verbose=2 "$app"
  xcrun stapler validate "$app"
  xcrun stapler validate "$dmg"
fi

echo "macOS artifacts verified ($mode): $(basename "$app"), $(basename "$dmg")"
