#!/bin/sh
set -eu

REPO="goat-agent/goat"
API_URL="https://api.github.com/repos/${REPO}/releases/latest"
INSTALL_DIR="${GOAT_INSTALL_DIR:-${HOME:-}/.local/bin}"
GOAT_ROOT="${GOAT_ROOT:-${HOME:-}/.goat}"
BIN_PATH="${INSTALL_DIR}/goat"

log() {
  printf '%s\n' "$*"
}

fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

have() {
  command -v "$1" >/dev/null 2>&1
}

cleanup() {
  if [ "${TMPDIR_GOAT:-}" ]; then
    rm -rf "$TMPDIR_GOAT"
  fi
}
trap cleanup EXIT HUP INT TERM

need_home() {
  [ -n "${HOME:-}" ] || fail "HOME is not set"
}

need_cmd() {
  have "$1" || fail "missing required command: $1"
}

detect_target() {
  os=$(uname -s 2>/dev/null || true)
  arch=$(uname -m 2>/dev/null || true)

  case "$os:$arch" in
    Darwin:x86_64) printf '%s\n' x86_64-apple-darwin ;;
    Darwin:arm64|Darwin:aarch64) printf '%s\n' aarch64-apple-darwin ;;
    Linux:x86_64|Linux:amd64) printf '%s\n' x86_64-unknown-linux-gnu ;;
    Linux:aarch64|Linux:arm64) printf '%s\n' aarch64-unknown-linux-gnu ;;
    *) fail "unsupported platform: ${os:-unknown}/${arch:-unknown}" ;;
  esac
}

latest_tag() {
  curl -fsSL "$API_URL" \
    | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
    | sed -n '1p'
}

sha256_file() {
  file=$1
  expected=$2

  if have sha256sum; then
    actual=$(sha256sum "$file" | awk '{print $1}')
  elif have shasum; then
    actual=$(shasum -a 256 "$file" | awk '{print $1}')
  else
    log "warning: neither sha256sum nor shasum found; skipping checksum verification"
    return 0
  fi

  [ "$actual" = "$expected" ] || fail "checksum mismatch for $(basename "$file")"
}

validate_archive() {
  archive=$1
  list=$2

  tar -tzf "$archive" > "$list"
  count=$(wc -l < "$list" | tr -d ' ')
  [ "$count" = "1" ] || fail "archive must contain exactly one entry named goat"

  entry=$(sed -n '1p' "$list")
  case "$entry" in
    goat|./goat) ;;
    *) fail "archive entry must be goat, got: $entry" ;;
  esac

  mode=$(tar -tvzf "$archive" | sed -n '1s/^\(.*\)$/\1/p' | cut -c 1)
  [ "$mode" = "-" ] || fail "archive entry must be a regular file"
}

install_binary() {
  src=$1
  [ -f "$src" ] || fail "archive did not contain goat"
  [ -x "$src" ] || chmod 755 "$src"

  mkdir -p "$INSTALL_DIR"
  tmp_bin="${BIN_PATH}.tmp.$$"
  cp "$src" "$tmp_bin"
  chmod 755 "$tmp_bin"
  mv "$tmp_bin" "$BIN_PATH"
}

