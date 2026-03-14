#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./common.sh
source "$SCRIPT_DIR/common.sh"

usage() {
  cat <<'EOF'
Usage: send-channel-message.sh [--server-url URL] [--token TOKEN] [--idempotency-key KEY] [--exclude-instance-id ID] [--include-self] <channel> [message...]

Send a channel-server message to the given channel.
Custom static headers from [control_plane].http_headers are sent automatically.

Examples:
  send-channel-message.sh ops "Please summarize the blocker."
  printf 'line 1\nline 2\n' | send-channel-message.sh ops
  send-channel-message.sh --include-self ops "Broadcast this to every subscriber, including me."
EOF
}

server_url=""
token=""
headers=()
idempotency_key=""
exclude_instance_id=""
include_self=false
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
    --header)
      [ "$#" -ge 2 ] || usage_error "--header requires a value"
      headers+=("$2")
      shift 2
      ;;
    --idempotency-key)
      [ "$#" -ge 2 ] || usage_error "--idempotency-key requires a value"
      idempotency_key="$2"
      shift 2
      ;;
    --exclude-instance-id)
      [ "$#" -ge 2 ] || usage_error "--exclude-instance-id requires a value"
      exclude_instance_id="$2"
      shift 2
      ;;
    --include-self)
      include_self=true
      shift
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

if [ "${#positionals[@]}" -lt 1 ]; then
  usage >&2
  exit 2
fi

channel="${positionals[0]}"

if [ "${#positionals[@]}" -gt 1 ]; then
  message_parts=("${positionals[@]:1}")
  message="${message_parts[*]}"
else
  if [ -t 0 ]; then
    usage_error "message text must be provided as an argument or via stdin"
  fi
  message="$(cat)"
fi

if ! printf '%s' "$message" | grep -q '[^[:space:]]'; then
  die "message text must not be empty"
fi

resolved_url="$(resolve_server_url "$server_url")"
resolved_token="$(resolve_server_token "$token")"

if [ -z "$idempotency_key" ]; then
  idempotency_key="$(generate_idempotency_key)"
fi

if [ -z "$exclude_instance_id" ] && [ "$include_self" = false ] && [ -n "${CODEX_THREAD_ID:-}" ]; then
  if resolved_instance_id="$(resolve_local_exclude_instance_id)"; then
    exclude_instance_id="$resolved_instance_id"
  fi
fi

payload="$(build_publish_payload_json "$channel" "$message" "$idempotency_key" "$exclude_instance_id")"
header_args=()
if [ "${#headers[@]}" -gt 0 ]; then
  for header in "${headers[@]}"; do
    header_args+=(--header "$header")
  done
  api_request "POST" "$resolved_url" "$resolved_token" "/v1/messages" --payload "$payload" "${header_args[@]}"
else
  api_request "POST" "$resolved_url" "$resolved_token" "/v1/messages" --payload "$payload"
fi
