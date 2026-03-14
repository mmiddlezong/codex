#!/usr/bin/env bash

set -euo pipefail

CODEX_CHANNELS_DEFAULT_CONFIG_PATH="${HOME}/.codex/config.toml"
CODEX_CHANNELS_SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

warn() {
  printf 'warning: %s\n' "$*" >&2
}

usage_error() {
  die "$@"
}

load_config_value() {
  local key="$1"
  local config_path="${CODEX_CONFIG_PATH:-$CODEX_CHANNELS_DEFAULT_CONFIG_PATH}"

  if [ ! -f "$config_path" ]; then
    return 0
  fi

  if ! command -v python3 >/dev/null 2>&1; then
    return 11
  fi

  python3 - "$config_path" "$key" <<'PY'
import sys

config_path, key = sys.argv[1:3]

try:
    import tomllib
except ModuleNotFoundError:
    raise SystemExit(12)

try:
    with open(config_path, "rb") as handle:
        config = tomllib.load(handle)
except tomllib.TOMLDecodeError:
    raise SystemExit(13)

value = config.get("control_plane", {}).get(key)
if isinstance(value, str):
    print(value)
PY
}

write_config_http_headers_null() {
  local config_path="${CODEX_CONFIG_PATH:-$CODEX_CHANNELS_DEFAULT_CONFIG_PATH}"

  if [ ! -f "$config_path" ]; then
    return 0
  fi

  if ! command -v python3 >/dev/null 2>&1; then
    die "python3 is required to read ${config_path}; remove [control_plane].http_headers or install python3"
  fi

  if python3 - "$config_path" <<'PY'
import sys

config_path = sys.argv[1]
reserved_headers = {"authorization", "content-type", "host"}

try:
    import tomllib
except ModuleNotFoundError:
    raise SystemExit(12)

try:
    with open(config_path, "rb") as handle:
        config = tomllib.load(handle)
except tomllib.TOMLDecodeError:
    raise SystemExit(13)

headers = config.get("control_plane", {}).get("http_headers")
if headers is None:
    raise SystemExit(0)

if not isinstance(headers, dict):
    print(
        "error: [control_plane].http_headers must be a table of string header values",
        file=sys.stderr,
    )
    raise SystemExit(14)

for name, value in headers.items():
    if not isinstance(name, str) or not isinstance(value, str):
        print(
            "error: [control_plane].http_headers entries must map header names to string values",
            file=sys.stderr,
        )
        raise SystemExit(14)
    if name.lower() in reserved_headers:
        print(
            f"error: [control_plane].http_headers cannot override reserved header `{name}`",
            file=sys.stderr,
        )
        raise SystemExit(14)
    if "\n" in name or "\r" in name:
        print(
            f"error: [control_plane].http_headers contains invalid header name `{name}`",
            file=sys.stderr,
        )
        raise SystemExit(14)
    if "\n" in value or "\r" in value:
        print(
            f"error: [control_plane].http_headers contains invalid value for `{name}`",
            file=sys.stderr,
        )
        raise SystemExit(14)

    sys.stdout.buffer.write(name.encode("utf-8"))
    sys.stdout.buffer.write(b"\0")
    sys.stdout.buffer.write(value.encode("utf-8"))
    sys.stdout.buffer.write(b"\0")
PY
  then
    return 0
  else
    local status=$?
    case "$status" in
      12 | 13)
        die "could not read ${config_path}; fix [control_plane].http_headers or remove it"
        ;;
      14)
        return 14
        ;;
      *)
        die "failed to read ${config_path}; fix [control_plane].http_headers or remove it"
        ;;
    esac
  fi
}

resolve_control_plane_value() {
  local explicit="$1"
  local env_name="$2"
  local config_key="$3"
  local label="$4"
  local flag_name="$5"
  local config_path="${CODEX_CONFIG_PATH:-$CODEX_CHANNELS_DEFAULT_CONFIG_PATH}"

  if [ -n "$explicit" ]; then
    printf '%s\n' "$explicit"
    return 0
  fi

  local env_value="${!env_name:-}"
  if [ -n "$env_value" ]; then
    printf '%s\n' "$env_value"
    return 0
  fi

  local config_value
  if config_value="$(load_config_value "$config_key")"; then
    if [ -n "$config_value" ]; then
      printf '%s\n' "$config_value"
      return 0
    fi
  else
    local status=$?
    case "$status" in
      11)
        die "python3 is required to read ${config_path}; pass ${flag_name} or set ${env_name}"
        ;;
      12 | 13)
        die "could not read ${config_path}; pass ${flag_name} or set ${env_name}"
        ;;
      *)
        die "failed to read ${config_path}; pass ${flag_name} or set ${env_name}"
        ;;
    esac
  fi

  die "missing ${label}; pass ${flag_name}, set ${env_name}, or configure [control_plane].${config_key} in ${config_path}"
}

