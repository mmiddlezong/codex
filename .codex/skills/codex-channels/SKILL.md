---
name: codex-channels
description: Send messages to Codex channel-server channels, list channels, or create channels using the bundled shell scripts instead of hand-written curl commands. Use when the user asks to post to a channel, send something to ops or research, create a channel, or inspect available channels.
---

# Codex Channels

## Objective
Use the bundled scripts in this skill for channel-server operations instead of retyping `curl` by hand.

When Codex runs these scripts itself, treat them as escalated shell operations. The local
control-plane socket discovery used for sender exclusion is not reliable in the default shell
sandbox, so the shell tool should request `sandbox_permissions: "require_escalated"` with a short
justification before running the command.

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
2. When Codex invokes a script from this skill, request escalated shell permissions first. A good justification is: `Allow channel publish and local control-plane socket discovery for this Codex session?`
3. Pass `--server-url` and `--token` only when you need to override defaults.
4. Otherwise let the scripts read `CHANNEL_SERVER_URL` / `CHANNEL_SERVER_TOKEN`, then fall back to `~/.codex/config.toml` `[control_plane]`.
5. For send operations, pass the channel explicitly and provide the message either as an argument or through stdin for multiline text.
6. If the user wants a channel that may not exist yet, run the create script first rather than improvising auto-create behavior.
7. By default, Codex-owned sends try to exclude the current local Codex instance by matching `CODEX_THREAD_ID` against local control-plane sockets, then reading the matching socket's `instanceId`.
8. If that discovery fails, the send still goes through and the script warns on stderr that exclusion could not be determined.
9. Use `--include-self` only when the user explicitly wants self-broadcast.

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

Broadcast to all subscribers, including the sender:

```bash
"$repo_root/.codex/skills/codex-channels/scripts/send-channel-message.sh" \
  --include-self \
  ops \
  "Please stop and summarize the current findings."
```

## Notes

- The scripts print raw JSON so agents can consume the responses directly.
- When Codex runs these scripts, it should use an escalated shell invocation rather than a normal sandboxed shell call.
- `create-channel.sh` enforces the same slug pattern as the server: lowercase letters, digits, `.`, `_`, and `-`.
- `send-channel-message.sh` supports `--exclude-instance-id` for explicit targeting overrides and otherwise attempts thread-based local control-plane discovery from `CODEX_THREAD_ID` unless `--include-self` is passed.
- The bundled Python helper enumerates local control-plane sockets, matches `current-session.threadId` to `CODEX_THREAD_ID`, then reads `status.instanceId`.
- If the user asks for metadata-rich sends or automatic channel creation during publish, say that this skill does not expose those conveniences yet and either extend the skill or use the raw API deliberately.
