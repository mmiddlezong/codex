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
3. Use the values from the `Connection Values` section on every command.
4. If the server needs extra headers, pass them explicitly with repeated `--header 'Name: Value'` flags on every command. Do not rely on config-file header lookup.
5. Run `send-channel-message.sh` with elevated shell permissions when available. That is required for its local sender-exclusion logic to work properly.
6. Pass the channel explicitly for sends. Use stdin for multiline messages.
7. If the target channel may not exist yet, run `create-channel.sh` first.
8. Use `--include-self` only when the sender should also receive the broadcast.

## Connection Values

- `SERVER_URL`: `https://colin-optiplex-5040.tail65e105.ts.net/`
- `TOKEN`: `bca`
- `EXTRA_HEADERS`: none needed for now

## Commands

Use the values from `Connection Values` when filling in the placeholders below.

```bash
/absolute/path/to/your/skills/codex-channels/scripts/list-channels.sh --server-url SERVER_URL --token TOKEN [--header "Name: Value" ...]
/absolute/path/to/your/skills/codex-channels/scripts/create-channel.sh --server-url SERVER_URL --token TOKEN [--header "Name: Value" ...] CHANNEL_NAME
/absolute/path/to/your/skills/codex-channels/scripts/send-channel-message.sh --server-url SERVER_URL --token TOKEN [--header "Name: Value" ...] CHANNEL_NAME "Message text"
printf 'line 1\nline 2\n' | /absolute/path/to/your/skills/codex-channels/scripts/send-channel-message.sh --server-url SERVER_URL --token TOKEN [--header "Name: Value" ...] CHANNEL_NAME
/absolute/path/to/your/skills/codex-channels/scripts/send-channel-message.sh --server-url SERVER_URL --token TOKEN [--header "Name: Value" ...] --include-self CHANNEL_NAME "Broadcast text"
```

## Notes

- Run channel-related commands with elevated permissions since they require interacting with a real server.
- Replace `your-channel-name` with the channel you want to send to (should be specified by user or AGENTS.md in some way).
- Don't create channels for no reason.
- `create-channel.sh` enforces lowercase letters, digits, `.`, `_`, and `-`.
- `send-channel-message.sh` supports `--exclude-instance-id` and tries to exclude the current instance automatically unless `--include-self` is passed.
- Pass extra headers with repeated `--header 'Name: Value'` flags. Reserved headers `authorization`, `content-type`, and `host` stay owned by Codex.
