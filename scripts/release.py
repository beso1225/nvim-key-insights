#!/usr/bin/env python3
"""Validate and update the repository release version without publishing."""

from __future__ import annotations

import argparse
import json
import os
import re
import stat
import subprocess
import sys
import tempfile
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import NoReturn


VERSION_PATTERN = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)")
CANONICAL_MANIFEST = Path("crates/key-insights-cli/Cargo.toml")
LOCK_FILE = Path("Cargo.lock")
FLAKE_FILE = Path("flake.nix")
PLUGIN_MANIFEST = Path("plugins/nvim-key-insights/.codex-plugin/plugin.json")
MARKETPLACE = Path(".agents/plugins/marketplace.json")
REQUIRED_FILES = (
    CANONICAL_MANIFEST,
    LOCK_FILE,
    FLAKE_FILE,
    PLUGIN_MANIFEST,
    MARKETPLACE,
)


class ContractError(Exception):
    pass


@dataclass(frozen=True)
class VersionUpdate:
    original: bytes
    replacement: bytes


def fail(message: str) -> NoReturn:
    raise ContractError(message)


def stable_version(value: object, field: str) -> str:
    if not isinstance(value, str) or VERSION_PATTERN.fullmatch(value) is None:
        fail(f"{field} must be a stable X.Y.Z version")
    return value


def read_regular_file(root: Path, relative: Path) -> bytes:
    path = root / relative
    try:
        metadata = path.lstat()
    except OSError as error:
        fail(f"cannot inspect {relative}: {error}")
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        fail(f"{relative} must be a regular file and not a symlink")
    try:
        return path.read_bytes()
    except OSError as error:
        fail(f"cannot read {relative}: {error}")


def parse_toml(data: bytes, field: str) -> dict[str, object]:
    try:
        document = tomllib.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        fail(f"{field} is not valid UTF-8 TOML: {error}")
    if not isinstance(document, dict):
        fail(f"{field} must be a TOML document")
    return document


def parse_json(data: bytes, field: str) -> object:
    def unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
        result: dict[str, object] = {}
        for key, value in pairs:
            if key in result:
                fail(f"{field} contains duplicate JSON key {key!r}")
            result[key] = value
        return result

    try:
        text = data.decode("utf-8")
        return json.loads(text, object_pairs_hook=unique_object)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        fail(f"{field} is not valid UTF-8 JSON: {error}")


def contains_version_key(value: object) -> bool:
    if isinstance(value, dict):
        return "version" in value or any(contains_version_key(child) for child in value.values())
    if isinstance(value, list):
        return any(contains_version_key(child) for child in value)
    return False


def validate_contract(root: Path, tag: str | None = None) -> str:
    files = {relative: read_regular_file(root, relative) for relative in REQUIRED_FILES}

    cargo = parse_toml(files[CANONICAL_MANIFEST], str(CANONICAL_MANIFEST))
    package = cargo.get("package")
    if not isinstance(package, dict) or package.get("name") != "key-insights":
        fail("canonical Cargo package must be named key-insights")
    version = stable_version(package.get("version"), "Cargo package version")

    lock = parse_toml(files[LOCK_FILE], str(LOCK_FILE))
    packages = lock.get("package")
    if not isinstance(packages, list):
        fail("Cargo.lock must contain package entries")
    own_packages = [
        entry
        for entry in packages
        if isinstance(entry, dict) and entry.get("name") == "key-insights"
    ]
    if len(own_packages) != 1:
        fail("Cargo.lock must contain exactly one key-insights package")
    lock_version = stable_version(own_packages[0].get("version"), "Cargo.lock version")
    if lock_version != version:
        fail(f"Cargo.lock version {lock_version} does not match Cargo version {version}")

    plugin = parse_json(files[PLUGIN_MANIFEST], str(PLUGIN_MANIFEST))
    if not isinstance(plugin, dict) or plugin.get("name") != "nvim-key-insights":
        fail("Codex plugin manifest name is invalid")
    plugin_version = stable_version(plugin.get("version"), "Codex plugin version")
    if plugin_version != version:
        fail(f"Codex plugin version {plugin_version} does not match Cargo version {version}")

    marketplace = parse_json(files[MARKETPLACE], str(MARKETPLACE))
    if contains_version_key(marketplace):
        fail("marketplace must not duplicate the plugin version")

    try:
        flake = files[FLAKE_FILE].decode("utf-8")
    except UnicodeDecodeError as error:
        fail(f"flake.nix is not valid UTF-8: {error}")
    source_expression = (
        "builtins.fromTOML (builtins.readFile "
        "./crates/key-insights-cli/Cargo.toml)"
    )
    if source_expression not in flake:
        fail("flake.nix must derive its version from the canonical Cargo manifest")
    if re.search(r'^\s*version\s*=\s*"[^"]+";\s*$', flake, re.MULTILINE):
        fail("flake.nix must not duplicate a literal package version")

    if tag is not None and tag != f"v{version}":
        fail(f"release tag {tag!r} must exactly match v{version}")
    return version


