#!/usr/bin/env python3

import json
import importlib.util
import shutil
import subprocess
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]
RELEASE_TOOL = ROOT / "scripts" / "release.py"
SCHEMA_COMPATIBILITY = ROOT / "docs" / "schema-compatibility.md"


def load_release_module():
    specification = importlib.util.spec_from_file_location("key_insights_release", RELEASE_TOOL)
    if specification is None or specification.loader is None:
        raise RuntimeError("cannot load release tool")
    module = importlib.util.module_from_spec(specification)
    sys.modules[specification.name] = module
    specification.loader.exec_module(module)
    return module


def run_release(*arguments: str, root: Path = ROOT) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["python3", str(RELEASE_TOOL), "--root", str(root), *arguments],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
    )


def copy_version_contract(destination: Path) -> None:
    for relative in (
        "Cargo.lock",
        "crates/key-insights-cli/Cargo.toml",
        "flake.nix",
        "plugins/nvim-key-insights/.codex-plugin/plugin.json",
        ".agents/plugins/marketplace.json",
    ):
        source = ROOT / relative
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)


def copy_schema_contract(destination: Path) -> None:
    for relative in (
        "codex/payload.schema.json",
        "codex/suggestions.schema.json",
        "crates/key-insights-cli/src/analyzer.rs",
        "crates/key-insights-cli/src/codex_payload.rs",
        "crates/key-insights-cli/src/codex_suggestions.rs",
        "crates/key-insights-cli/src/ergonomics.rs",
        "crates/key-insights-cli/src/keymap_snapshot.rs",
        "crates/key-insights-cli/src/lib.rs",
        "docs/schema-compatibility.md",
        "lua/key-insights/contract_versions.lua",
        "lua/key-insights/keymap_snapshot.lua",
        "lua/key-insights/report.lua",
        "lua/key-insights/schema.lua",
        "plugins/nvim-key-insights/skills/analyze-neovim-usage/references/payload.schema.json",
        "plugins/nvim-key-insights/skills/analyze-neovim-usage/references/suggestions.schema.json",
    ):
        source = ROOT / relative
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)


