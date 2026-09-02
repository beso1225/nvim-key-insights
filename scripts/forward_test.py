#!/usr/bin/env python3

import argparse
import hashlib
import json
import os
import stat
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MAX_MANIFEST_BYTES = 16 * 1024
MAX_SUMMARY_BYTES = 16 * 1024 * 1024
MAX_REPORT_BYTES = 1024 * 1024
MAX_PAYLOAD_BYTES = 256 * 1024
SESSION_IDS = ("forward-session-alpha", "forward-session-beta")
PROJECT_CANARY = "FORWARD_PROJECT_PRIVATE_CANARY"
ADJACENT_CANARY = "FORWARD_ADJACENT_PRIVATE_CANARY"


class ForwardTestError(Exception):
    pass


def private_empty_workspace(raw_path: str) -> Path:
    candidate = Path(raw_path)
    if not candidate.is_absolute():
        raise ForwardTestError("workspace must be absolute")
    try:
        leaf = candidate.lstat()
    except OSError as error:
        raise ForwardTestError("workspace is unavailable") from error
    if stat.S_ISLNK(leaf.st_mode) or not stat.S_ISDIR(leaf.st_mode):
        raise ForwardTestError("workspace must be a real directory")
    if hasattr(os, "geteuid") and leaf.st_uid != os.geteuid():
        raise ForwardTestError("workspace must be owned by the current user")
    if stat.S_IMODE(leaf.st_mode) != 0o700:
        raise ForwardTestError("workspace must have mode 0700")
    try:
        workspace = candidate.resolve(strict=True)
        entries = list(workspace.iterdir())
    except OSError as error:
        raise ForwardTestError("workspace cannot be inspected") from error
    if workspace == ROOT or ROOT in workspace.parents:
        raise ForwardTestError("workspace must be outside the source tree")
    if entries:
        raise ForwardTestError("workspace must be empty")
    return workspace


def analyzer_binary(raw_path: str) -> Path:
    candidate = Path(raw_path)
    if not candidate.is_absolute():
        raise ForwardTestError("analyzer binary must be absolute")
    try:
        leaf = candidate.lstat()
    except OSError as error:
        raise ForwardTestError("analyzer binary is unavailable") from error
    if stat.S_ISLNK(leaf.st_mode) or not stat.S_ISREG(leaf.st_mode):
        raise ForwardTestError("analyzer binary must be a regular non-symlink file")
    if not os.access(candidate, os.X_OK):
        raise ForwardTestError("analyzer binary must be executable")
    return candidate.resolve(strict=True)


def write_private_new(path: Path, payload: bytes) -> None:
    flags = os.O_CREAT | os.O_EXCL | os.O_WRONLY
    flags |= getattr(os, "O_CLOEXEC", 0)
    flags |= getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags, 0o600)
        try:
            view = memoryview(payload)
            while view:
                written = os.write(descriptor, view)
                if written <= 0:
                    raise ForwardTestError("private artifact write failed")
                view = view[written:]
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
    except FileExistsError as error:
        raise ForwardTestError("private artifact already exists") from error
    except OSError as error:
        raise ForwardTestError("private artifact write failed") from error


def synthetic_jsonl() -> bytes:
    events = [
        {
            "schema_version": 1,
            "event_type": "session_start",
            "session_id": SESSION_IDS[0],
            "elapsed_ms": 0,
            "project_id": PROJECT_CANARY,
        },
        {
            "schema_version": 1,
            "event_type": "key_sequence",
            "session_id": SESSION_IDS[0],
            "elapsed_ms": 40,
            "mode": "normal",
            "keys": ["j", "j", "d", "d"],
            "duration_ms": 30,
        },
        {
            "schema_version": 1,
            "event_type": "text_run",
            "session_id": SESSION_IDS[0],
            "elapsed_ms": 70,
            "key_count": 5,
            "duration_ms": 20,
        },
        {
            "schema_version": 1,
            "event_type": "session_end",
            "session_id": SESSION_IDS[0],
            "elapsed_ms": 100,
        },
        {
            "schema_version": 1,
            "event_type": "session_start",
            "session_id": SESSION_IDS[1],
            "elapsed_ms": 0,
        },
        {
            "schema_version": 1,
            "event_type": "key_sequence",
            "session_id": SESSION_IDS[1],
            "elapsed_ms": 10,
            "mode": "visual",
            "keys": ["x"],
            "duration_ms": 0,
        },
        {
            "schema_version": 1,
            "event_type": "key_sequence",
            "session_id": SESSION_IDS[1],
            "elapsed_ms": 20,
            "mode": "operator_pending",
            "keys": ["d", "w"],
            "duration_ms": 4,
        },
        {
            "schema_version": 1,
            "event_type": "session_end",
            "session_id": SESSION_IDS[1],
            "elapsed_ms": 30,
        },
    ]
    return b"".join(
        json.dumps(event, separators=(",", ":"), sort_keys=True).encode() + b"\n"
        for event in events
    )


