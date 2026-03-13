---
name: codex-channels
description: Send messages to Codex channel-server channels, list channels, or create channels using the bundled shell scripts instead of hand-written curl commands. Use when the user asks to post to a channel, send something to ops or research, create a channel, or inspect available channels.
---

# Codex Channels

## Objective
Use the bundled scripts in this skill for channel-server operations instead of retyping `curl` by hand.

This skill covers:

- sending a message to a channel
- listing channels
- creating a channel

This v1 skill does not:

- expose metadata flags
- auto-create channels during send
- infer a target channel from saved subscriptions

## Workflow

1. Prefer the bundled scripts under `scripts/` for any supported channel action.
2. Pass `--server-url` and `--token` only when you need to override defaults.
3. Otherwise let the scripts read `CHANNEL_SERVER_URL` / `CHANNEL_SERVER_TOKEN`, then fall back to `~/.codex/config.toml` `[control_plane]`.
4. For send operations, pass the channel explicitly and provide the message either as an argument or through stdin for multiline text.
5. If the user wants a channel that may not exist yet, run the create script first rather than improvising auto-create behavior.

## Commands

Assume the repo root is available as:

```bash
repo_root="$(git rev-parse --show-toplevel)"
```

List channels:

```bash
"$repo_root/.codex/skills/codex-channels/scripts/list-channels.sh"
```

Create a channel:

```bash
"$repo_root/.codex/skills/codex-channels/scripts/create-channel.sh" ops
```

Send a short message:

```bash
"$repo_root/.codex/skills/codex-channels/scripts/send-channel-message.sh" ops "Please summarize the blocker."
```

Send multiline text via stdin:

```bash
cat <<'EOF' | "$repo_root/.codex/skills/codex-channels/scripts/send-channel-message.sh" ops
Please stop and summarize the current findings.
Include open questions and next steps.
EOF
```

Override config explicitly:

```bash
"$repo_root/.codex/skills/codex-channels/scripts/list-channels.sh" \
  --server-url http://127.0.0.1:3000 \
  --token dev-token
```

## Notes

- The scripts print raw JSON so agents can consume the responses directly.
- `create-channel.sh` enforces the same slug pattern as the server: lowercase letters, digits, `.`, `_`, and `-`.
- If the user asks for metadata-rich sends or automatic channel creation during publish, say that this skill does not expose those conveniences yet and either extend the skill or use the raw API deliberately.
