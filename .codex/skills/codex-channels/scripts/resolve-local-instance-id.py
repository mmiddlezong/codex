#!/usr/bin/env python3

from __future__ import annotations

import json
import os
import socket
import stat
import sys
from pathlib import Path


DEFAULT_CONFIG_PATH = Path.home() / ".codex" / "config.toml"
DEFAULT_IPC_DIR = Path.home() / ".codex" / "control-plane" / "instances"


def load_ipc_dir() -> Path:
    config_path = Path(os.environ.get("CODEX_CONFIG_PATH", DEFAULT_CONFIG_PATH))
    try:
        import tomllib
    except ModuleNotFoundError:
        return DEFAULT_IPC_DIR

    if not config_path.is_file():
        return DEFAULT_IPC_DIR

    try:
        with config_path.open("rb") as handle:
            config = tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError):
        return DEFAULT_IPC_DIR

    ipc_dir = config.get("control_plane", {}).get("ipc_dir")
    if isinstance(ipc_dir, str) and ipc_dir.strip():
        return Path(ipc_dir).expanduser()
    return DEFAULT_IPC_DIR


def request_socket(socket_path: Path, payload: dict[str, object]) -> dict[str, object]:
    if os.name == "nt":
        raise RuntimeError("local control-plane socket discovery via CODEX_THREAD_ID is unsupported on Windows")

    request_body = json.dumps(payload).encode("utf-8") + b"\n"
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
            sock.settimeout(1.0)
            sock.connect(str(socket_path))
            sock.sendall(request_body)
            sock.shutdown(socket.SHUT_WR)

            chunks: list[bytes] = []
            while True:
                chunk = sock.recv(65536)
                if not chunk:
                    break
                chunks.append(chunk)
    except OSError as err:
        raise RuntimeError(f"could not query socket {socket_path}: {err}") from err

    if not chunks:
        raise RuntimeError(f"socket {socket_path} returned no response")

    try:
        return json.loads(b"".join(chunks).decode("utf-8").strip())
    except (UnicodeDecodeError, json.JSONDecodeError) as err:
        raise RuntimeError(f"socket {socket_path} returned malformed JSON: {err}") from err


def candidate_sockets(ipc_dir: Path) -> list[Path]:
    try:
        entries = list(ipc_dir.iterdir())
    except OSError as err:
        raise RuntimeError(f"could not read control-plane socket directory {ipc_dir}: {err}") from err

    sockets: list[Path] = []
    for entry in sorted(entries):
        try:
            mode = entry.stat().st_mode
        except OSError:
            continue
        if stat.S_ISSOCK(mode):
            sockets.append(entry)
    return sockets


def main() -> int:
    thread_id = os.environ.get("CODEX_THREAD_ID", "").strip()
    if not thread_id:
        print("CODEX_THREAD_ID is not set", file=sys.stderr)
        return 1

    ipc_dir = load_ipc_dir()
    sockets = candidate_sockets(ipc_dir)
    if not sockets:
        print(f"no local control-plane sockets found in {ipc_dir}", file=sys.stderr)
        return 1

    matching_sockets: list[Path] = []
    for socket_path in sockets:
        try:
            response = request_socket(socket_path, {"command": "current-session"})
        except RuntimeError:
            continue

        payload = response.get("payload")
        if not isinstance(payload, dict):
            continue
        if payload.get("threadId") == thread_id:
            matching_sockets.append(socket_path)

    if not matching_sockets:
        print(
            f"no local control-plane socket matched CODEX_THREAD_ID={thread_id}",
            file=sys.stderr,
        )
        return 1

    if len(matching_sockets) > 1:
        matches = ", ".join(str(path) for path in matching_sockets)
        print(
            f"multiple local control-plane sockets matched CODEX_THREAD_ID={thread_id}: {matches}",
            file=sys.stderr,
        )
        return 1

    socket_path = matching_sockets[0]
    try:
        response = request_socket(socket_path, {"command": "status"})
    except RuntimeError as err:
        print(str(err), file=sys.stderr)
        return 1

    payload = response.get("payload")
    if not isinstance(payload, dict):
        print(f"status response from {socket_path} was missing a payload", file=sys.stderr)
        return 1

    instance_id = payload.get("instanceId")
    if not isinstance(instance_id, str) or not instance_id.strip():
        print(f"status response from {socket_path} did not include instanceId", file=sys.stderr)
        return 1

    print(instance_id)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
