#!/bin/sh
set -eu
cd "$(dirname "$0")/.."

xcodebuild -project Goat.xcodeproj -scheme Goat -destination 'platform=macOS' \
  -derivedDataPath DerivedData -skipPackagePluginValidation -quiet build

# Launched through `open`, not by running the binary, so macOS asks for Accessibility and Screen
# Recording on behalf of the app rather than the terminal.
pkill -f 'Goat.app/Contents/MacOS/Goat' || true
trap "pkill -f 'Goat.app/Contents/MacOS/Goat'" EXIT
trap 'exit 130' INT TERM
open -W DerivedData/Build/Products/Debug/Goat.app
