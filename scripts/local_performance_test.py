#!/usr/bin/env python3

"""Measure the local analyzer against private finalized sessions.

Only aggregate resource observations are written to the requested manifest.
The analyzer's temporary summary and report are removed when this process
exits; this command never invokes Codex or a network service.
"""

from __future__ import annotations

import argparse
import json
import os
import resource
import signal
import stat
import subprocess
import sys
import tempfile
import time
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MAX_DIRECTORY_ENTRIES = 8192
MAX_SUMMARY_BYTES = 16 * 1024 * 1024
MAX_REPORT_BYTES = 1024 * 1024
MAX_MANIFEST_BYTES = 16 * 1024
OWNER_DIRECTORY = 0o700
OWNER_FILE = 0o600


class LocalPerformanceTestError(Exception):
    pass


def outside_source_tree(path: Path) -> bool:
    return path != ROOT and ROOT not in path.parents


def owner_only_directory(raw_path: str, label: str) -> Path:
    candidate = Path(raw_path)
    if not candidate.is_absolute():
        raise LocalPerformanceTestError(f"{label} must be an absolute path")
    try:
        leaf = candidate.lstat()
        resolved = candidate.resolve(strict=True)
    except OSError as error:
        raise LocalPerformanceTestError(f"{label} is unavailable") from error
    if stat.S_ISLNK(leaf.st_mode) or not stat.S_ISDIR(leaf.st_mode):
        raise LocalPerformanceTestError(f"{label} must be a real directory")
    if not outside_source_tree(resolved):
        raise LocalPerformanceTestError(f"{label} must be outside the source tree")
    if hasattr(os, "geteuid") and leaf.st_uid != os.geteuid():
        raise LocalPerformanceTestError(f"{label} must be owned by the current user")
    if stat.S_IMODE(leaf.st_mode) != OWNER_DIRECTORY:
        raise LocalPerformanceTestError(f"{label} must have mode 0700")
    return resolved


def private_manifest_path(raw_path: str) -> Path:
    candidate = Path(raw_path)
    if not candidate.is_absolute():
        raise LocalPerformanceTestError("manifest must be an absolute path")
    parent = candidate.parent
    try:
        parent_leaf = parent.lstat()
        resolved_parent = parent.resolve(strict=True)
    except OSError as error:
        raise LocalPerformanceTestError("manifest path cannot be inspected") from error
    try:
        candidate_leaf = candidate.lstat()
    except FileNotFoundError:
        candidate_leaf = None
    except OSError as error:
        raise LocalPerformanceTestError("manifest path cannot be inspected") from error
    if not outside_source_tree(resolved_parent):
        raise LocalPerformanceTestError("manifest must be outside the source tree")
    if stat.S_ISLNK(parent_leaf.st_mode) or not stat.S_ISDIR(parent_leaf.st_mode):
        raise LocalPerformanceTestError("manifest parent must be a real directory")
    if hasattr(os, "geteuid") and parent_leaf.st_uid != os.geteuid():
        raise LocalPerformanceTestError("manifest parent must be owned by the current user")
    if stat.S_IMODE(parent_leaf.st_mode) != OWNER_DIRECTORY:
        raise LocalPerformanceTestError("manifest parent must have mode 0700")
    if candidate_leaf is not None:
        raise LocalPerformanceTestError("manifest must not already exist")
    return resolved_parent / candidate.name


def private_executable(raw_path: str, label: str) -> Path:
    candidate = Path(raw_path)
    if not candidate.is_absolute():
        raise LocalPerformanceTestError(f"{label} must be an absolute path")
    try:
        leaf = candidate.lstat()
        resolved = candidate.resolve(strict=True)
    except OSError as error:
        raise LocalPerformanceTestError(f"{label} is unavailable") from error
    if stat.S_ISLNK(leaf.st_mode) or not stat.S_ISREG(leaf.st_mode):
        raise LocalPerformanceTestError(f"{label} must be a regular non-symlink file")
    if not os.access(candidate, os.X_OK):
        raise LocalPerformanceTestError(f"{label} must be executable")
    return resolved


def directory_identity(path: Path) -> tuple[int, int]:
    try:
        metadata = path.stat()
    except OSError as error:
        raise LocalPerformanceTestError("session directory cannot be inspected") from error
    return metadata.st_dev, metadata.st_ino


