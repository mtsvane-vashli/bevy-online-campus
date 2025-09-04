#!/usr/bin/env bash
set -euo pipefail

# Defaults
ADDRESS="0.0.0.0"
PORT="5000"
LOG_LEVEL="warn"
SECURE="0"
KEY=""
KEY_FILE=""

usage() {
  cat <<USAGE
Usage: $0 [-a ADDRESS] [-p PORT] [-l LOG] [--secure] [--key HEX] [--key-file PATH]

Options:
  -a, --address    Bind/advertise address (default: 0.0.0.0)
  -p, --port       UDP port (default: 5000)
  -l, --log        RUST_LOG level (default: warn)
      --secure     Enable Secure auth (requires --key or --key-file)
      --key        64-hex shared key (with or without 0x)
      --key-file   Path to key file (32B binary or HEX string)

Environment defaults set by script (override if不要):
  WGPU_BACKEND=vk, WGPU_ALLOW_SOFTWARE=1
USAGE
}

ARGS=("$@")
while [[ $# -gt 0 ]]; do
  case "$1" in
    -a|--address) ADDRESS="$2"; shift 2;;
    -p|--port) PORT="$2"; shift 2;;
    -l|--log) LOG_LEVEL="$2"; shift 2;;
    --secure) SECURE="1"; shift;;
    --key) KEY="$2"; shift 2;;
    --key-file) KEY_FILE="$2"; shift 2;;
    -h|--help) usage; exit 0;;
    *) echo "Unknown option: $1"; usage; exit 1;;
  esac
done

export SERVER_ADDR="${ADDRESS}:${PORT}"
export RUST_LOG="${LOG_LEVEL}"
export WGPU_BACKEND="${WGPU_BACKEND:-vk}"
export WGPU_ALLOW_SOFTWARE="${WGPU_ALLOW_SOFTWARE:-1}"

if [[ "$SECURE" == "1" ]]; then
  export SECURE=1
  if [[ -n "$KEY" ]]; then
    export NETCODE_KEY="${KEY}"
  elif [[ -n "$KEY_FILE" ]]; then
    export NETCODE_KEY_FILE="${KEY_FILE}"
  else
    echo "--secure is set but no --key/--key-file provided" >&2
    exit 2
  fi
fi

echo "SERVER_ADDR=$SERVER_ADDR RUST_LOG=$RUST_LOG SECURE=${SECURE:-0} WGPU_BACKEND=$WGPU_BACKEND WGPU_ALLOW_SOFTWARE=$WGPU_ALLOW_SOFTWARE"

DIR="$(cd "$(dirname "$0")" && pwd)"
BIN1="$DIR/server"
BIN2="$DIR/target/release/server"

if [[ -x "$BIN1" ]]; then
  exec "$BIN1"
elif [[ -x "$BIN2" ]]; then
  exec "$BIN2"
else
  echo "Executable not found. Falling back to cargo run --release --bin server" >&2
  exec cargo run --release --bin server
fi

