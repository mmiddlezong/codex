#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./common.sh
source "$SCRIPT_DIR/common.sh"

usage() {
  cat <<'EOF'
Usage: create-channel.sh [--server-url URL] [--token TOKEN] <channel>

Create a channel-server channel if it does not already exist.
EOF
}

server_url=""
token=""
positionals=()

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
    --)
      shift
      while [ "$#" -gt 0 ]; do
        positionals+=("$1")
        shift
      done
      ;;
    -*)
      usage_error "unknown option: $1"
      ;;
    *)
      positionals+=("$1")
      shift
      ;;
  esac
done

if [ "${#positionals[@]}" -ne 1 ]; then
  usage >&2
  exit 2
fi

channel="${positionals[0]}"
validate_channel_slug "$channel"

resolved_url="$(resolve_server_url "$server_url")"
resolved_token="$(resolve_server_token "$token")"
payload="$(build_create_channel_payload_json "$channel")"

api_request "POST" "$resolved_url" "$resolved_token" "/v1/channels" "$payload"
