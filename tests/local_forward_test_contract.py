#!/usr/bin/env python3

import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
FORWARD_TOOL = ROOT / "scripts" / "local_forward_test.py"
KEY_INSIGHTS_BIN = Path(
    os.environ.get("KEY_INSIGHTS_BIN", ROOT / "target" / "debug" / "key-insights")
).resolve()
PRIVATE_CANARIES = (
    "forward-session-alpha",
    "forward-session-beta",
    "FORWARD_PROJECT_PRIVATE_CANARY",
    "FORWARD_ADJACENT_PRIVATE_CANARY",
)


SYNTHETIC_EVENTS = (
    {
        "schema_version": 1,
        "event_type": "session_start",
        "session_id": "forward-session-alpha",
        "elapsed_ms": 0,
        "project_id": "FORWARD_PROJECT_PRIVATE_CANARY",
    },
    {
        "schema_version": 1,
        "event_type": "key_sequence",
        "session_id": "forward-session-alpha",
        "elapsed_ms": 10,
        "mode": "normal",
        "keys": ["j", "j"],
        "duration_ms": 4,
    },
    {
        "schema_version": 1,
        "event_type": "session_end",
        "session_id": "forward-session-alpha",
        "elapsed_ms": 20,
    },
)


def write_session(directory: Path) -> None:
    path = directory / "nvim-key-insights-forward-session.jsonl"
    payload = b"".join(
        json.dumps(event, separators=(",", ":"), sort_keys=True).encode() + b"\n"
        for event in SYNTHETIC_EVENTS
    )
    path.write_bytes(payload)
    path.chmod(0o600)


class LocalForwardTestContract(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="nvim-key-insights-local-forward-")
        self.root = Path(self.temporary.name)
        self.sessions = self.root / "sessions"
        self.reports = self.root / "reports"
        self.manifest = self.root / "inspection-manifest.json"
        self.nvim = self.root / "nvim-version"
        self.nvim.write_text(f"#!{sys.executable}\nprint('NVIM v0.10.0')\n")
        self.nvim.chmod(0o700)
        for directory in (self.sessions, self.reports):
            directory.mkdir()
            directory.chmod(0o700)
        write_session(self.sessions)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def run_tool(self, answers: str, **paths: Path) -> subprocess.CompletedProcess[str]:
        arguments = [
            "python3",
            str(FORWARD_TOOL),
            "--session-dir",
            str(paths.get("session_dir", self.sessions)),
            "--report-dir",
            str(paths.get("report_dir", self.reports)),
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
            input=answers,
            capture_output=True,
            text=True,
            check=False,
            timeout=30,
        )

    def test_requires_four_human_boundary_inspections_and_writes_only_aggregate_manifest(
        self,
    ) -> None:
        self.assertTrue(KEY_INSIGHTS_BIN.is_file(), "build the key-insights binary first")

        completed = self.run_tool("yes\nyes\nyes\nyes\n")
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(completed.stderr, "")

        manifest_bytes = self.manifest.read_bytes()
        self.assertLessEqual(len(manifest_bytes), 16 * 1024)
        self.assertEqual(stat.S_IMODE(self.manifest.stat().st_mode), 0o600)
        manifest = json.loads(manifest_bytes)
        self.assertEqual(manifest["manifest_version"], 1)
        self.assertEqual(manifest["mode"], "real-local")
        self.assertEqual(
            manifest["checks"],
            {
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
        )
        self.assertEqual(manifest["observations"], {"session_count": 1})
        self.assertEqual(
            manifest["contracts"],
            {"event_schema": 1, "payload_schema": 1, "summary_schema": 3},
        )
        self.assertEqual(set(manifest["tools"]), {"key_insights", "neovim", "python"})
        for value in manifest["tools"].values():
            self.assertIsInstance(value, str)
            self.assertGreater(len(value), 0)
            self.assertLessEqual(len(value), 256)

        manifest_text = manifest_bytes.decode()
        for private_value in (str(self.root), *PRIVATE_CANARIES):
            self.assertNotIn(private_value, manifest_text)
        self.assertTrue((self.reports / "summary.json").is_file())
        self.assertTrue((self.reports / "report.md").is_file())
        self.assertTrue((self.reports / "payload.json").is_file())

    def test_refusal_at_any_boundary_does_not_publish_manifest(self) -> None:
        completed = self.run_tool("yes\nyes\nno\nyes\n")
        self.assertNotEqual(completed.returncode, 0)
        self.assertFalse(self.manifest.exists())

    def test_session_report_and_manifest_must_be_outside_source_tree(self) -> None:
        for option, path in (
            ("session_dir", ROOT / "tests"),
            ("report_dir", ROOT / "tests"),
            ("manifest", ROOT / "inspection-manifest.json"),
        ):
            completed = self.run_tool(
                "yes\nyes\nyes\nyes\n",
                **{option: path},
            )
            self.assertNotEqual(completed.returncode, 0, option)
        self.assertFalse((ROOT / "inspection-manifest.json").exists())

    def test_harness_has_no_network_codex_or_shell_execution_surface(self) -> None:
        source = FORWARD_TOOL.read_text()
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
