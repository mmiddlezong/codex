# Local TUI Control Plane

This fork adds a local-only control-plane seam for interactive `codex` sessions.
It does not change auth, the normal chat UI, or `codex exec`.

## Enable It

Add this to `~/.codex/config.toml`:

```toml
[control_plane]
enabled = true
```

Available settings:

```toml
[control_plane]
enabled = false
consent = "accepted" # or "declined"; managed by the TUI after prompting
ipc_dir = "/absolute/path/to/control-plane/instances"
steering_enabled = true
server_url = "http://127.0.0.1:3000"
server_token = "replace-me"
channel_subscriptions = ["ops", "research"]

# Reserved for future server/channel work.
# Additional server/channel behavior is evolving, but these are now used by
# the Rust-owned wrapper flow.
```

Defaults:

- `enabled = false`
- `consent = unset`
- `ipc_dir = "$CODEX_HOME/control-plane/instances"`
- `steering_enabled = true`
- `server_url = unset`
- `server_token = unset`
- `channel_subscriptions = unset`

When `enabled = true` and consent is not yet accepted, the interactive TUI shows a short startup
prompt. Accepting persists `consent = "accepted"` and starts the local socket immediately.
Declining persists `consent = "declined"`, starts no socket, and prompts again on the next launch.

## Socket Discovery

Each interactive TUI instance gets its own local socket under `control_plane.ipc_dir`.
External tools should enumerate socket files in that directory and call `status` to identify the
right instance.

The socket is created only after consent is accepted.

## Request Format

The protocol is one JSON request per connection, with one JSON response.

Example requests:

```json
{"command":"status"}
{"command":"current-session"}
{"command":"current-turn"}
{"command":"apply-steer","expectedTurnId":"turn-123","text":"Please stop and summarize the current findings.","idempotencyKey":"req-001","metadata":{"source":"relay"}}
```

Example Unix call with `socat`:

```sh
printf '%s\n' '{"command":"status"}' | socat - UNIX-CONNECT:/path/to/socket.sock
```

## Commands

- `status`: returns instance metadata, socket path, launch kind, and booleans for feature state,
  consent, steering enablement, session presence, and active-turn presence.
- `current-session`: returns `null` until the primary thread emits `SessionConfigured`, then returns
  the primary thread id, thread name, cwd, and launch kind.
- `current-turn`: returns `null` when the primary thread has no in-flight turn, otherwise the
  current primary turn id and `in_progress`.
- `apply-steer`: applies text steering to the active primary turn only.

## Steering Safety Checks

`apply-steer` validates, in order:

1. The control-plane feature is enabled.
2. Consent was accepted and local steering is allowed.
3. The primary interactive thread is known and has an active turn.
4. `expectedTurnId` matches the active turn id.
5. `(expectedTurnId, idempotencyKey)` has not already been accepted.
6. `text` is not empty.

If validation passes, the TUI maps `text` into normal Codex user input and calls the real core
`steer_input` seam, which enforces the same active-turn and expected-turn checks again.

Possible `apply-steer` outcomes:

- `applied`
- `rejected_not_enabled`
- `rejected_not_authorized`
- `rejected_no_active_turn`
- `rejected_stale_turn`
- `duplicate`
- `error`

Steer requests are logged with structured metadata, but not with the raw steer text.
