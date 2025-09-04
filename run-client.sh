#!/usr/bin/env bash
set -euo pipefail

# Defaults
SERVER="127.0.0.1:5000"
LOG_LEVEL="warn"
LOW_GFX=0
NO_VSYNC=0
CLIENT_PORT=0
SECURE=0
KEY=""
KEY_FILE=""

usage() {
  cat <<USAGE
Usage: $0 [-s HOST:PORT] [-l LOG] [--low-gfx] [--no-vsync] [--client-port N] [--secure] [--key HEX] [--key-file PATH]

Options:
  -s, --server      Server address (default: 127.0.0.1:5000)
  -l, --log         RUST_LOG level (default: warn)
      --low-gfx     Disable HDR/shadows
      --no-vsync    Disable VSync
      --client-port Local UDP port to bind (default: OS assigns)
      --secure      Enable Secure auth (requires --key or --key-file)
      --key         64-hex shared key (with or without 0x)
      --key-file    Path to key file (32B binary or HEX string)
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -s|--server) SERVER="$2"; shift 2;;
    -l|--log) LOG_LEVEL="$2"; shift 2;;
    --low-gfx) LOW_GFX=1; shift;;
    --no-vsync) NO_VSYNC=1; shift;;
    --client-port) CLIENT_PORT="$2"; shift 2;;
    --secure) SECURE=1; shift;;
    --key) KEY="$2"; shift 2;;
    --key-file) KEY_FILE="$2"; shift 2;;
    -h|--help) usage; exit 0;;
    *) echo "Unknown option: $1"; usage; exit 1;;
  esac
done

export SERVER_ADDR="$SERVER"
export RUST_LOG="$LOG_LEVEL"

if [[ $LOW_GFX -eq 1 ]]; then export LOW_GFX=1; else unset LOW_GFX || true; fi
if [[ $NO_VSYNC -eq 1 ]]; then export NO_VSYNC=1; else unset NO_VSYNC || true; fi
if [[ "$CLIENT_PORT" != "0" ]]; then export CLIENT_PORT="$CLIENT_PORT"; else unset CLIENT_PORT || true; fi

if [[ $SECURE -eq 1 ]]; then
  export SECURE=1
  if [[ -n "$KEY" ]]; then
    export NETCODE_KEY="$KEY"
  elif [[ -n "$KEY_FILE" ]]; then
    export NETCODE_KEY_FILE="$KEY_FILE"
  else
    echo "--secure is set but no --key/--key-file provided" >&2
    exit 2
  fi
fi

echo "SERVER_ADDR=$SERVER_ADDR LOW_GFX=${LOW_GFX:-0} NO_VSYNC=${NO_VSYNC:-0} CLIENT_PORT=${CLIENT_PORT:-0} SECURE=${SECURE:-0} RUST_LOG=$RUST_LOG"

DIR="$(cd "$(dirname "$0")" && pwd)"
BIN1="$DIR/bevy-online-campus"
BIN2="$DIR/target/release/bevy-online-campus"

if [[ -x "$BIN1" ]]; then
  exec "$BIN1"
elif [[ -x "$BIN2" ]]; then
  exec "$BIN2"
else
  echo "Executable not found. Falling back to cargo run --release" >&2
  exec cargo run --release
fi