def replace_once(data: bytes, old: bytes, new: bytes, field: str) -> bytes:
    if data.count(old) != 1:
        fail(f"{field} did not contain exactly one expected version field")
    return data.replace(old, new, 1)


def candidate_updates(
    root: Path, old_version: str, new_version: str
) -> dict[Path, VersionUpdate]:
    cargo = read_regular_file(root, CANONICAL_MANIFEST)
    lock = read_regular_file(root, LOCK_FILE)
    plugin = read_regular_file(root, PLUGIN_MANIFEST)

    cargo_new = replace_once(
        cargo,
        f'version = "{old_version}"'.encode(),
        f'version = "{new_version}"'.encode(),
        str(CANONICAL_MANIFEST),
    )
    lock_new = replace_once(
        lock,
        f'name = "key-insights"\nversion = "{old_version}"'.encode(),
        f'name = "key-insights"\nversion = "{new_version}"'.encode(),
        str(LOCK_FILE),
    )
    plugin_new = replace_once(
        plugin,
        f'"version": "{old_version}"'.encode(),
        f'"version": "{new_version}"'.encode(),
        str(PLUGIN_MANIFEST),
    )
    return {
        CANONICAL_MANIFEST: VersionUpdate(cargo, cargo_new),
        LOCK_FILE: VersionUpdate(lock, lock_new),
        PLUGIN_MANIFEST: VersionUpdate(plugin, plugin_new),
    }


def validate_candidates(
    root: Path, updates: dict[Path, VersionUpdate], new_version: str
) -> None:
    with tempfile.TemporaryDirectory(prefix="key-insights-release-check-") as temporary:
        candidate_root = Path(temporary)
        for relative in REQUIRED_FILES:
            destination = candidate_root / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            update = updates.get(relative)
            destination.write_bytes(
                update.replacement if update is not None else read_regular_file(root, relative)
            )
        if validate_contract(candidate_root) != new_version:
            fail("updated files did not produce the requested release version")


def stage_file(path: Path, data: bytes, label: str) -> Path:
    mode = stat.S_IMODE(path.stat().st_mode)
    temporary_name: str | None = None
    try:
        with tempfile.NamedTemporaryFile(
            dir=path.parent,
            prefix=f".release-{label}-{path.name}-",
            delete=False,
        ) as temporary:
            temporary_name = temporary.name
            temporary.write(data)
            temporary.flush()
            os.fsync(temporary.fileno())
        os.chmod(temporary_name, mode)
        return Path(temporary_name)
    except OSError as error:
        if temporary_name is not None:
            try:
                os.unlink(temporary_name)
            except OSError:
                pass
        fail(f"cannot stage {path}: {error}")


def remove_if_present(path: Path) -> None:
    try:
        path.unlink(missing_ok=True)
    except OSError:
        pass


def assert_originals_unchanged(root: Path, updates: dict[Path, VersionUpdate]) -> None:
    for relative, update in updates.items():
        if read_regular_file(root, relative) != update.original:
            fail(f"{relative} changed while preparing the version update")


