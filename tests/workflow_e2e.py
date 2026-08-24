#!/usr/bin/env python3
from __future__ import annotations

import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
NVIM = os.environ.get("NVIM", "nvim")
ANALYZER = Path(os.environ.get("KEY_INSIGHTS_BIN", ROOT / "target/debug/key-insights")).resolve()
PUBLIC_CANARIES = (
    "PUBLIC_BUFFER_PATH_SECRET",
    "PUBLIC_INSERT_TEXT_SECRET",
    "PUBLIC_COMMAND_TEXT_SECRET",
    "PUBLIC_SEARCH_TEXT_SECRET",
    "PUBLIC_MAPPING_RHS_SECRET",
)


def private_directory(path: Path) -> None:
    path.mkdir(parents=True, exist_ok=True)
    path.chmod(0o700)


class PublicWorkflowE2E(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="key-insights-workflow-")
        self.root = Path(self.temporary.name)
        self.home = self.root / "home"
        self.state = self.root / "state"
        self.cache = self.root / "cache"
        self.config = self.root / "config"
        for path in (self.home, self.state, self.cache, self.config):
            private_directory(path)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def environment(self, **values: str) -> dict[str, str]:
        environment = {
            "HOME": str(self.home),
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "TMPDIR": str(self.root),
            "XDG_CACHE_HOME": str(self.cache),
            "XDG_CONFIG_HOME": str(self.config),
            "XDG_STATE_HOME": str(self.state),
            "KEY_INSIGHTS_BIN": str(ANALYZER),
        }
        environment.update(values)
        return environment

    def run_nvim(self, scenario: str, environment: dict[str, str]) -> None:
        try:
            subprocess.run(
                [NVIM, "--headless", "-u", "tests/lua/minimal_init.lua", "-l", scenario],
                cwd=ROOT,
                env=environment,
                check=True,
                capture_output=True,
                text=True,
                timeout=15,
            )
        except subprocess.CalledProcessError as error:
            self.fail(
                f"child Neovim failed for {scenario}\nstdout:\n{error.stdout}\nstderr:\n{error.stderr}"
            )

    def analyze(self, session_directory: Path, report_directory: Path) -> None:
        private_directory(report_directory)
        subprocess.run(
            [
                str(ANALYZER),
                "analyze",
                "--session-dir",
                str(session_directory),
                "--summary",
                str(report_directory / "summary.json"),
                "--report",
                str(report_directory / "report.md"),
            ],
            cwd=ROOT,
            env=self.environment(),
            check=True,
            capture_output=True,
            text=True,
            timeout=15,
        )

    def finalized_logs(self, directory: Path) -> list[Path]:
        return sorted(directory.glob("*.jsonl"))

    def events(self, path: Path) -> list[dict[str, object]]:
        return [json.loads(line) for line in path.read_text().splitlines()]

    def assert_private_finalized_session(self, directory: Path) -> list[dict[str, object]]:
        logs = self.finalized_logs(directory)
        self.assertEqual(len(logs), 1)
        self.assertEqual(stat.S_IMODE(logs[0].stat().st_mode), 0o600)
        self.assertEqual(list(directory.glob("*.part")), [])
        self.assertEqual(list(directory.glob("*.lock")), [])
        events = self.events(logs[0])
        self.assertEqual(sum(event["event_type"] == "session_start" for event in events), 1)
        self.assertEqual(sum(event["event_type"] == "session_end" for event in events), 1)
        self.assertEqual(events[0]["session_id"], events[-1]["session_id"])
        return events

    def assert_mode(self, path: Path, expected: int) -> None:
        self.assertEqual(stat.S_IMODE(path.stat().st_mode), expected, str(path))

    def assert_canaries_absent(self, boundary: str, contents: str) -> None:
        for canary in PUBLIC_CANARIES:
            self.assertNotIn(canary, contents, f"{boundary} leaked {canary}")

    def test_public_commands_preserve_pause_and_restart_boundaries(self) -> None:
        sessions = self.root / "command-sessions"
        reports = self.root / "command-reports"
        trace = self.root / "command-trace.json"
        self.run_nvim(
            "tests/e2e/public_lifecycle.lua",
            self.environment(
                KEY_INSIGHTS_SESSION_DIR=str(sessions),
                KEY_INSIGHTS_REPORT_DIR=str(reports),
                KEY_INSIGHTS_TRACE_PATH=str(trace),
            ),
        )

        logs = self.finalized_logs(sessions)
        self.assertEqual(len(logs), 2)
        self.assert_mode(sessions, 0o700)
        self.assertEqual(list(sessions.glob("*.part")), [])
        self.assertEqual(list(sessions.glob("*.lock")), [])
        combined_jsonl = "\n".join(log.read_text() for log in logs)
        self.assert_canaries_absent("public collector JSONL", combined_jsonl)
        for log in logs:
            self.assert_mode(log, 0o600)
        trace_document = json.loads(trace.read_text())
        self.assertEqual(trace_document["state"], "stopped")
        self.assertNotEqual(trace_document["first_session"], trace_document["second_session"])
        self.assertTrue(any("recording" in message for message in trace_document["notifications"]))
        self.assertEqual(Path(trace_document["report_path"]).resolve(), (reports / "report.md").resolve())
        self.assertTrue((reports / "summary.json").is_file())
        self.assert_mode(reports, 0o700)
        self.assert_mode(reports / "summary.json", 0o600)
        self.assert_mode(reports / "report.md", 0o600)

        by_session = {}
        for log in logs:
            events = self.events(log)
            by_session[events[0]["session_id"]] = events
        first = by_session[trace_document["first_session"]]
        first_keys = [
            key
            for event in first
            if event["event_type"] == "key_sequence"
            for key in event["keys"]
        ]
        self.assertEqual(first_keys, ["j", "j", "i", ":", "/", "l", "l"])
        self.assertNotIn("k", first_keys)
        self.assertEqual(sum(event["event_type"] == "session_start" for event in first), 1)
        self.assertEqual(sum(event["event_type"] == "session_end" for event in first), 1)
        second = by_session[trace_document["second_session"]]
        second_keys = [
            key
            for event in second
            if event["event_type"] == "key_sequence"
            for key in event["keys"]
        ]
        self.assertEqual(second_keys, ["h"])
        self.assertEqual(sum(event["event_type"] == "session_start" for event in second), 1)
        self.assertEqual(sum(event["event_type"] == "session_end" for event in second), 1)

        summary_contents = (reports / "summary.json").read_text()
        report_contents = (reports / "report.md").read_text()
        public_summary = json.loads(summary_contents)
        self.assertEqual(public_summary["schema_version"], 3)
        self.assertEqual(public_summary["sessions"], 2)
        self.assertGreaterEqual(public_summary["text_runs"], 1)
        self.assertTrue(report_contents.startswith("# Neovim Key Insights"))
        self.assert_canaries_absent("public summary", summary_contents)
        self.assert_canaries_absent("public report", report_contents)
        notification_text = "\n".join(trace_document["notifications"])
        self.assert_canaries_absent("public notifications", notification_text)
        for session_id in (trace_document["first_session"], trace_document["second_session"]):
            self.assertNotIn(session_id, summary_contents)
            self.assertNotIn(session_id, report_contents)

        independent_reports = self.root / "independent-command-reports"
        self.analyze(sessions, independent_reports)
        self.assertEqual(json.loads((independent_reports / "summary.json").read_text())["sessions"], 2)

    def test_vimleavepre_finalizes_recording_and_paused_sessions(self) -> None:
        for exit_state in ("recording", "paused"):
            with self.subTest(exit_state=exit_state):
                sessions = self.root / f"{exit_state}-sessions"
                reports = self.root / f"{exit_state}-reports"
                self.run_nvim(
                    "tests/e2e/vimleavepre.lua",
                    self.environment(
                        KEY_INSIGHTS_SESSION_DIR=str(sessions),
                        KEY_INSIGHTS_REPORT_DIR=str(reports),
                        KEY_INSIGHTS_EXIT_STATE=exit_state,
                    ),
                )
                events = self.assert_private_finalized_session(sessions)
                self.assertTrue(any(event["event_type"] == "key_sequence" for event in events))
                self.analyze(sessions, reports)
                summary = json.loads((reports / "summary.json").read_text())
                self.assertEqual(summary["sessions"], 1)


if __name__ == "__main__":
    unittest.main(verbosity=2)
