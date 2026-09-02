#!/usr/bin/env python3

import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PERFORMANCE_TOOL = ROOT / "scripts" / "local_performance_test.py"
KEY_INSIGHTS_BIN = Path(
    os.environ.get("KEY_INSIGHTS_BIN", ROOT / "target" / "debug" / "key-insights")
).resolve()
PRIVATE_CANARIES = (
    "performance-session-alpha",
    "PERFORMANCE_PROJECT_PRIVATE_CANARY",
)


def write_session(directory: Path) -> None:
    events = [
        {
            "schema_version": 1,
            "event_type": "session_start",
            "session_id": "performance-session-alpha",
            "elapsed_ms": 0,
            "project_id": "PERFORMANCE_PROJECT_PRIVATE_CANARY",
        },
        {
            "schema_version": 1,
            "event_type": "key_sequence",
            "session_id": "performance-session-alpha",
            "elapsed_ms": 10,
            "mode": "normal",
            "keys": ["j", "j"],
            "duration_ms": 4,
        },
        {
            "schema_version": 1,
            "event_type": "session_end",
            "session_id": "performance-session-alpha",
            "elapsed_ms": 20,
        },
    ]
    path = directory / "nvim-key-insights-performance-session.jsonl"
    path.write_bytes(
        b"".join(
            json.dumps(event, separators=(",", ":"), sort_keys=True).encode() + b"\n"
            for event in events
        )
    )
    path.chmod(0o600)


class LocalPerformanceTestContract(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="nvim-key-insights-local-performance-")
        self.root = Path(self.temporary.name)
        self.sessions = self.root / "sessions"
        self.sessions.mkdir()
        self.sessions.chmod(0o700)
        self.manifest = self.root / "performance-manifest.json"
        self.nvim = self.root / "nvim-version"
        self.nvim.write_text(f"#!{sys.executable}\nprint('NVIM v0.10.0')\n")
        self.nvim.chmod(0o700)
        write_session(self.sessions)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def run_tool(self, **paths: Path) -> subprocess.CompletedProcess[str]:
        arguments = [
            "python3",
            str(PERFORMANCE_TOOL),
            "--session-dir",
            str(paths.get("session_dir", self.sessions)),
            "--manifest",
            str(paths.get("manifest", self.manifest)),
            "--key-insights-bin",
            str(KEY_INSIGHTS_BIN),
            "--nvim-bin",
            str(self.nvim),
        ]
        return subprocess.run(
            arguments,
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
            timeout=30,
        )

    def test_writes_only_bounded_aggregate_performance_manifest(self) -> None:
        self.assertTrue(KEY_INSIGHTS_BIN.is_file(), "build the key-insights binary first")

        completed = self.run_tool()

        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertNotIn(str(self.root), completed.stdout)
        self.assertNotIn(str(self.root), completed.stderr)
        manifest_bytes = self.manifest.read_bytes()
        self.assertLessEqual(len(manifest_bytes), 16 * 1024)
        self.assertEqual(stat.S_IMODE(self.manifest.stat().st_mode), 0o600)
        manifest = json.loads(manifest_bytes)
        self.assertEqual(manifest["manifest_version"], 1)
        self.assertEqual(manifest["mode"], "real-local-performance")
        self.assertEqual(
            manifest["contracts"],
            {"event_schema": 1, "summary_schema": 3, "ergonomics": 2},
        )
        self.assertEqual(manifest["observations"]["session_count"], 1)
        analyzer = manifest["observations"]["analyzer"]
        self.assertEqual(set(analyzer), {"elapsed_ms", "child_max_rss_bytes", "summary_bytes", "report_bytes"})
        for value in analyzer.values():
            self.assertIsInstance(value, int)
            self.assertGreaterEqual(value, 0)
        self.assertGreater(analyzer["summary_bytes"], 0)
        self.assertGreater(analyzer["report_bytes"], 0)
        self.assertEqual(
            manifest["checks"],
            {
                "codex_invoked": False,
                "measurement_artifacts_private": True,
                "network_used": False,
                "raw_artifacts_persisted": False,
                "session_directory_unchanged": True,
                "source_tree_excluded": True,
            },
        )
        manifest_text = manifest_bytes.decode()
        for private_value in (str(self.root), *PRIVATE_CANARIES):
            self.assertNotIn(private_value, manifest_text)

    def test_session_directory_and_manifest_must_be_outside_source_tree(self) -> None:
        completed = self.run_tool(session_dir=ROOT / "tests")
        self.assertNotEqual(completed.returncode, 0)
        self.assertFalse(self.manifest.exists())

        completed = self.run_tool(manifest=ROOT / "performance-manifest.json")
        self.assertNotEqual(completed.returncode, 0)
        self.assertFalse((ROOT / "performance-manifest.json").exists())

    def test_harness_has_no_network_codex_or_shell_execution_surface(self) -> None:
        source = PERFORMANCE_TOOL.read_text()
        for forbidden in (
            "import socket",
            "import urllib",
            "import http",
            "requests",
            "codex exec",
            "shell=True",
        ):
            self.assertNotIn(forbidden, source)


if __name__ == "__main__":
    unittest.main(verbosity=2)
