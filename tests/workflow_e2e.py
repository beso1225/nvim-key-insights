#!/usr/bin/env python3
from __future__ import annotations

import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile
import time
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
    'PUBLIC_UNICODE_雪_\\"_SECRET',
)
CODEX_CANARIES = (
    "CODEX_BUFFER_PATH_SECRET",
    "CODEX_INSERT_TEXT_SECRET",
    "CODEX_COMMAND_TEXT_SECRET",
    "CODEX_SEARCH_TEXT_SECRET",
    "CODEX_MAPPING_RHS_SECRET",
    "CODEX_REPORT_ONLY_SECRET",
    "CODEX_ADJACENT_FILE_SECRET",
    "CODEX_PARENT_ENV_SECRET",
    "CODEX_FILE_AUTH_SECRET",
    'CODEX_UNICODE_雪_\\"_SECRET',
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
        command = [
            str(ANALYZER),
            "analyze",
            "--session-dir",
            str(session_directory),
            "--summary",
            str(report_directory / "summary.json"),
            "--report",
            str(report_directory / "report.md"),
        ]
        result = subprocess.run(
            command,
            cwd=ROOT,
            env=self.environment(),
            capture_output=True,
            text=True,
            timeout=15,
        )
        self.assertEqual(result.returncode, 0, f"analyzer failed: {result.stderr}")

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
            for representation in (canary, json.dumps(canary, ensure_ascii=True)[1:-1]):
                self.assertNotIn(representation, contents, f"{boundary} leaked {canary}")

    def assert_json_canaries_absent(
        self, boundary: str, document: object, canaries: tuple[str, ...]
    ) -> None:
        def visit(value: object) -> None:
            if isinstance(value, dict):
                for key, nested in value.items():
                    visit(key)
                    visit(nested)
            elif isinstance(value, list):
                for nested in value:
                    visit(nested)
            elif isinstance(value, str):
                for canary in canaries:
                    self.assertNotIn(canary, value, f"{boundary} semantically leaked {canary}")

        visit(document)

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
            self.assert_json_canaries_absent("public collector JSONL", self.events(log), PUBLIC_CANARIES)
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
        self.assertEqual(first_keys, ["j", "j", "i", "i", ":", "/", "l", "l", "z", "9"])
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
        self.assert_json_canaries_absent("public summary", public_summary, PUBLIC_CANARIES)
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

    def test_crashed_collector_is_ignored_then_publicly_purged(self) -> None:
        sessions = self.root / "crash-sessions"
        baseline_reports = self.root / "crash-baseline-reports"
        self.run_nvim(
            "tests/e2e/vimleavepre.lua",
            self.environment(
                KEY_INSIGHTS_SESSION_DIR=str(sessions),
                KEY_INSIGHTS_REPORT_DIR=str(baseline_reports),
                KEY_INSIGHTS_EXIT_STATE="recording",
            ),
        )
        ready = self.root / "crash-ready"
        process = subprocess.Popen(
            [NVIM, "--headless", "-u", "tests/lua/minimal_init.lua", "-l", "tests/e2e/crash_collector.lua"],
            cwd=ROOT,
            env=self.environment(
                KEY_INSIGHTS_SESSION_DIR=str(sessions),
                KEY_INSIGHTS_REPORT_DIR=str(baseline_reports),
                KEY_INSIGHTS_READY_PATH=str(ready),
            ),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        deadline = time.monotonic() + 5
        while not ready.exists() and process.poll() is None and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertTrue(ready.exists(), "crash child did not publish its ready marker")
        process.kill()
        process.communicate(timeout=5)

        partials = list(sessions.glob("*.jsonl.part"))
        locks = list(sessions.glob("*.lock"))
        self.assertEqual(len(partials), 1)
        self.assertEqual(len(locks), 1)
        crash_lock = json.loads(locks[0].read_text())
        self.assertEqual(crash_lock["pid"], process.pid)
        ignored_reports = self.root / "crash-ignored-reports"
        self.analyze(sessions, ignored_reports)
        self.assertEqual(json.loads((ignored_reports / "summary.json").read_text())["sessions"], 1)

        malformed_part = sessions / "nvim-key-insights-malformed.jsonl.part"
        malformed_lock = sessions / "nvim-key-insights-malformed.lock"
        unrelated = sessions / "unrelated.txt"
        symlink = sessions / "nvim-key-insights-symlink.jsonl"
        for path, contents in (
            (malformed_part, "malformed-part"),
            (malformed_lock, "not-json"),
            (unrelated, "unrelated"),
        ):
            path.write_text(contents)
            path.chmod(0o600)
        symlink.symlink_to(unrelated)
        symlink_before = (symlink.lstat().st_ino, os.readlink(symlink))
        locks[0].write_text('{"pid":2147483647,"version":1}\n')
        locks[0].chmod(0o600)
        preserved_before = {
            path.name: (path.read_bytes(), stat.S_IMODE(path.lstat().st_mode), path.lstat().st_ino)
            for path in (malformed_part, malformed_lock, unrelated)
        }
        report_before = {
            path.name: path.read_bytes()
            for path in (ignored_reports / "summary.json", ignored_reports / "report.md")
        }
        before = sorted(path.name for path in sessions.iterdir())
        trace = self.root / "purge-trace.json"
        self.run_nvim(
            "tests/e2e/public_purge.lua",
            self.environment(
                KEY_INSIGHTS_SESSION_DIR=str(sessions),
                KEY_INSIGHTS_TRACE_PATH=str(trace),
            ),
        )
        purge_trace = json.loads(trace.read_text())
        active_part = f"nvim-key-insights-{purge_trace['active_session']}.jsonl.part"
        active_lock = f"nvim-key-insights-{purge_trace['active_session']}.lock"
        expected_before = sorted(before + [active_part, active_lock])
        self.assertEqual(purge_trace["before"], expected_before)
        self.assertEqual(purge_trace["after_cancel"], expected_before)
        after_force = purge_trace["after_force"]
        self.assertIn(malformed_part.name, after_force)
        self.assertIn(malformed_lock.name, after_force)
        self.assertIn(unrelated.name, after_force)
        self.assertIn(symlink.name, after_force)
        self.assertIn(active_part, after_force)
        self.assertIn(active_lock, after_force)
        self.assertFalse(
            any(
                name.endswith(".jsonl.part") and name not in (malformed_part.name, active_part)
                for name in after_force
            ),
            (after_force, purge_trace["notifications"]),
        )
        self.assertFalse(any(
            name.endswith(".jsonl") and name not in (symlink.name,)
            for name in after_force
        ))
        self.assertTrue((ignored_reports / "summary.json").is_file())
        self.assertTrue((ignored_reports / "report.md").is_file())
        active_final = f"nvim-key-insights-{purge_trace['active_session']}.jsonl"
        self.assertIn(active_final, purge_trace["after_stop"])
        self.assertNotIn(active_part, purge_trace["after_stop"])
        self.assertNotIn(active_lock, purge_trace["after_stop"])
        for path in (malformed_part, malformed_lock, unrelated):
            before_bytes, before_mode, before_inode = preserved_before[path.name]
            self.assertEqual(path.read_bytes(), before_bytes)
            self.assertEqual(stat.S_IMODE(path.lstat().st_mode), before_mode)
            self.assertEqual(path.lstat().st_ino, before_inode)
        for path in (ignored_reports / "summary.json", ignored_reports / "report.md"):
            self.assertEqual(path.read_bytes(), report_before[path.name])
        self.assertEqual((symlink.lstat().st_ino, os.readlink(symlink)), symlink_before)
        self.assertEqual(purge_trace["notifications"], [
            "key-insights: purge removed 3; protected 4; skipped 2; failed 0"
        ])

    def test_public_finalization_applies_age_then_count_retention(self) -> None:
        sessions = self.root / "retention-sessions"
        reports = self.root / "retention-reports"
        trace = self.root / "retention-trace.json"
        self.run_nvim(
            "tests/e2e/retention_finalize.lua",
            self.environment(
                KEY_INSIGHTS_SESSION_DIR=str(sessions),
                KEY_INSIGHTS_REPORT_DIR=str(reports),
                KEY_INSIGHTS_TRACE_PATH=str(trace),
            ),
        )
        retention = json.loads(trace.read_text())
        names = retention["names"]
        self.assertIn("nvim-key-insights-live.jsonl", names)
        self.assertIn("nvim-key-insights-live.lock", names)
        self.assertIn(f"nvim-key-insights-{retention['current_session']}.jsonl", names)
        self.assertIn("nvim-key-insights-incomplete.jsonl.part", names)
        self.assertIn("unrelated.txt", names)
        self.assertEqual(retention["preserved_before"], retention["preserved_after"])
        self.assertEqual((sessions / "nvim-key-insights-incomplete.jsonl.part").read_text(), "incomplete\n")
        self.assert_mode(sessions / "nvim-key-insights-incomplete.jsonl.part", 0o600)
        self.assertEqual((sessions / "unrelated.txt").read_text(), "unrelated\n")
        self.assert_mode(sessions / "unrelated.txt", 0o600)
        self.assertNotIn("nvim-key-insights-expired.jsonl", names)
        self.assertNotIn("nvim-key-insights-old-a.jsonl", names)
        self.assertNotIn("nvim-key-insights-old-b.jsonl", names)
        self.analyze(sessions, reports)
        self.assertEqual(json.loads((reports / "summary.json").read_text())["sessions"], 2)

    def test_public_report_failure_preserves_the_published_pair(self) -> None:
        root = self.root / "public-report-failure"
        sessions = root / "sessions"
        reports = root / "reports"
        trace = root / "trace.json"
        failing_analyzer = root / "failing-analyzer"
        sessions.mkdir(parents=True)
        reports.mkdir()
        failing_analyzer.write_text(
            "#!/bin/sh\nprintf '%s\\n' 'ANALYZER_STDERR_PRIVATE_SECRET' >&2\nexit 23\n"
        )
        failing_analyzer.chmod(0o700)

        self.run_nvim(
            "tests/e2e/public_report_failure.lua",
            self.environment(
                KEY_INSIGHTS_SESSION_DIR=str(sessions),
                KEY_INSIGHTS_REPORT_DIR=str(reports),
                KEY_INSIGHTS_TRACE_PATH=str(trace),
                KEY_INSIGHTS_FAILING_ANALYZER=str(failing_analyzer),
            ),
        )

        document = json.loads(trace.read_text())
        self.assertTrue(document["summary_preserved"])
        self.assertTrue(document["report_preserved"])
        notifications = "\n".join(document["notifications"])
        self.assertIn("report failed (the analyzer exited unsuccessfully)", notifications)
        self.assertNotIn("ANALYZER_STDERR_PRIVATE_SECRET", notifications)

    def test_public_mock_codex_uses_only_the_confirmed_sanitized_boundary(self) -> None:
        sessions = self.root / "codex-sessions"
        reports = self.root / "codex-reports"
        mock_directory = self.root / "mock-codex"
        codex_home = self.root / "codex-home"
        private_directory(mock_directory)
        private_directory(codex_home)
        mock_codex = mock_directory / "codex"
        subprocess.run(
            ["cc", str(ROOT / "tests/e2e/mock_codex.c"), "-o", str(mock_codex)],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        )
        mock_codex.chmod(0o700)
        (codex_home / "auth.json").write_text('{"private":"CODEX_FILE_AUTH_SECRET"}')
        (codex_home / "auth.json").chmod(0o600)
        trace = self.root / "codex-trace.json"
        preview = self.root / "codex-preview.json"
        suggestions = self.root / "codex-suggestions.md"
        environment = self.environment(
            KEY_INSIGHTS_SESSION_DIR=str(sessions),
            KEY_INSIGHTS_REPORT_DIR=str(reports),
            KEY_INSIGHTS_TRACE_PATH=str(trace),
            KEY_INSIGHTS_PREVIEW_PATH=str(preview),
            KEY_INSIGHTS_SUGGESTIONS_PATH=str(suggestions),
            KEY_INSIGHTS_MOCK_CODEX=str(mock_codex),
            CODEX_HOME=str(codex_home),
            OPENAI_API_KEY="CODEX_PARENT_ENV_SECRET",
            KEY_INSIGHTS_UNRELATED_PARENT="CODEX_UNRELATED_ENV_SECRET",
        )
        self.run_nvim("tests/e2e/public_codex.lua", environment)

        trace_document = json.loads(trace.read_text())
        self.assertEqual(trace_document["confirmation_count"], 2)
        self.assertIn(str(mock_codex), trace_document["confirmation_prompt"])
        self.assertEqual((mock_directory / "codex-invocations.txt").read_text(), "1")
        preview_bytes = preview.read_bytes()
        stdin_bytes = (mock_directory / "codex-stdin.json").read_bytes()
        self.assertEqual(preview_bytes, stdin_bytes)
        payload = json.loads(stdin_bytes)
        self.assertEqual(payload["payload_schema_version"], 1)
        self.assertEqual(payload["summary"]["sessions"], 1)

        codex_environment = dict(
            line.split("=", 1)
            for line in (mock_directory / "codex-env.txt").read_text().splitlines()
        )
        self.assertEqual(set(codex_environment), {"CODEX_HOME", "PATH"})
        self.assertEqual(codex_environment["CODEX_HOME"], str(codex_home))
        self.assertEqual(codex_environment["PATH"], environment["PATH"])
        for removed in ("HOME", "OPENAI_API_KEY", "HTTPS_PROXY", "SSL_CERT_FILE"):
            self.assertNotIn(removed, codex_environment)
        serialized_codex_environment = json.dumps(codex_environment, sort_keys=True)
        for canary in CODEX_CANARIES + ("CODEX_UNRELATED_ENV_SECRET",):
            self.assertNotIn(canary, serialized_codex_environment)
        codex_argv = (mock_directory / "codex-argv.txt").read_text().splitlines()
        self.assertEqual(codex_argv, [
            "exec",
            "--ephemeral",
            "--ignore-user-config",
            "--ignore-rules",
            "--strict-config",
            "--skip-git-repo-check",
            "--cd",
            str(self.cache / "nvim/key-insights/codex-empty"),
            "--config",
            'shell_environment_policy.inherit="none"',
            "--config",
            'approval_policy="never"',
            "--config",
            'default_permissions="key-insights-payload-only"',
            "--config",
            'permissions.key-insights-payload-only.filesystem={":root"="deny",":minimal"="read"}',
            "--config",
            "permissions.key-insights-payload-only.network.enabled=false",
            "--output-schema",
            str(ROOT / "lua/key-insights/../../codex/suggestions.schema.json"),
        ])

        summary_contents = (reports / "summary.json").read_text()
        report_contents = (reports / "report.md").read_text()
        output_contents = (mock_directory / "codex-output.json").read_text()
        suggestion_contents = suggestions.read_text()
        self.assertTrue(suggestion_contents.startswith("# Codex suggestions"))
        self.assertIn("Keep the measured workflow", suggestion_contents)
        self.assertIn("The sanitized aggregate does not justify a mapping change.", suggestion_contents)
        self.assertIn("- `sessions`: 1", suggestion_contents)
        combined_jsonl = "\n".join(path.read_text() for path in sessions.glob("*.jsonl"))
        self.assertIn(trace_document["session_id"], combined_jsonl)
        for canary in CODEX_CANARIES:
            for representation in (canary, json.dumps(canary, ensure_ascii=True)[1:-1]):
                self.assertNotIn(representation, combined_jsonl, f"collector JSONL leaked {canary}")
        for path in sessions.glob("*.jsonl"):
            self.assert_json_canaries_absent("collector JSONL", self.events(path), CODEX_CANARIES)
        boundaries = {
            "summary": summary_contents,
            "preview": preview_bytes.decode(),
            "codex stdin": stdin_bytes.decode(),
            "codex output": output_contents,
            "rendered suggestions": suggestion_contents,
            "notifications": "\n".join(trace_document["notifications"]),
        }
        for boundary, contents in boundaries.items():
            for canary in CODEX_CANARIES:
                for representation in (canary, json.dumps(canary, ensure_ascii=True)[1:-1]):
                    self.assertNotIn(representation, contents, f"{boundary} leaked {canary}")
            self.assertNotIn(trace_document["session_id"], contents, f"{boundary} leaked session ID")
        for boundary, document in {
            "summary": json.loads(summary_contents),
            "preview": payload,
            "Codex stdin": json.loads(stdin_bytes),
            "Codex output": json.loads(output_contents),
        }.items():
            self.assert_json_canaries_absent(boundary, document, CODEX_CANARIES)
        self.assertIn("CODEX_REPORT_ONLY_SECRET", report_contents)
        for canary in CODEX_CANARIES:
            if canary != "CODEX_REPORT_ONLY_SECRET":
                self.assertNotIn(canary, report_contents, f"local report leaked {canary}")
        self.assertNotIn(trace_document["session_id"], report_contents, "local report leaked session ID")


if __name__ == "__main__":
    unittest.main(verbosity=2)
