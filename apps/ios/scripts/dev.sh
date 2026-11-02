#!/bin/sh
set -eu
cd "$(dirname "$0")/.."

device=$(xcrun simctl list devices booted | grep iPhone | grep -m1 -oE '[0-9A-F-]{36}' ||
  xcrun simctl list devices available | grep iPhone | grep -m1 -oE '[0-9A-F-]{36}')

xcrun simctl bootstatus "$device" -b >/dev/null
# Xcode 27 replaced Simulator with Device Hub.
tools="$(xcode-select -p)/../Applications"
open "$tools/DeviceHub.app" 2>/dev/null || open "$tools/Simulator.app"

xcodebuild -project Goat.xcodeproj -scheme Goat -destination "id=$device" \
  -derivedDataPath DerivedData -skipPackagePluginValidation -quiet build
xcrun simctl install "$device" DerivedData/Build/Products/Debug-iphonesimulator/Goat.app

trap 'xcrun simctl terminate "$device" com.goatagent.goat' EXIT
trap 'exit 130' INT TERM
xcrun simctl launch --console-pty --terminate-running-process "$device" com.goatagent.goat
