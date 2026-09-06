#!/usr/bin/env python3

import json
import os
import shutil
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
FORWARD_TOOL = ROOT / "scripts" / "forward_test.py"
KEY_INSIGHTS_BIN = Path(
    os.environ.get("KEY_INSIGHTS_BIN", ROOT / "target" / "debug" / "key-insights")
).resolve()
SESSION_IDS = ("forward-session-alpha", "forward-session-beta")
PRIVATE_CANARIES = (
    *SESSION_IDS,
    "FORWARD_PROJECT_PRIVATE_CANARY",
    "FORWARD_ADJACENT_PRIVATE_CANARY",
)


def run_forward(workspace: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            "python3",
            str(FORWARD_TOOL),
            "--workspace",
            str(workspace),
            "--key-insights-bin",
            str(KEY_INSIGHTS_BIN),
        ],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
    )


class ForwardTestContract(unittest.TestCase):
    def private_workspace(self) -> Path:
        path = Path(tempfile.mkdtemp(prefix="nvim-key-insights-forward-"))
        path.chmod(0o700)
        self.addCleanup(shutil.rmtree, path, True)
        return path

    def test_synthetic_workflow_emits_only_a_bounded_sanitized_manifest(self) -> None:
        self.assertTrue(KEY_INSIGHTS_BIN.is_file(), "build the key-insights binary first")
        workspace = self.private_workspace()

        completed = run_forward(workspace)
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(completed.stderr, "")

        manifest_path = workspace / "inspection-manifest.json"
        manifest_bytes = manifest_path.read_bytes()
        self.assertLessEqual(len(manifest_bytes), 16 * 1024)
        self.assertEqual(completed.stdout, manifest_bytes.decode() + "\n")
        manifest = json.loads(manifest_bytes)
        self.assertEqual(
            manifest,
            {
                "manifest_version": 1,
                "mode": "synthetic-offline",
                "contracts": {"event_schema": 2, "payload_schema": 2, "summary_schema": 4},
                "artifacts": manifest["artifacts"],
                "checks": {
                    "codex_invoked": False,
                    "network_used": False,
                    "private_inputs_used": False,
                    "sanitized_boundaries": True,
                },
            },
        )
        self.assertEqual(set(manifest["artifacts"]), {"payload", "report", "summary"})
        for metadata in manifest["artifacts"].values():
            self.assertEqual(set(metadata), {"bytes", "sha256"})
            self.assertIsInstance(metadata["bytes"], int)
            self.assertGreater(metadata["bytes"], 0)
            self.assertLessEqual(metadata["bytes"], 16 * 1024 * 1024)
            self.assertRegex(metadata["sha256"], r"^[0-9a-f]{64}$")

        manifest_text = manifest_bytes.decode()
        self.assertNotIn(str(workspace), manifest_text)
        for canary in PRIVATE_CANARIES:
            self.assertNotIn(canary, manifest_text)

        session_log = workspace / "synthetic-sessions.jsonl"
        local_artifacts = [
            workspace / "summary.json",
            workspace / "report.md",
            workspace / "payload.json",
        ]
        adjacent = workspace / "adjacent-private-canary.txt"
        self.assertEqual(adjacent.read_text(), "FORWARD_ADJACENT_PRIVATE_CANARY")
        session_log_text = session_log.read_text()
        self.assertTrue(all(session_id in session_log_text for session_id in SESSION_IDS))
        session_events = [json.loads(line) for line in session_log_text.splitlines()]
        self.assertTrue(session_events)
        self.assertTrue(all(event["schema_version"] == 2 for event in session_events))
        self.assertIn(
            {
                "schema_version": 2,
                "event_type": "control_key_use",
                "session_id": "forward-session-alpha",
                "elapsed_ms": 80,
                "mode": "insert",
                "key": "<C-Y>",
                "count": 2,
            },
            session_events,
        )
        for artifact in [session_log, adjacent, *local_artifacts, manifest_path]:
            self.assertTrue(artifact.is_file())
            self.assertEqual(stat.S_IMODE(artifact.stat().st_mode), 0o600)
        for artifact in local_artifacts:
            content = artifact.read_text()
            self.assertNotIn(str(workspace), content)
            for canary in PRIVATE_CANARIES:
                self.assertNotIn(canary, content)

    def test_workspace_must_be_private_empty_and_outside_the_source_tree(self) -> None:
        nonempty = self.private_workspace()
        marker = nonempty / "keep-me"
        marker.write_text("unchanged")
        completed = run_forward(nonempty)
        self.assertNotEqual(completed.returncode, 0)
        self.assertEqual(marker.read_text(), "unchanged")

        permissive = self.private_workspace()
        permissive.chmod(0o755)
        completed = run_forward(permissive)
        self.assertNotEqual(completed.returncode, 0)
        self.assertEqual(list(permissive.iterdir()), [])

        inside_root = Path(tempfile.mkdtemp(prefix=".forward-test-", dir=ROOT))
        self.addCleanup(shutil.rmtree, inside_root, True)
        inside_root.chmod(0o700)
        completed = run_forward(inside_root)
        self.assertNotEqual(completed.returncode, 0)
        self.assertEqual(list(inside_root.iterdir()), [])

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
