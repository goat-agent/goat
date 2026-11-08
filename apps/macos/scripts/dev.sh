#!/bin/sh
set -eu
cd "$(dirname "$0")/.."

xcodebuild -project Goat.xcodeproj -scheme Goat -destination 'platform=macOS' \
  -derivedDataPath DerivedData -skipPackagePluginValidation -quiet build

pkill -f 'Goat.app/Contents/MacOS/Goat' || true
trap "pkill -f 'Goat.app/Contents/MacOS/Goat'" EXIT
trap 'exit 130' INT TERM
open -W DerivedData/Build/Products/Debug/Goat.app
