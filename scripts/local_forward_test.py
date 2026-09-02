#!/usr/bin/env python3

"""Run a deliberate, local-only inspection of real collector artifacts.

This command deliberately requires a person to confirm each privacy boundary.
It never prints or stores artifact contents, and its manifest contains only
aggregate observations and tool versions.
"""

from __future__ import annotations

import argparse
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
OWNER_DIRECTORY = 0o700
OWNER_FILE = 0o600


class LocalForwardTestError(Exception):
    pass


def outside_source_tree(path: Path) -> bool:
    return path != ROOT and ROOT not in path.parents


def owner_only_directory(raw_path: str, label: str, *, require_empty: bool) -> Path:
    candidate = Path(raw_path)
    if not candidate.is_absolute():
        raise LocalForwardTestError(f"{label} must be an absolute path")
    try:
        leaf = candidate.lstat()
        resolved = candidate.resolve(strict=True)
    except OSError as error:
        raise LocalForwardTestError(f"{label} is unavailable") from error
    if stat.S_ISLNK(leaf.st_mode) or not stat.S_ISDIR(leaf.st_mode):
        raise LocalForwardTestError(f"{label} must be a real directory")
    if not outside_source_tree(resolved):
        raise LocalForwardTestError(f"{label} must be outside the source tree")
    if hasattr(os, "geteuid") and leaf.st_uid != os.geteuid():
        raise LocalForwardTestError(f"{label} must be owned by the current user")
    if stat.S_IMODE(leaf.st_mode) != OWNER_DIRECTORY:
        raise LocalForwardTestError(f"{label} must have mode 0700")
    try:
        entries = list(resolved.iterdir())
    except OSError as error:
        raise LocalForwardTestError(f"{label} cannot be inspected") from error
    if require_empty and entries:
        raise LocalForwardTestError(f"{label} must be empty")
    return resolved


def private_manifest_path(raw_path: str) -> Path:
    candidate = Path(raw_path)
    if not candidate.is_absolute():
        raise LocalForwardTestError("manifest must be an absolute path")
    parent = candidate.parent
    try:
        parent_leaf = parent.lstat()
        resolved_parent = parent.resolve(strict=True)
    except OSError as error:
        raise LocalForwardTestError("manifest path cannot be inspected") from error
    try:
        candidate_leaf = candidate.lstat()
    except FileNotFoundError:
        candidate_leaf = None
    except OSError as error:
        raise LocalForwardTestError("manifest path cannot be inspected") from error
    if not outside_source_tree(resolved_parent):
        raise LocalForwardTestError("manifest must be outside the source tree")
    if stat.S_ISLNK(parent_leaf.st_mode) or not stat.S_ISDIR(parent_leaf.st_mode):
        raise LocalForwardTestError("manifest parent must be a real directory")
    if hasattr(os, "geteuid") and parent_leaf.st_uid != os.geteuid():
        raise LocalForwardTestError("manifest parent must be owned by the current user")
    if stat.S_IMODE(parent_leaf.st_mode) != OWNER_DIRECTORY:
        raise LocalForwardTestError("manifest parent must have mode 0700")
    if candidate_leaf is not None:
        raise LocalForwardTestError("manifest must not already exist")
    return resolved_parent / candidate.name


def private_executable(raw_path: str, label: str) -> Path:
    candidate = Path(raw_path)
    if not candidate.is_absolute():
        raise LocalForwardTestError(f"{label} must be an absolute path")
    try:
        leaf = candidate.lstat()
        resolved = candidate.resolve(strict=True)
    except OSError as error:
        raise LocalForwardTestError(f"{label} is unavailable") from error
    if stat.S_ISLNK(leaf.st_mode) or not stat.S_ISREG(leaf.st_mode):
        raise LocalForwardTestError(f"{label} must be a regular non-symlink file")
    if not os.access(candidate, os.X_OK):
        raise LocalForwardTestError(f"{label} must be executable")
    return resolved


def directory_identity(path: Path) -> tuple[int, int]:
    try:
        metadata = path.stat()
    except OSError as error:
        raise LocalForwardTestError("private directory cannot be inspected") from error
    return metadata.st_dev, metadata.st_ino


