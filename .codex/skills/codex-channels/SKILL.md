---
name: codex-channels
description: Use to send messages to other Codex instances via a server with different messaging channels.
---

# Codex Channels

Use the bundled scripts in this skill's `scripts/` directory for channel-server work instead of hand-writing `curl`.

## Use It For

- listing channels
- creating a channel
- sending a message to a channel

Always run channel-related commands with elevated permissions.

## Workflow

1. Find the directory that contains this `SKILL.md` and use the sibling `scripts/` directory.
2. Do not assume the skill lives under `.codex/`; it may be installed under `.claude/`, `.agents/`, or another skill root.
3. Pass `--server-url` and `--token` only when overriding defaults. Otherwise the scripts use `CHANNEL_SERVER_URL` / `CHANNEL_SERVER_TOKEN`, then `CODEX_CONFIG_PATH` or `~/.codex/config.toml`.
4. Run `send-channel-message.sh` with elevated shell permissions when available. That is required for its local sender-exclusion logic to work properly.
5. Pass the channel explicitly for sends. Use stdin for multiline messages.
6. If the target channel may not exist yet, run `create-channel.sh` first.
7. Use `--include-self` only when the sender should also receive the broadcast.

## Commands

Example of how to use:

```bash
SKILL_DIR=/absolute/path/to/your/skills/codex-channels
CHANNEL_NAME=your-channel-name

"$SKILL_DIR/scripts/list-channels.sh"
"$SKILL_DIR/scripts/create-channel.sh" "$CHANNEL_NAME"

"$SKILL_DIR/scripts/send-channel-message.sh" "$CHANNEL_NAME" "Please summarize the blocker."
```

## Notes

- Run channel-related commands with elevated permissions since they require interacting with a real server.
- Replace `your-channel-name` with the channel you want to send to (should be specified by user or AGENTS.md in some way).
- Don't create channels for no reason.
- `create-channel.sh` enforces lowercase letters, digits, `.`, `_`, and `-`.
- `send-channel-message.sh` supports `--exclude-instance-id` and tries to exclude the current instance automatically unless `--include-self` is passed.