def discoverable_session_count(session_directory: Path) -> int:
    count = 0
    scanned = 0
    try:
        entries = session_directory.iterdir()
        for entry in entries:
            scanned += 1
            if scanned > MAX_DIRECTORY_ENTRIES:
                raise LocalPerformanceTestError("session directory scan exceeds its bound")
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
                raise LocalPerformanceTestError("session directory changed during inspection") from error
            if (
                stat.S_ISREG(metadata.st_mode)
                and metadata.st_nlink == 1
                and stat.S_IMODE(metadata.st_mode) == OWNER_FILE
                and (not hasattr(os, "geteuid") or metadata.st_uid == os.geteuid())
            ):
                count += 1
    except OSError as error:
        raise LocalPerformanceTestError("session directory cannot be inspected") from error
    if count == 0:
        raise LocalPerformanceTestError("session directory contains no private finalized sessions")
    return count


def read_bounded(path: Path, limit: int, label: str) -> bytes:
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
    except OSError as error:
        raise LocalPerformanceTestError(f"{label} is unavailable") from error
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
            raise LocalPerformanceTestError(f"{label} is not a bounded private regular file")
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
            raise LocalPerformanceTestError(f"{label} changed during inspection")
        return payload
    except OSError as error:
        raise LocalPerformanceTestError(f"{label} cannot be read") from error
    finally:
        os.close(descriptor)


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
        raise LocalPerformanceTestError(f"failed to inspect {label} version") from error
    if completed.returncode != 0:
        raise LocalPerformanceTestError(f"failed to inspect {label} version")
    try:
        version = completed.stdout.decode("utf-8").splitlines()[0].strip()
    except (UnicodeDecodeError, IndexError) as error:
        raise LocalPerformanceTestError(f"{label} reported an invalid version") from error
    if not version or len(version) > 256 or "/" in version or "\\" in version:
        raise LocalPerformanceTestError(f"{label} reported an unsafe version")
    return version


def child_max_rss_bytes(usage: resource.struct_rusage) -> int:
    raw = int(usage.ru_maxrss)
    return raw if sys.platform == "darwin" else raw * 1024


def wait_for_analyzer(process: subprocess.Popen[bytes]) -> tuple[int, resource.struct_rusage]:
    if not hasattr(os, "wait4"):
        process.kill()
        process.wait()
        raise LocalPerformanceTestError("direct analyzer resource accounting is unavailable")

    deadline = time.monotonic() + 60
    while True:
        try:
            waited_pid, status, usage = os.wait4(process.pid, os.WNOHANG)
        except OSError as error:
            raise LocalPerformanceTestError("local analyzer performance run failed") from error
        if waited_pid == process.pid:
            process.returncode = os.waitstatus_to_exitcode(status)
            return status, usage
        if time.monotonic() >= deadline:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except OSError:
                process.kill()
            try:
                _, status, _ = os.wait4(process.pid, 0)
                process.returncode = os.waitstatus_to_exitcode(status)
            except ChildProcessError:
                pass
            raise LocalPerformanceTestError("local analyzer performance run timed out")
        time.sleep(0.01)