resolve_server_url() {
  local explicit="${1:-}"
  resolve_control_plane_value "$explicit" "CHANNEL_SERVER_URL" "server_url" "server URL" "--server-url"
}

resolve_server_token() {
  local explicit="${1:-}"
  resolve_control_plane_value "$explicit" "CHANNEL_SERVER_TOKEN" "server_token" "server token" "--token"
}

api_request() {
  local method="$1"
  local base_url="$2"
  local token="$3"
  local path="$4"
  shift 4
  local url="${base_url%/}${path}"
  local payload=""
  local headers_file
  local -a extra_headers=()

  while [ "$#" -gt 0 ]; do
    case "$1" in
      --payload)
        [ "$#" -ge 2 ] || die "api_request missing payload value"
        payload="$2"
        shift 2
        ;;
      --header)
        [ "$#" -ge 2 ] || die "api_request missing header value"
        extra_headers+=("$2")
        shift 2
        ;;
      *)
        die "api_request received unexpected argument: $1"
        ;;
    esac
  done

  local -a curl_args=(
    --silent
    --show-error
    --fail-with-body
    -X "$method"
    "$url"
    -H "authorization: Bearer $token"
  )
  headers_file="$(mktemp)"
  write_config_http_headers_null >"$headers_file"
  while IFS= read -r -d '' header_name && IFS= read -r -d '' header_value; do
    curl_args+=(-H "${header_name}: ${header_value}")
  done <"$headers_file"
  rm -f "$headers_file"
  local header
  local header_name
  local normalized_header_name
  if [ "${#extra_headers[@]}" -gt 0 ]; then
    for header in "${extra_headers[@]}"; do
      if [[ "$header" != *:* ]]; then
        die "invalid header \`${header}\`; expected 'Name: Value'"
      fi
      header_name="${header%%:*}"
      if [ -z "${header_name//[[:space:]]/}" ]; then
        die "invalid header \`${header}\`; header name must not be empty"
      fi
      normalized_header_name="$(printf '%s' "$header_name" | tr '[:upper:]' '[:lower:]' | tr -d '[:space:]')"
      case "$normalized_header_name" in
        authorization | content-type | host)
          die "header \`${header_name}\` is reserved; do not pass it via --header"
          ;;
      esac
      curl_args+=(-H "$header")
    done
  fi

  if [ -n "$payload" ]; then
    curl_args+=(-H "content-type: application/json" --data "$payload")
    curl "${curl_args[@]}"
    return
  fi

  curl "${curl_args[@]}"
}

validate_channel_slug() {
  local channel="$1"
  if [[ ! "$channel" =~ ^[a-z0-9._-]+$ ]]; then
    die "invalid channel name \`${channel}\`; use lowercase letters, digits, '.', '_' or '-'"
  fi
}

generate_idempotency_key() {
  if command -v python3 >/dev/null 2>&1; then
    python3 - <<'PY'
import uuid
print(uuid.uuid4())
PY
    return
  fi

  printf 'codex-channels-%s-%s\n' "$(date +%s)" "$$"
}

build_publish_payload_json() {
  local channel="$1"
  local message="$2"
  local idempotency_key="$3"
  local exclude_instance_id="${4:-}"

  if ! command -v python3 >/dev/null 2>&1; then
    die "python3 is required to build the publish payload"
  fi

  python3 - "$channel" "$message" "$idempotency_key" "$exclude_instance_id" <<'PY'
import json
import sys

channel, message, idempotency_key, exclude_instance_id = sys.argv[1:5]
payload = {
    "channel": channel,
    "text": message,
    "idempotencyKey": idempotency_key,
}
if exclude_instance_id:
    payload["excludeInstanceId"] = exclude_instance_id
print(json.dumps(payload))
PY
}

resolve_local_exclude_instance_id() {
  if [ -z "${CODEX_THREAD_ID:-}" ]; then
    return 1
  fi

  local helper_path="$CODEX_CHANNELS_SCRIPT_DIR/resolve-local-instance-id.py"
  local helper_output
  if helper_output="$(python3 "$helper_path" 2>&1)"; then
    printf '%s\n' "$helper_output"
    return 0
  fi

  helper_output="${helper_output//$'\n'/ }"
  warn "$helper_output"
  return 1
}

build_create_channel_payload_json() {
  local channel="$1"

  if ! command -v python3 >/dev/null 2>&1; then
    die "python3 is required to build the create-channel payload"
  fi

  python3 - "$channel" <<'PY'
import json
import sys

print(json.dumps({"channel": sys.argv[1]}))
PY
}