def discoverable_session_count(session_directory: Path) -> int:
    count = 0
    try:
        entries = list(session_directory.iterdir())
    except OSError as error:
        raise LocalForwardTestError("session directory cannot be inspected") from error
    for entry in entries:
        if not entry.name.startswith("nvim-key-insights-") or not entry.name.endswith(".jsonl"):
            continue
        session_id = entry.name[len("nvim-key-insights-") : -len(".jsonl")]
        if (
            not 1 <= len(session_id) <= 128
            or any(
                character not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-"
                for character in session_id
            )
        ):
            continue
        try:
            metadata = entry.lstat()
        except OSError as error:
            raise LocalForwardTestError("session directory changed during inspection") from error
        if (
            stat.S_ISREG(metadata.st_mode)
            and metadata.st_nlink == 1
            and stat.S_IMODE(metadata.st_mode) == OWNER_FILE
            and (not hasattr(os, "geteuid") or metadata.st_uid == os.geteuid())
        ):
            count += 1
    if count == 0:
        raise LocalForwardTestError("session directory contains no private finalized sessions")
    return count


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
        raise LocalForwardTestError("local analyzer execution failed") from error
    if completed.returncode != 0:
        raise LocalForwardTestError("local analyzer rejected the private workflow")


def bounded_private_read(path: Path, limit: int, label: str) -> bytes:
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
    except OSError as error:
        raise LocalForwardTestError(f"{label} is unavailable") from error
    try:
        before = os.fstat(descriptor)
        if (
            not stat.S_ISREG(before.st_mode)
            or before.st_nlink != 1
            or stat.S_IMODE(before.st_mode) != OWNER_FILE
            or (hasattr(os, "geteuid") and before.st_uid != os.geteuid())
            or before.st_size <= 0
            or before.st_size > limit
        ):
            raise LocalForwardTestError(f"{label} is not a bounded private regular file")
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
        if (
            len(payload) > limit
            or before.st_size != len(payload)
            or after.st_size != len(payload)
            or (before.st_dev, before.st_ino) != (after.st_dev, after.st_ino)
        ):
            raise LocalForwardTestError(f"{label} changed during inspection")
        return payload
    except OSError as error:
        raise LocalForwardTestError(f"{label} cannot be read") from error
    finally:
        os.close(descriptor)


def write_all(descriptor: int, payload: bytes, label: str) -> None:
    view = memoryview(payload)
    while view:
        try:
            written = os.write(descriptor, view)
        except OSError as error:
            raise LocalForwardTestError(f"{label} cannot be written") from error
        if written <= 0:
            raise LocalForwardTestError(f"{label} cannot be written")
        view = view[written:]