def measure_analyzer(binary: Path, session_directory: Path, workspace: Path) -> dict[str, int]:
    summary_path = workspace / "summary.json"
    report_path = workspace / "report.md"
    started = time.perf_counter_ns()
    try:
        process = subprocess.Popen(
            [
                str(binary),
                "analyze",
                "--session-dir",
                str(session_directory),
                "--summary",
                str(summary_path),
                "--report",
                str(report_path),
            ],
            cwd=workspace,
            env={},
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        status, usage = wait_for_analyzer(process)
    except OSError as error:
        raise LocalPerformanceTestError("local analyzer performance run failed") from error
    elapsed_ms = max(1, (time.perf_counter_ns() - started) // 1_000_000)
    returncode = os.waitstatus_to_exitcode(status)
    if returncode != 0:
        raise LocalPerformanceTestError("local analyzer rejected the private performance run")

    try:
        summary_bytes = read_bounded(summary_path, MAX_SUMMARY_BYTES, "summary")
        report_bytes = read_bounded(report_path, MAX_REPORT_BYTES, "report")
        summary = json.loads(summary_bytes)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise LocalPerformanceTestError("local analyzer produced invalid JSON") from error
    if (
        not isinstance(summary, dict)
        or summary.get("schema_version") != 3
        or not isinstance(summary.get("ergonomics"), dict)
        or summary["ergonomics"].get("contract_version") != 2
    ):
        raise LocalPerformanceTestError("local analyzer produced an unsupported summary")
    return {
        "elapsed_ms": elapsed_ms,
        "child_max_rss_bytes": child_max_rss_bytes(usage),
        "summary_bytes": len(summary_bytes),
        "report_bytes": len(report_bytes),
    }


def write_manifest(path: Path, manifest: dict[str, object]) -> bytes:
    payload = json.dumps(manifest, separators=(",", ":"), sort_keys=True).encode()
    if len(payload) > MAX_MANIFEST_BYTES:
        raise LocalPerformanceTestError("performance manifest exceeds its bound")
    try:
        descriptor = os.open(
            path,
            os.O_CREAT
            | os.O_EXCL
            | os.O_WRONLY
            | getattr(os, "O_CLOEXEC", 0)
            | getattr(os, "O_NOFOLLOW", 0),
            OWNER_FILE,
        )
    except OSError as error:
        raise LocalPerformanceTestError("performance manifest cannot be created") from error
    try:
        view = memoryview(payload)
        while view:
            written = os.write(descriptor, view)
            if written <= 0:
                raise LocalPerformanceTestError("performance manifest cannot be written")
            view = view[written:]
        os.fsync(descriptor)
    except OSError as error:
        raise LocalPerformanceTestError("performance manifest cannot be written") from error
    finally:
        os.close(descriptor)
    return payload


def execute(
    session_directory: Path,
    manifest_path: Path,
    analyzer: Path,
    nvim: Path,
) -> bytes:
    session_identity = directory_identity(session_directory)
    session_count = discoverable_session_count(session_directory)
    analyzer_version = tool_version(analyzer, "key-insights")
    nvim_version = tool_version(nvim, "Neovim")

    with tempfile.TemporaryDirectory(prefix="nvim-key-insights-performance-") as temporary:
        workspace = Path(temporary)
        workspace.chmod(OWNER_DIRECTORY)
        measurements = measure_analyzer(analyzer, session_directory, workspace)

    if directory_identity(session_directory) != session_identity:
        raise LocalPerformanceTestError("session directory changed during measurement")
    if discoverable_session_count(session_directory) != session_count:
        raise LocalPerformanceTestError("session directory contents changed during measurement")

    manifest = {
        "manifest_version": 1,
        "mode": "real-local-performance",
        "contracts": {"event_schema": 1, "summary_schema": 3, "ergonomics": 2},
        "tools": {
            "key_insights": analyzer_version,
            "neovim": nvim_version,
            "python": f"{sys.version_info.major}.{sys.version_info.minor}.{sys.version_info.micro}",
        },
        "observations": {"session_count": session_count, "analyzer": measurements},
        "checks": {
            "codex_invoked": False,
            "measurement_artifacts_private": True,
            "network_used": False,
            "raw_artifacts_persisted": False,
            "session_directory_unchanged": True,
            "source_tree_excluded": True,
        },
    }
    return write_manifest(manifest_path, manifest)


def parse_arguments(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Measure the local analyzer using private finalized sessions"
    )
    parser.add_argument("--session-dir", required=True)
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--key-insights-bin", required=True)
    parser.add_argument("--nvim-bin", required=True)
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    try:
        arguments = parse_arguments(argv)
        session_directory = owner_only_directory(arguments.session_dir, "session directory")
        manifest_path = private_manifest_path(arguments.manifest)
        analyzer = private_executable(arguments.key_insights_bin, "key-insights binary")
        nvim = private_executable(arguments.nvim_bin, "Neovim binary")
        manifest = execute(session_directory, manifest_path, analyzer, nvim)
    except LocalPerformanceTestError as error:
        print(f"local-performance-test: {error}", file=sys.stderr)
        return 1
    sys.stdout.buffer.write(manifest + b"\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
