#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./common.sh
source "$SCRIPT_DIR/common.sh"

usage() {
  cat <<'EOF'
Usage: list-channels.sh [--server-url URL] [--token TOKEN]

List available channel-server channels.
EOF
}

server_url=""
token=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --server-url)
      [ "$#" -ge 2 ] || usage_error "--server-url requires a value"
      server_url="$2"
      shift 2
      ;;
    --token|--server-token)
      [ "$#" -ge 2 ] || usage_error "--token requires a value"
      token="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    -*)
      usage_error "unknown option: $1"
      ;;
    *)
      usage_error "unexpected argument: $1"
      ;;
  esac
done

resolved_url="$(resolve_server_url "$server_url")"
resolved_token="$(resolve_server_token "$token")"

api_request "GET" "$resolved_url" "$resolved_token" "/v1/channels"
