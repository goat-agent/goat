#!/bin/sh
set -eu
cd "$(dirname "$0")/.."

xcodebuild -project Goat.xcodeproj -scheme Goat -destination 'platform=macOS' \
  -derivedDataPath DerivedData -skipPackagePluginValidation -quiet build

app='Goat.app/Contents/MacOS/Goat'
pkill -f "$app" || true
trap "pkill -f '$app' || true" EXIT
trap 'exit 130' INT TERM
open DerivedData/Build/Products/Debug/Goat.app
# macOS quits and reopens Goat to apply screen recording, so stop only once it stays gone.
gone=0
while [ "$gone" -lt 3 ]; do
  if pgrep -f "$app" >/dev/null; then gone=0; else gone=$((gone + 1)); fi
  sleep 1
done