def tool_version(binary: Path, label: str) -> str:
    try:
        completed = subprocess.run(
            [str(binary), "--version"],
            cwd=ROOT,
            env={},
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            check=False,
            timeout=10,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise LocalForwardTestError(f"failed to inspect {label} version") from error
    if completed.returncode != 0:
        raise LocalForwardTestError(f"failed to inspect {label} version")
    try:
        version = completed.stdout.decode("utf-8").splitlines()[0].strip()
    except (UnicodeDecodeError, IndexError) as error:
        raise LocalForwardTestError(f"{label} reported an invalid version") from error
    if not version or len(version) > 256 or "/" in version or "\\" in version:
        raise LocalForwardTestError(f"{label} reported an unsafe version")
    return version


def require_human_inspection(label: str, path: Path, instruction: str) -> None:
    print(f"Inspect {label} locally: {path}")
    print(instruction)
    try:
        answer = input("Type 'yes' after completing this inspection: ")
    except EOFError as error:
        raise LocalForwardTestError(f"human inspection required for {label}") from error
    if answer.strip().lower() != "yes":
        raise LocalForwardTestError(f"human inspection was not confirmed for {label}")


def validate_generated_artifacts(summary: bytes, report: bytes, payload: bytes) -> None:
    try:
        summary_document = json.loads(summary)
        payload_document = json.loads(payload)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise LocalForwardTestError("generated JSON artifact is invalid") from error
    if summary_document.get("schema_version") != 3:
        raise LocalForwardTestError("generated summary schema is unsupported")
    if payload_document.get("payload_schema_version") != 1:
        raise LocalForwardTestError("generated payload schema is unsupported")
    if not report.startswith(b"# Neovim Key Insights"):
        raise LocalForwardTestError("generated report contract is invalid")


def execute(
    session_directory: Path,
    report_directory: Path,
    manifest_path: Path,
    analyzer: Path,
    nvim: Path,
) -> bytes:
    if directory_identity(session_directory) == directory_identity(report_directory):
        raise LocalForwardTestError("session and report directories must be distinct")
    session_count = discoverable_session_count(session_directory)
    summary_path = report_directory / "summary.json"
    report_path = report_directory / "report.md"
    payload_path = report_directory / "payload.json"

    run_local(
        analyzer,
        report_directory,
        [
            "analyze",
            "--session-dir",
            str(session_directory),
            "--summary",
            str(summary_path),
            "--report",
            str(report_path),
        ],
    )
    run_local(
        analyzer,
        report_directory,
        ["preview", str(summary_path), "--output", str(payload_path)],
    )
    if discoverable_session_count(session_directory) != session_count:
        raise LocalForwardTestError("session directory changed during inspection")

    summary = bounded_private_read(summary_path, MAX_SUMMARY_BYTES, "summary")
    report = bounded_private_read(report_path, MAX_REPORT_BYTES, "report")
    payload = bounded_private_read(payload_path, MAX_PAYLOAD_BYTES, "payload")
    validate_generated_artifacts(summary, report, payload)

    require_human_inspection(
        "private collector JSONL",
        session_directory,
        "Check that collection contains no typed text, paths, command/search contents, or mapping implementations.",
    )
    require_human_inspection(
        "sanitized summary.json",
        summary_path,
        "Check the aggregate metrics and confirm that session IDs, project IDs, and paths are absent.",
    )
    require_human_inspection(
        "local report.md",
        report_path,
        "Check that the report is a useful deterministic local rendering and contains no private content.",
    )
    require_human_inspection(
        "canonical Codex preview payload.json",
        payload_path,
        "Check that this is the exact sanitized payload you would approve; do not send it yet.",
    )

    manifest = {
        "manifest_version": 1,
        "mode": "real-local",
        "contracts": {"event_schema": 1, "payload_schema": 1, "summary_schema": 3},
        "tools": {
            "key_insights": tool_version(analyzer, "key-insights"),
            "neovim": tool_version(nvim, "Neovim"),
            "python": f"{sys.version_info.major}.{sys.version_info.minor}.{sys.version_info.micro}",
        },
        "observations": {"session_count": session_count},
        "checks": {
            "codex_invoked": False,
            "human_inspection_required": True,
            "jsonl_inspected": True,
            "network_used": False,
            "payload_inspected": True,
            "private_inputs_used": True,
            "report_inspected": True,
            "source_tree_excluded": True,
            "summary_inspected": True,
        },
    }
    manifest_bytes = json.dumps(manifest, separators=(",", ":"), sort_keys=True).encode()
    if len(manifest_bytes) > MAX_MANIFEST_BYTES:
        raise LocalForwardTestError("inspection manifest exceeds its bound")
    try:
        descriptor = os.open(
            manifest_path,
            os.O_CREAT
            | os.O_EXCL
            | os.O_WRONLY
            | getattr(os, "O_CLOEXEC", 0)
            | getattr(os, "O_NOFOLLOW", 0),
            OWNER_FILE,
        )
    except OSError as error:
        raise LocalForwardTestError("inspection manifest cannot be created") from error
    write_all(descriptor, manifest_bytes, "inspection manifest")
    try:
        os.fsync(descriptor)
    except OSError as error:
        raise LocalForwardTestError("inspection manifest cannot be written") from error
    finally:
        os.close(descriptor)
    return manifest_bytes


def parse_arguments(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run a deliberate local inspection of real key-insights artifacts"
    )
    parser.add_argument("--session-dir", required=True)
    parser.add_argument("--report-dir", required=True)
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--key-insights-bin", required=True)
    parser.add_argument("--nvim-bin", required=True)
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    try:
        arguments = parse_arguments(argv)
        session_directory = owner_only_directory(
            arguments.session_dir, "session directory", require_empty=False
        )
        report_directory = owner_only_directory(
            arguments.report_dir, "report directory", require_empty=True
        )
        manifest_path = private_manifest_path(arguments.manifest)
        analyzer = private_executable(arguments.key_insights_bin, "key-insights binary")
        nvim = private_executable(arguments.nvim_bin, "Neovim binary")
        manifest = execute(session_directory, report_directory, manifest_path, analyzer, nvim)
    except LocalForwardTestError as error:
        print(f"local-forward-test: {error}", file=sys.stderr)
        return 1
    sys.stdout.buffer.write(manifest + b"\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