def write_updates_transactionally(
    root: Path, updates: dict[Path, VersionUpdate]
) -> None:
    assert_originals_unchanged(root, updates)
    staged: list[tuple[Path, Path, Path, Path]] = []
    try:
        for relative in (CANONICAL_MANIFEST, LOCK_FILE, PLUGIN_MANIFEST):
            destination = root / relative
            update = updates[relative]
            replacement = stage_file(destination, update.replacement, "new")
            try:
                backup = stage_file(destination, update.original, "old")
            except (OSError, ContractError):
                remove_if_present(replacement)
                raise
            staged.append((relative, destination, replacement, backup))
    except (OSError, ContractError):
        for _, _, replacement, backup in staged:
            remove_if_present(replacement)
            remove_if_present(backup)
        raise

    installed: list[tuple[Path, Path]] = []
    try:
        assert_originals_unchanged(root, updates)
        for relative, destination, replacement, backup in staged:
            if read_regular_file(root, relative) != updates[relative].original:
                fail(f"{relative} changed while installing the version update")
            os.replace(replacement, destination)
            installed.append((destination, backup))
    except BaseException as error:
        rollback_errors: list[tuple[Path, str]] = []
        for destination, backup in reversed(installed):
            try:
                os.replace(backup, destination)
            except BaseException as rollback_error:
                rollback_errors.append(
                    (backup, f"{destination}: {rollback_error}")
                )
        failed_backups = {backup for backup, _ in rollback_errors}
        for _, _, replacement, backup in staged:
            remove_if_present(replacement)
            if backup not in failed_backups:
                remove_if_present(backup)
        if rollback_errors:
            fail(
                "version update failed and rollback was incomplete; preserve "
                "these recovery backups: "
                + "; ".join(
                    f"{backup} ({message})" for backup, message in rollback_errors
                )
            )
        raise

    for _, _, _, backup in staged:
        remove_if_present(backup)


def bump(root: Path, old_version: str, new_version: str) -> None:
    stable_version(old_version, "--from")
    stable_version(new_version, "--to")
    if old_version == new_version:
        fail("--from and --to must differ")
    current = validate_contract(root)
    if current != old_version:
        fail(f"--from {old_version} does not match current version {current}")

    updates = candidate_updates(root, old_version, new_version)
    validate_candidates(root, updates, new_version)
    write_updates_transactionally(root, updates)


def validate_nix_versions(root: Path, version: str, systems: list[str]) -> None:
    for system in systems:
        if not system or any(character.isspace() for character in system):
            fail(f"invalid Nix system {system!r}")
        for package in (
            "key-insights",
            "nvim-key-insights",
            "nvim-key-insights-codex-plugin",
        ):
            attribute = f".#packages.{system}.{package}.version"
            try:
                result = subprocess.run(
                    ["nix", "eval", "--no-update-lock-file", "--raw", attribute],
                    cwd=root,
                    check=False,
                    capture_output=True,
                    text=True,
                )
            except OSError as error:
                fail(f"cannot evaluate Nix package {package} for {system}: {error}")
            if result.returncode != 0:
                diagnostic = result.stderr.strip().splitlines()
                detail = diagnostic[-1] if diagnostic else "nix eval failed"
                fail(f"cannot evaluate Nix package {package} for {system}: {detail}")
            if result.stdout != version:
                fail(
                    f"Nix package {package} for {system} has version "
                    f"{result.stdout!r}, expected {version}"
                )


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    command.add_argument("--root", type=Path, default=Path.cwd())
    subcommands = command.add_subparsers(dest="command", required=True)

    check = subcommands.add_parser("check", help="validate the release version contract")
    check.add_argument("--tag", help="require an exact vX.Y.Z release tag")
    check.add_argument(
        "--nix-system",
        action="append",
        default=[],
        help="evaluate package versions for a Nix system (repeatable)",
    )

    update = subcommands.add_parser("bump", help="update synchronized version mirrors")
    update.add_argument("--from", dest="old_version", required=True)
    update.add_argument("--to", dest="new_version", required=True)
    return command


def main() -> int:
    arguments = parser().parse_args()
    root = arguments.root.resolve()
    try:
        if arguments.command == "check":
            version = validate_contract(root, arguments.tag)
            validate_nix_versions(root, version, arguments.nix_system)
            print(f"release contract {version}: ok")
        else:
            bump(root, arguments.old_version, arguments.new_version)
            print(f"release version {arguments.old_version} -> {arguments.new_version}: updated")
    except (ContractError, OSError) as error:
        print(f"release: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
