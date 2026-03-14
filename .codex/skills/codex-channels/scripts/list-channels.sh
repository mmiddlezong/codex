#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./common.sh
source "$SCRIPT_DIR/common.sh"

usage() {
  cat <<'EOF'
Usage: list-channels.sh --server-url URL --token TOKEN [--header 'Name: Value' ...]

List available channel-server channels.
Pass any extra headers explicitly with repeated --header flags.
EOF
}

server_url=""
token=""
headers=()

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
    --header)
      [ "$#" -ge 2 ] || usage_error "--header requires a value"
      headers+=("$2")
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

header_args=()
if [ "${#headers[@]}" -gt 0 ]; then
  for header in "${headers[@]}"; do
    header_args+=(--header "$header")
  done
  api_request "GET" "$resolved_url" "$resolved_token" "/v1/channels" "${header_args[@]}"
else
  api_request "GET" "$resolved_url" "$resolved_token" "/v1/channels"
fi