has_initial_config() {
  personas_dir="$GOAT_ROOT/personas"
  [ -d "$personas_dir" ] || return 1
  for persona in "$personas_dir"/*; do
    [ -f "$persona/persona.md" ] && return 0
  done
  return 1
}

path_warning() {
  case ":${PATH:-}:" in
    *:"$INSTALL_DIR":*) ;;
    *)
      log "warning: $INSTALL_DIR is not on PATH"
      log "         use $BIN_PATH explicitly or add $INSTALL_DIR to PATH"
      ;;
  esac
}

write_launchd_service() {
  plist_dir="$HOME/Library/LaunchAgents"
  plist="$plist_dir/com.goat.agent.plist"
  mkdir -p "$plist_dir" "$GOAT_ROOT/logs"

  cat > "$plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.goat.agent</string>
  <key>ProgramArguments</key>
  <array>
    <string>$BIN_PATH</string>
    <string>run</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>$GOAT_ROOT/logs/launchd.out.log</string>
  <key>StandardErrorPath</key>
  <string>$GOAT_ROOT/logs/launchd.err.log</string>
  <key>WorkingDirectory</key>
  <string>$HOME</string>
</dict>
</plist>
EOF

  if have launchctl; then
    launchctl bootout "gui/$(id -u)" "$plist" >/dev/null 2>&1 || true
    launchctl bootstrap "gui/$(id -u)" "$plist"
    launchctl enable "gui/$(id -u)/com.goat.agent" >/dev/null 2>&1 || true
    log "service: launchctl print gui/$(id -u)/com.goat.agent"
    log "restart: launchctl kickstart -k gui/$(id -u)/com.goat.agent"
  else
    log "warning: launchctl not found; service file written to $plist but not loaded"
  fi
}

write_systemd_service() {
  systemd_dir="$HOME/.config/systemd/user"
  unit="$systemd_dir/goat.service"
  mkdir -p "$systemd_dir" "$GOAT_ROOT/logs"

  cat > "$unit" <<EOF
[Unit]
Description=goat personal AI agent
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=$BIN_PATH run
Restart=on-failure
RestartSec=5
WorkingDirectory=$HOME

[Install]
WantedBy=default.target
EOF

  if have systemctl; then
    systemctl --user daemon-reload
    systemctl --user enable goat.service
    systemctl --user restart goat.service
    log "service: systemctl --user status goat.service"
    log "restart: systemctl --user restart goat.service"
  else
    log "warning: systemctl not found; service file written to $unit but not loaded"
  fi
}

install_service() {
  os=$(uname -s 2>/dev/null || true)
  case "$os" in
    Darwin) write_launchd_service ;;
    Linux) write_systemd_service ;;
    *) fail "unsupported service platform: ${os:-unknown}" ;;
  esac
}

main() {
  need_home
  need_cmd curl
  need_cmd tar
  need_cmd sed
  need_cmd awk
  need_cmd wc

  target=$(detect_target)
  tag=${GOAT_VERSION:-}
  if [ -z "$tag" ]; then
    tag=$(latest_tag)
    [ -n "$tag" ] || fail "could not resolve latest release tag"
  fi

  asset="goat-${tag}-${target}.tar.gz"
  base_url="https://github.com/${REPO}/releases/download/${tag}"

  TMPDIR_GOAT=$(mktemp -d 2>/dev/null || mktemp -d -t goat)
  archive="$TMPDIR_GOAT/$asset"
  checksum="$TMPDIR_GOAT/$asset.sha256"
  list="$TMPDIR_GOAT/archive.list"
  extract_dir="$TMPDIR_GOAT/extract"

  log "installing goat ${tag} for ${target}"
  curl -fL "$base_url/$asset" -o "$archive"
  curl -fL "$base_url/$asset.sha256" -o "$checksum"

  expected=$(awk '{print $1}' "$checksum")
  [ -n "$expected" ] || fail "empty checksum file for $asset"
  sha256_file "$archive" "$expected"

  validate_archive "$archive" "$list"
  mkdir -p "$extract_dir"
  tar -xzf "$archive" -C "$extract_dir"
  install_binary "$extract_dir/goat"

  log "installed: $BIN_PATH"
  path_warning

  fresh=0
  if has_initial_config; then
    fresh=1
  fi

  if [ "$fresh" = "0" ] && [ -t 0 ] && [ -t 1 ]; then
    log "running first-time setup"
    "$BIN_PATH" setup
  elif [ "$fresh" = "0" ]; then
    log "setup required: $BIN_PATH setup"
    log "after setup, restart the service with the restart command printed below"
  fi

  install_service

  log "doctor: $BIN_PATH doctor"
  log "done"
}

main "$@"