class ReleaseContractTest(unittest.TestCase):
    def test_current_repository_has_one_release_version(self) -> None:
        system = subprocess.run(
            ["nix", "eval", "--raw", "--impure", "--expr", "builtins.currentSystem"],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        ).stdout
        result = run_release("check", "--nix-system", system)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "release contract 0.1.0: ok")

        cargo = tomllib.loads(
            (ROOT / "crates/key-insights-cli/Cargo.toml").read_text()
        )
        plugin = json.loads(
            (ROOT / "plugins/nvim-key-insights/.codex-plugin/plugin.json").read_text()
        )
        self.assertEqual(cargo["package"]["version"], plugin["version"])

        flake = (ROOT / "flake.nix").read_text()
        self.assertIn(
            "builtins.fromTOML (builtins.readFile ./crates/key-insights-cli/Cargo.toml)",
            flake,
        )
        self.assertNotIn('version = "0.1.0";', flake)

    def test_schema_versions_match_runtime_and_bundled_contracts(self) -> None:
        self.assertTrue(SCHEMA_COMPATIBILITY.is_file())
        result = run_release("check")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_schema_contract_rejects_runtime_documentation_and_copy_drift(self) -> None:
        mutations = {
            "crates/key-insights-cli/src/lib.rs": (
                "pub const SCHEMA_VERSION: u32 = 1;",
                "pub const SCHEMA_VERSION: u32 = 2;",
            ),
            "lua/key-insights/contract_versions.lua": (
                "event_log = 1",
                "event_log = 2",
            ),
            "lua/key-insights/report.lua": (
                "summary.schema_version ~= contract_versions.analysis_summary",
                "summary.schema_version ~= 4",
            ),
            "crates/key-insights-cli/src/keymap_snapshot.rs": (
                'append_length_prefixed(&mut preimage, "mapping-v1");',
                'append_length_prefixed(&mut preimage, "mapping-v2");',
            ),
            "docs/schema-compatibility.md": (
                "| Event log | `1` |",
                "| Event log | `2` |",
            ),
            "codex/suggestions.schema.json": (
                '"schema_version": { "const": 1 }',
                '"schema_version": { "const": 2 }',
            ),
            "plugins/nvim-key-insights/skills/analyze-neovim-usage/references/payload.schema.json": (
                '"payload_schema_version": { "const": 1 }',
                '"payload_schema_version": { "const": 2 }',
            ),
        }
        for relative, (before, after) in mutations.items():
            with self.subTest(relative=relative):
                with tempfile.TemporaryDirectory() as temporary_directory:
                    root = Path(temporary_directory)
                    copy_version_contract(root)
                    copy_schema_contract(root)
                    path = root / relative
                    changed = path.read_text().replace(before, after, 1)
                    self.assertNotEqual(changed, path.read_text())
                    path.write_text(changed)
                    result = run_release("check", root=root)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("schema", result.stderr.lower())

        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            copy_version_contract(root)
            copy_schema_contract(root)
            versions_path = root / "lua/key-insights/contract_versions.lua"
            changed = versions_path.read_text().replace("[2] = true", "[4] = true", 1)
            self.assertNotEqual(changed, versions_path.read_text())
            versions_path.write_text(changed)
            result = run_release("check", root=root)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("report summary versions", result.stderr)

    def test_schema_contract_rejects_coordinated_nested_snapshot_drift(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            copy_version_contract(root)
            copy_schema_contract(root)
            canonical_path = root / "codex/payload.schema.json"
            bundled_path = root / (
                "plugins/nvim-key-insights/skills/analyze-neovim-usage/"
                "references/payload.schema.json"
            )
            document = json.loads(canonical_path.read_text())
            document["$defs"]["mapping_attribution"]["properties"][
                "snapshot_version"
            ]["const"] = 2
            changed = json.dumps(document, indent=2).encode() + b"\n"
            canonical_path.write_bytes(changed)
            bundled_path.write_bytes(changed)

            result = run_release("check", root=root)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("mapping attribution snapshot", result.stderr)

    def test_schema_contract_rejects_one_drifted_lua_snapshot_gate(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            copy_version_contract(root)
            copy_schema_contract(root)
            report_path = root / "lua/key-insights/report.lua"
            changed = report_path.read_text().replace(
                "decoded.keymap_snapshot.snapshot_version ~= contract_versions.keymap_snapshot",
                "decoded.keymap_snapshot.snapshot_version ~= 2",
                1,
            )
            self.assertNotEqual(changed, report_path.read_text())
            report_path.write_text(changed)

            result = run_release("check", root=root)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("schema Lua validator", result.stderr)

    def test_schema_contract_rejects_coordinated_mapping_identity_drift(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            copy_version_contract(root)
            copy_schema_contract(root)
            for relative in (
                "codex/payload.schema.json",
                "codex/suggestions.schema.json",
                "plugins/nvim-key-insights/skills/analyze-neovim-usage/references/payload.schema.json",
                "plugins/nvim-key-insights/skills/analyze-neovim-usage/references/suggestions.schema.json",
            ):
                path = root / relative
                path.write_text(path.read_text().replace("mapping-v1", "mapping-v2"))

            result = run_release("check", root=root)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("mapping identity", result.stderr)

    def test_public_schema_documents_link_the_upgrade_policy(self) -> None:
        for relative in (
            "docs/analyzer.md",
            "docs/event-schema.md",
            "docs/installation.md",
        ):
            with self.subTest(relative=relative):
                self.assertIn(
                    "schema-compatibility.md",
                    (ROOT / relative).read_text(),
                )

    def test_release_tag_must_exactly_match_the_package_version(self) -> None:
        accepted = run_release("check", "--tag", "v0.1.0")
        self.assertEqual(accepted.returncode, 0, accepted.stderr)

        for tag in ("0.1.0", "v0.1.1", "v0.1.0-rc.1", "refs/tags/v0.1.0"):
            with self.subTest(tag=tag):
                rejected = run_release("check", "--tag", tag)
                self.assertNotEqual(rejected.returncode, 0)
                self.assertIn("release tag", rejected.stderr)

    def test_version_bump_updates_only_the_explicit_mirrors(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            copy_version_contract(root)
            copy_schema_contract(root)

            result = run_release(
                "bump", "--from", "0.1.0", "--to", "0.2.0", root=root
            )
            self.assertEqual(result.returncode, 0, result.stderr)

            cargo = tomllib.loads(
                (root / "crates/key-insights-cli/Cargo.toml").read_text()
            )
            lock = tomllib.loads((root / "Cargo.lock").read_text())
            plugin = json.loads(
                (root / "plugins/nvim-key-insights/.codex-plugin/plugin.json").read_text()
            )
            package_versions = {
                package["version"]
                for package in lock["package"]
                if package["name"] == "key-insights"
            }
            self.assertEqual(cargo["package"]["version"], "0.2.0")
            self.assertEqual(package_versions, {"0.2.0"})
            self.assertEqual(plugin["version"], "0.2.0")
            self.assertNotIn('version = "0.2.0";', (root / "flake.nix").read_text())

            check = run_release("check", root=root)
            self.assertEqual(check.returncode, 0, check.stderr)

    def test_invalid_bump_leaves_every_file_unchanged(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            copy_version_contract(root)
            tracked = sorted(path for path in root.rglob("*") if path.is_file())
            before = {path: path.read_bytes() for path in tracked}

            for arguments in (
                ("bump", "--from", "0.1.0", "--to", "invalid"),
                ("bump", "--from", "9.9.9", "--to", "1.0.0"),
                ("bump", "--from", "0.1.0", "--to", "0.1.0"),
            ):
                with self.subTest(arguments=arguments):
                    result = run_release(*arguments, root=root)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(
                        {path: path.read_bytes() for path in tracked}, before
                    )

    def test_marketplace_does_not_duplicate_the_plugin_version(self) -> None:
        marketplace = json.loads(
            (ROOT / ".agents/plugins/marketplace.json").read_text()
        )

        def contains_version(value: object) -> bool:
            if isinstance(value, dict):
                return "version" in value or any(
                    contains_version(child) for child in value.values()
                )
            if isinstance(value, list):
                return any(contains_version(child) for child in value)
            return False

        self.assertFalse(contains_version(marketplace))

    def test_write_failure_rolls_back_every_version_file(self) -> None:
        release = load_release_module()
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            copy_version_contract(root)
            tracked = [
                root / "crates/key-insights-cli/Cargo.toml",
                root / "Cargo.lock",
                root / "plugins/nvim-key-insights/.codex-plugin/plugin.json",
            ]
            before = {path: path.read_bytes() for path in tracked}
            real_replace = release.os.replace
            replacement_count = 0

            def fail_second_install(source, destination):
                nonlocal replacement_count
                if Path(destination) in tracked and ".release-new-" in Path(source).name:
                    replacement_count += 1
                    if replacement_count == 2:
                        raise OSError("injected replacement failure")
                return real_replace(source, destination)

            with mock.patch.object(release.os, "replace", side_effect=fail_second_install):
                with self.assertRaises(OSError):
                    release.bump(root, "0.1.0", "0.2.0")

            self.assertEqual({path: path.read_bytes() for path in tracked}, before)

    def test_catchable_interruption_rolls_back_every_version_file(self) -> None:
        release = load_release_module()
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            copy_version_contract(root)
            tracked = [
                root / "crates/key-insights-cli/Cargo.toml",
                root / "Cargo.lock",
                root / "plugins/nvim-key-insights/.codex-plugin/plugin.json",
            ]
            before = {path: path.read_bytes() for path in tracked}
            real_replace = release.os.replace
            replacement_count = 0

            def interrupt_second_install(source, destination):
                nonlocal replacement_count
                if Path(destination) in tracked and ".release-new-" in Path(source).name:
                    replacement_count += 1
                    if replacement_count == 2:
                        raise KeyboardInterrupt()
                return real_replace(source, destination)

            with mock.patch.object(
                release.os, "replace", side_effect=interrupt_second_install
            ):
                with self.assertRaises(KeyboardInterrupt):
                    release.bump(root, "0.1.0", "0.2.0")

            self.assertEqual({path: path.read_bytes() for path in tracked}, before)

    def test_concurrent_edit_is_not_overwritten(self) -> None:
        release = load_release_module()
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            copy_version_contract(root)
            cargo_path = root / "crates/key-insights-cli/Cargo.toml"
            original_validate = release.validate_candidates

            def edit_after_validation(candidate_root, updates, new_version):
                original_validate(candidate_root, updates, new_version)
                cargo_path.write_text(cargo_path.read_text() + "\n# concurrent edit\n")

            with mock.patch.object(
                release, "validate_candidates", side_effect=edit_after_validation
            ):
                with self.assertRaises(release.ContractError):
                    release.bump(root, "0.1.0", "0.2.0")

            self.assertTrue(cargo_path.read_text().endswith("# concurrent edit\n"))
            self.assertIn('version = "0.1.0"', cargo_path.read_text())
            self.assertEqual(list(root.rglob(".release-*-*")), [])

    def test_edit_between_installs_is_preserved_and_prior_install_is_rolled_back(self) -> None:
        release = load_release_module()
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            copy_version_contract(root)
            cargo_path = root / "crates/key-insights-cli/Cargo.toml"
            lock_path = root / "Cargo.lock"
            original_cargo = cargo_path.read_bytes()
            real_replace = release.os.replace

            def edit_lock_after_cargo_install(source, destination):
                result = real_replace(source, destination)
                if (
                    Path(destination) == cargo_path
                    and ".release-new-" in Path(source).name
                ):
                    lock_path.write_text(lock_path.read_text() + "\n# concurrent lock edit\n")
                return result

            with mock.patch.object(
                release.os, "replace", side_effect=edit_lock_after_cargo_install
            ):
                with self.assertRaises(release.ContractError):
                    release.bump(root, "0.1.0", "0.2.0")

            self.assertEqual(cargo_path.read_bytes(), original_cargo)
            self.assertTrue(lock_path.read_text().endswith("# concurrent lock edit\n"))
            self.assertIn('version = "0.1.0"', lock_path.read_text())
            self.assertEqual(list(root.rglob(".release-*-*")), [])

    def test_failed_rollback_preserves_the_recovery_backup(self) -> None:
        release = load_release_module()
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            copy_version_contract(root)
            cargo_path = root / "crates/key-insights-cli/Cargo.toml"
            lock_path = root / "Cargo.lock"
            original_cargo = cargo_path.read_bytes()
            real_replace = release.os.replace
            installed_cargo = False

            def fail_install_and_rollback(source, destination):
                nonlocal installed_cargo
                source_path = Path(source)
                destination_path = Path(destination)
                if destination_path == cargo_path and ".release-new-" in source_path.name:
                    installed_cargo = True
                elif destination_path == lock_path and ".release-new-" in source_path.name:
                    raise OSError("injected install failure")
                elif (
                    installed_cargo
                    and destination_path == cargo_path
                    and ".release-old-" in source_path.name
                ):
                    raise OSError("injected rollback failure")
                return real_replace(source, destination)

            with mock.patch.object(
                release.os, "replace", side_effect=fail_install_and_rollback
            ):
                with self.assertRaises(release.ContractError) as raised:
                    release.bump(root, "0.1.0", "0.2.0")

            backups = list(cargo_path.parent.glob(".release-old-Cargo.toml-*"))
            self.assertEqual(len(backups), 1)
            self.assertEqual(backups[0].read_bytes(), original_cargo)
            self.assertIn(str(backups[0]), str(raised.exception))

    def test_staging_failure_leaves_no_partial_files(self) -> None:
        release = load_release_module()
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            copy_version_contract(root)
            tracked = sorted(path for path in root.rglob("*") if path.is_file())
            before = {path: path.read_bytes() for path in tracked}
            real_stage = release.stage_file
            stage_count = 0

            def fail_second_stage(path, data, label):
                nonlocal stage_count
                stage_count += 1
                if stage_count == 2:
                    raise release.ContractError("injected staging failure")
                return real_stage(path, data, label)

            with mock.patch.object(release, "stage_file", side_effect=fail_second_stage):
                with self.assertRaises(release.ContractError):
                    release.bump(root, "0.1.0", "0.2.0")

            self.assertEqual({path: path.read_bytes() for path in tracked}, before)
            self.assertEqual(list(root.rglob(".release-*-*")), [])

    def test_duplicate_json_version_key_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            copy_version_contract(root)
            plugin_path = root / "plugins/nvim-key-insights/.codex-plugin/plugin.json"
            plugin_path.write_text(
                plugin_path.read_text().replace(
                    '"version": "0.1.0",',
                    '"version": "9.9.9",\n  "version": "0.1.0",',
                )
            )
            result = run_release("check", root=root)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("duplicate JSON key", result.stderr)

    def test_non_utf8_json_manifest_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            copy_version_contract(root)
            plugin_path = root / "plugins/nvim-key-insights/.codex-plugin/plugin.json"
            plugin_path.write_bytes(plugin_path.read_text().encode("utf-16"))
            result = run_release("check", root=root)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("UTF-8 JSON", result.stderr)


if __name__ == "__main__":
    unittest.main()