def run_local(binary: Path, workspace: Path, arguments: list[str]) -> None:
    try:
        completed = subprocess.run(
            [str(binary), *arguments],
            cwd=workspace,
            env={},
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
            timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ForwardTestError("local analyzer execution failed") from error
    if completed.returncode != 0:
        raise ForwardTestError("local analyzer rejected the synthetic workflow")


def read_private_bounded(path: Path, limit: int) -> bytes:
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0)
    flags |= getattr(os, "O_NOFOLLOW", 0)
    flags |= getattr(os, "O_NONBLOCK", 0)
    try:
        descriptor = os.open(path, flags)
    except OSError as error:
        raise ForwardTestError("expected artifact is unavailable") from error
    try:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode):
            raise ForwardTestError("expected artifact is unsafe")
        if hasattr(os, "geteuid") and before.st_uid != os.geteuid():
            raise ForwardTestError("expected artifact has an unsafe owner")
        if before.st_nlink != 1 or stat.S_IMODE(before.st_mode) != 0o600:
            raise ForwardTestError("expected artifact is not private")
        if before.st_size <= 0 or before.st_size > limit:
            raise ForwardTestError("expected artifact exceeds its bound")
        chunks: list[bytes] = []
        consumed = 0
        while consumed <= limit:
            chunk = os.read(descriptor, min(64 * 1024, limit + 1 - consumed))
            if not chunk:
                break
            chunks.append(chunk)
            consumed += len(chunk)
        payload = b"".join(chunks)
        after = os.fstat(descriptor)
        if len(payload) > limit:
            raise ForwardTestError("expected artifact exceeds its bound")
        if (before.st_dev, before.st_ino) != (after.st_dev, after.st_ino):
            raise ForwardTestError("expected artifact changed during inspection")
        if (
            not stat.S_ISREG(after.st_mode)
            or (hasattr(os, "geteuid") and after.st_uid != os.geteuid())
            or after.st_nlink != 1
            or stat.S_IMODE(after.st_mode) != 0o600
        ):
            raise ForwardTestError("expected artifact changed during inspection")
        if before.st_size != len(payload) or after.st_size != len(payload):
            raise ForwardTestError("expected artifact changed during inspection")
        return payload
    except OSError as error:
        raise ForwardTestError("expected artifact cannot be read") from error
    finally:
        os.close(descriptor)


def ensure_sanitized(payloads: list[bytes], workspace: Path) -> None:
    forbidden = [
        str(workspace).encode(),
        PROJECT_CANARY.encode(),
        ADJACENT_CANARY.encode(),
        *(session_id.encode() for session_id in SESSION_IDS),
    ]
    if any(canary in payload for payload in payloads for canary in forbidden):
        raise ForwardTestError("a private canary crossed a sanitized boundary")


def artifact_metadata(payload: bytes) -> dict[str, int | str]:
    return {"bytes": len(payload), "sha256": hashlib.sha256(payload).hexdigest()}


def execute(workspace: Path, binary: Path) -> bytes:
    session_path = workspace / "synthetic-sessions.jsonl"
    summary_path = workspace / "summary.json"
    report_path = workspace / "report.md"
    payload_path = workspace / "payload.json"
    adjacent_path = workspace / "adjacent-private-canary.txt"
    manifest_path = workspace / "inspection-manifest.json"

    write_private_new(session_path, synthetic_jsonl())
    write_private_new(adjacent_path, ADJACENT_CANARY.encode())
    run_local(
        binary,
        workspace,
        [
            "analyze",
            str(session_path),
            "--summary",
            str(summary_path),
            "--report",
            str(report_path),
        ],
    )
    run_local(
        binary,
        workspace,
        ["preview", str(summary_path), "--output", str(payload_path)],
    )

    summary = read_private_bounded(summary_path, MAX_SUMMARY_BYTES)
    report = read_private_bounded(report_path, MAX_REPORT_BYTES)
    payload = read_private_bounded(payload_path, MAX_PAYLOAD_BYTES)
    ensure_sanitized([summary, report, payload], workspace)
    try:
        summary_document = json.loads(summary)
        payload_document = json.loads(payload)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ForwardTestError("generated JSON artifact is invalid") from error
    if summary_document.get("schema_version") != 3:
        raise ForwardTestError("generated summary schema is unsupported")
    if payload_document.get("payload_schema_version") != 1:
        raise ForwardTestError("generated payload schema is unsupported")
    if not report.startswith(b"# Neovim Key Insights"):
        raise ForwardTestError("generated report contract is invalid")

    manifest = {
        "manifest_version": 1,
        "mode": "synthetic-offline",
        "contracts": {"event_schema": 1, "payload_schema": 1, "summary_schema": 3},
        "artifacts": {
            "payload": artifact_metadata(payload),
            "report": artifact_metadata(report),
            "summary": artifact_metadata(summary),
        },
        "checks": {
            "codex_invoked": False,
            "network_used": False,
            "private_inputs_used": False,
            "sanitized_boundaries": True,
        },
    }
    manifest_bytes = json.dumps(manifest, separators=(",", ":"), sort_keys=True).encode()
    if len(manifest_bytes) > MAX_MANIFEST_BYTES:
        raise ForwardTestError("inspection manifest exceeds its bound")
    ensure_sanitized([manifest_bytes], workspace)
    write_private_new(manifest_path, manifest_bytes)
    return manifest_bytes


def parse_arguments(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run the synthetic offline forward-test contract")
    parser.add_argument("--workspace", required=True)
    parser.add_argument("--key-insights-bin", required=True)
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    try:
        arguments = parse_arguments(argv)
        workspace = private_empty_workspace(arguments.workspace)
        binary = analyzer_binary(arguments.key_insights_bin)
        manifest = execute(workspace, binary)
    except ForwardTestError as error:
        print(f"forward-test: {error}", file=sys.stderr)
        return 1
    sys.stdout.buffer.write(manifest + b"\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
