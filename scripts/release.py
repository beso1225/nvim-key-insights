#!/usr/bin/env python3
"""Validate and update the repository release version without publishing."""

from __future__ import annotations

import argparse
import ctypes
import errno
import hashlib
import io
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import tomllib
from collections import Counter
from dataclasses import dataclass
from datetime import date
from pathlib import Path
from typing import NoReturn


VERSION_PATTERN = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)")
CANONICAL_MANIFEST = Path("crates/key-insights-cli/Cargo.toml")
LOCK_FILE = Path("Cargo.lock")
FLAKE_FILE = Path("flake.nix")
PLUGIN_MANIFEST = Path("plugins/nvim-key-insights/.codex-plugin/plugin.json")
PLUGIN_ROOT = Path("plugins/nvim-key-insights")
PLUGIN_ARTIFACT_FILES = (
    Path(".codex-plugin/plugin.json"),
    Path("skills/analyze-neovim-usage/SKILL.md"),
    Path("skills/analyze-neovim-usage/agents/openai.yaml"),
    Path("skills/analyze-neovim-usage/references/payload.schema.json"),
    Path("skills/analyze-neovim-usage/references/suggestions.schema.json"),
)
MAX_PLUGIN_ARTIFACT_SOURCE_BYTES = 2 * 1024 * 1024
MAX_GIT_METADATA_BYTES = 4 * 1024 * 1024
MARKETPLACE = Path(".agents/plugins/marketplace.json")
REQUIRED_FILES = (
    CANONICAL_MANIFEST,
    LOCK_FILE,
    FLAKE_FILE,
    PLUGIN_MANIFEST,
    MARKETPLACE,
)
SCHEMA_REQUIRED_FILES = (
    Path("codex/payload.schema.json"),
    Path("codex/suggestions.schema.json"),
    Path("crates/key-insights-cli/src/analyzer.rs"),
    Path("crates/key-insights-cli/src/codex_payload.rs"),
    Path("crates/key-insights-cli/src/codex_suggestions.rs"),
    Path("crates/key-insights-cli/src/ergonomics.rs"),
    Path("crates/key-insights-cli/src/keymap_snapshot.rs"),
    Path("crates/key-insights-cli/src/lib.rs"),
    Path("docs/schema-compatibility.md"),
    Path("lua/key-insights/contract_versions.lua"),
    Path("lua/key-insights/keymap_snapshot.lua"),
    Path("lua/key-insights/report.lua"),
    Path("lua/key-insights/schema.lua"),
    Path(
        "plugins/nvim-key-insights/skills/analyze-neovim-usage/"
        "references/payload.schema.json"
    ),
    Path(
        "plugins/nvim-key-insights/skills/analyze-neovim-usage/"
        "references/suggestions.schema.json"
    ),
)
SCHEMA_VERSIONS = {
    "Event log": 1,
    "Analysis summary": 3,
    "Keymap snapshot": 1,
    "Codex payload": 1,
    "Codex suggestions": 1,
    "Ergonomics contract": 1,
    "Histogram layout": 1,
    "Operation token set": 1,
    "Count-prefix token set": 1,
    "Directional-motion token set": 1,
    "Candidate kind": 1,
}
MAPPING_IDENTITY = "mapping-v1"
MAPPING_CANDIDATE_IDENTITY = "mapping-unobserved-v1"
CHANGELOG_FILE = Path("CHANGELOG.md")
LICENSE_FILE = Path("LICENSE")
RELEASE_DOCUMENTATION_FILES = (
    CHANGELOG_FILE,
    Path("README.md"),
    Path("docs/installation.md"),
    Path("docs/releasing.md"),
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


def rust_constant(root: Path, relative: Path, name: str) -> int:
    try:
        source = read_regular_file(root, relative).decode("utf-8")
    except UnicodeDecodeError as error:
        fail(f"schema source {relative} is not UTF-8: {error}")
    matches = re.findall(
        rf"^(?:pub )?const {re.escape(name)}: u32 = ([0-9]+);$",
        source,
        re.MULTILINE,
    )
    if len(matches) != 1:
        fail(f"schema source {relative} must define exactly one {name}")
    return int(matches[0])


def lua_named_version(root: Path, relative: Path, name: str) -> int:
    try:
        source = read_regular_file(root, relative).decode("utf-8")
    except UnicodeDecodeError as error:
        fail(f"schema source {relative} is not UTF-8: {error}")
    matches = re.findall(
        rf"^\s*{re.escape(name)} = ([0-9]+),$", source, re.MULTILINE
    )
    if len(matches) != 1:
        fail(f"schema source {relative} must define exactly one {name}")
    return int(matches[0])


def json_field(document: object, path: tuple[str, ...], field: str) -> object:
    value = document
    for component in path:
        if not isinstance(value, dict) or component not in value:
            fail(f"schema {field} is missing {'.'.join(path)}")
        value = value[component]
    return value


def schema_const(document: object, path: tuple[str, ...], field: str) -> int:
    value = json_field(document, path, field)
    if not isinstance(value, int) or isinstance(value, bool):
        fail(f"schema {field} must be an integer const")
    return value


def schema_nullable_version(
    document: object, path: tuple[str, ...], expected: int, field: str
) -> None:
    value = json_field(document, path, field)
    if not isinstance(value, dict):
        fail(f"schema {field} must be an object")
    if value.get("type") != ["integer", "null"]:
        fail(f"schema {field} must accept only integer or null")
    if value.get("minimum") != expected or value.get("maximum") != expected:
        fail(f"schema {field} must be null or exact version {expected}")


def json_patterns(value: object) -> list[str]:
    patterns: list[str] = []
    if isinstance(value, dict):
        for key, child in value.items():
            if key == "pattern" and isinstance(child, str):
                patterns.append(child)
            patterns.extend(json_patterns(child))
    elif isinstance(value, list):
        for child in value:
            patterns.extend(json_patterns(child))
    return patterns


def validate_schema_contract(root: Path) -> None:
    for relative in SCHEMA_REQUIRED_FILES:
        read_regular_file(root, relative)

    runtime_versions = {
        "Event log": rust_constant(root, Path("crates/key-insights-cli/src/lib.rs"), "SCHEMA_VERSION"),
        "Analysis summary": rust_constant(
            root, Path("crates/key-insights-cli/src/analyzer.rs"), "SUMMARY_SCHEMA_VERSION"
        ),
        "Keymap snapshot": rust_constant(
            root, Path("crates/key-insights-cli/src/keymap_snapshot.rs"), "SNAPSHOT_VERSION"
        ),
        "Codex payload": rust_constant(
            root,
            Path("crates/key-insights-cli/src/codex_payload.rs"),
            "CODEX_PAYLOAD_SCHEMA_VERSION",
        ),
        "Codex suggestions": rust_constant(
            root,
            Path("crates/key-insights-cli/src/codex_suggestions.rs"),
            "CODEX_SUGGESTIONS_SCHEMA_VERSION",
        ),
        "Ergonomics contract": rust_constant(
            root,
            Path("crates/key-insights-cli/src/ergonomics.rs"),
            "ERGONOMICS_CONTRACT_VERSION",
        ),
        "Histogram layout": rust_constant(
            root, Path("crates/key-insights-cli/src/ergonomics.rs"), "HISTOGRAM_VERSION"
        ),
        "Operation token set": rust_constant(
            root,
            Path("crates/key-insights-cli/src/ergonomics.rs"),
            "OPERATION_TOKEN_SET_VERSION",
        ),
        "Count-prefix token set": rust_constant(
            root,
            Path("crates/key-insights-cli/src/ergonomics.rs"),
            "COUNTABLE_TOKEN_SET_VERSION",
        ),
        "Directional-motion token set": rust_constant(
            root,
            Path("crates/key-insights-cli/src/ergonomics.rs"),
            "DIRECTIONAL_MOTION_TOKEN_SET_VERSION",
        ),
        "Candidate kind": rust_constant(
            root,
            Path("crates/key-insights-cli/src/ergonomics.rs"),
            "CANDIDATE_KIND_VERSION",
        ),
    }
    for contract, expected in SCHEMA_VERSIONS.items():
        if runtime_versions.get(contract) != expected:
            fail(
                f"schema runtime version for {contract} is "
                f"{runtime_versions.get(contract)}, expected {expected}"
            )

    lua_contract_path = Path("lua/key-insights/contract_versions.lua")
    lua_version_fields = {
        "Event log": "event_log",
        "Analysis summary": "analysis_summary",
        "Keymap snapshot": "keymap_snapshot",
        "Codex payload": "codex_payload",
        "Codex suggestions": "codex_suggestions",
        "Ergonomics contract": "ergonomics",
        "Histogram layout": "histogram",
        "Operation token set": "operation_token_set",
        "Count-prefix token set": "count_prefix_token_set",
        "Directional-motion token set": "directional_motion_token_set",
        "Candidate kind": "candidate_kind",
    }
    lua_versions = {
        contract: lua_named_version(root, lua_contract_path, field)
        for contract, field in lua_version_fields.items()
    }
    for contract, actual in lua_versions.items():
        expected = SCHEMA_VERSIONS[contract]
        if actual != expected:
            fail(f"schema Lua version for {contract} is {actual}, expected {expected}")

    try:
        lua_contract_source = read_regular_file(root, lua_contract_path).decode("utf-8")
    except UnicodeDecodeError as error:
        fail(f"schema source {lua_contract_path} is not UTF-8: {error}")
    summary_table = re.search(
        r"^\s*report_summary_versions = \{\n(?P<body>.*?)^\s*\},$",
        lua_contract_source,
        re.MULTILINE | re.DOTALL,
    )
    if summary_table is None:
        fail("schema Lua report summary versions table is missing")
    body = summary_table.group("body")
    summary_entries = re.findall(r"^\s*\[([0-9]+)\] = true,$", body, re.MULTILINE)
    remainder = re.sub(r"^\s*\[[0-9]+\] = true,\s*$", "", body, flags=re.MULTILINE)
    expected_summary_versions = {1, 2, SCHEMA_VERSIONS["Analysis summary"]}
    if (
        remainder.strip()
        or len(summary_entries) != len(set(summary_entries))
        or {int(version) for version in summary_entries} != expected_summary_versions
    ):
        fail(
            "schema Lua report summary versions must be exactly "
            f"{sorted(expected_summary_versions)} with true values"
        )

    lua_writer_contracts = {
        Path("lua/key-insights/schema.lua"): (
            'local contract_versions = require("key-insights.contract_versions")',
            "M.VERSION = contract_versions.event_log",
        ),
        Path("lua/key-insights/keymap_snapshot.lua"): (
            'local contract_versions = require("key-insights.contract_versions")',
            "M.VERSION = contract_versions.keymap_snapshot",
        ),
    }
    for relative, expressions in lua_writer_contracts.items():
        try:
            source = read_regular_file(root, relative).decode("utf-8")
        except UnicodeDecodeError as error:
            fail(f"schema source {relative} is not UTF-8: {error}")
        for expression in expressions:
            if expression not in source:
                fail(f"schema Lua writer {relative} is missing {expression}")

    report_path = Path("lua/key-insights/report.lua")
    try:
        report_source = read_regular_file(root, report_path).decode("utf-8")
    except UnicodeDecodeError as error:
        fail(f"schema source lua/key-insights/report.lua is not UTF-8: {error}")
    numeric_version_comparison = re.search(
        r"(?:schema_version|snapshot_version|contract_version|"
        r"histogram_version|token_set_version|kind_version) ~= [0-9]",
        report_source,
    )
    if numeric_version_comparison is not None:
        fail(
            "schema Lua validator contains a numeric version gate instead of "
            "contract_versions"
        )
    expected_report_references = Counter(
        {
            "analysis_summary": 1,
            "candidate_kind": 1,
            "codex_payload": 1,
            "codex_suggestions": 1,
            "count_prefix_token_set": 1,
            "directional_motion_token_set": 1,
            "ergonomics": 1,
            "histogram": 1,
            "keymap_snapshot": 6,
            "operation_token_set": 1,
            "report_summary_versions": 1,
        }
    )
    actual_report_references = Counter(
        re.findall(r"contract_versions\.([a-z_]+)", report_source)
    )
    if actual_report_references != expected_report_references:
        fail(
            "schema Lua validator contract_versions references drifted: "
            f"{dict(actual_report_references)}"
        )
    lua_report_contracts = {
        "decoded.payload_schema_version ~= contract_versions.codex_payload": "Codex payload",
        "summary.schema_version ~= contract_versions.analysis_summary": "Analysis summary",
        "document.schema_version ~= contract_versions.codex_suggestions": "Codex suggestions",
        "contract_versions.report_summary_versions[summary.schema_version] ~= true": (
            "legacy report summary versions"
        ),
    }
    for expression, contract in lua_report_contracts.items():
        if expression not in report_source:
            fail(f"schema Lua validator is missing {contract} version expression")

    payload_path = Path("codex/payload.schema.json")
    suggestions_path = Path("codex/suggestions.schema.json")
    payload_bytes = read_regular_file(root, payload_path)
    suggestions_bytes = read_regular_file(root, suggestions_path)
    payload = parse_json(payload_bytes, str(payload_path))
    suggestions = parse_json(suggestions_bytes, str(suggestions_path))
    schema_versions = {
        "Codex payload": schema_const(
            payload, ("properties", "payload_schema_version", "const"), "Codex payload"
        ),
        "Analysis summary": schema_const(
            payload,
            ("$defs", "summary", "properties", "schema_version", "const"),
            "Analysis summary",
        ),
        "Keymap snapshot": schema_const(
            payload,
            ("$defs", "keymap_snapshot", "properties", "snapshot_version", "const"),
            "Keymap snapshot",
        ),
        "Ergonomics contract": schema_const(
            payload,
            ("$defs", "ergonomics", "properties", "contract_version", "const"),
            "Ergonomics contract",
        ),
        "Histogram layout": schema_const(
            payload,
            (
                "$defs",
                "ergonomics",
                "properties",
                "distributions",
                "properties",
                "histogram_version",
                "const",
            ),
            "Histogram layout",
        ),
        "Operation token set": schema_const(
            payload,
            (
                "$defs",
                "ergonomics",
                "properties",
                "operations",
                "properties",
                "token_set_version",
                "const",
            ),
            "Operation token set",
        ),
        "Count-prefix token set": schema_const(
            payload,
            (
                "$defs",
                "ergonomics",
                "properties",
                "count_prefixes",
                "properties",
                "token_set_version",
                "const",
            ),
            "Count-prefix token set",
        ),
        "Directional-motion token set": schema_const(
            payload,
            (
                "$defs",
                "ergonomics",
                "properties",
                "repeated_motions",
                "properties",
                "token_set_version",
                "const",
            ),
            "Directional-motion token set",
        ),
        "Candidate kind": schema_const(
            payload,
            ("$defs", "candidate", "properties", "kind_version", "const"),
            "Candidate kind",
        ),
        "Codex suggestions": schema_const(
            suggestions,
            ("properties", "schema_version", "const"),
            "Codex suggestions",
        ),
    }
    for contract, actual in schema_versions.items():
        expected = SCHEMA_VERSIONS[contract]
        if actual != expected:
            fail(f"schema JSON version for {contract} is {actual}, expected {expected}")

    attribution_snapshot_version = schema_const(
        payload,
        (
            "$defs",
            "mapping_attribution",
            "properties",
            "snapshot_version",
            "const",
        ),
        "mapping attribution snapshot",
    )
    if attribution_snapshot_version != SCHEMA_VERSIONS["Keymap snapshot"]:
        fail(
            "schema JSON version for mapping attribution snapshot is "
            f"{attribution_snapshot_version}, expected "
            f"{SCHEMA_VERSIONS['Keymap snapshot']}"
        )
    schema_nullable_version(
        payload,
        (
            "$defs",
            "ergonomics",
            "properties",
            "mapping_coverage",
            "properties",
            "snapshot_version",
        ),
        SCHEMA_VERSIONS["Keymap snapshot"],
        "mapping coverage snapshot",
    )

    identity_code_contracts = {
        Path("crates/key-insights-cli/src/keymap_snapshot.rs"): (
            'append_length_prefixed(&mut preimage, "mapping-v1");',
            'format!("mapping-v1:{:x}", Sha256::digest(preimage.as_bytes()))',
        ),
        Path("crates/key-insights-cli/src/codex_payload.rs"): (
            'mapping_id.strip_prefix("mapping-v1:")',
            '.strip_prefix("mapping-unobserved-v1:")',
        ),
        Path("crates/key-insights-cli/src/codex_suggestions.rs"): (
            'value.strip_prefix("mapping-v1:")',
        ),
        Path("lua/key-insights/keymap_snapshot.lua"): (
            'length_prefix("mapping-v1")',
            'return "mapping-v1:" .. digest',
        ),
        Path("lua/key-insights/report.lua"): (
            'string.sub(value, 1, 11) ~= "mapping-v1:"',
            '#"mapping-v1" .. ":mapping-v1"',
            'return "mapping-v1:" .. vim.fn.sha256(preimage)',
            '"^mapping%-unobserved%-v1:(mapping%-v1:.+)$"',
        ),
        Path("crates/key-insights-cli/src/ergonomics.rs"): (
            'format!("mapping-unobserved-v1:{}", mapping.mapping_id)',
        ),
    }
    for relative, expressions in identity_code_contracts.items():
        try:
            source = read_regular_file(root, relative).decode("utf-8")
        except UnicodeDecodeError as error:
            fail(f"schema source {relative} is not UTF-8: {error}")
        for expression in expressions:
            if expression not in source:
                fail(
                    f"schema mapping identity {MAPPING_IDENTITY} is missing "
                    f"from {relative}"
                )
    expected_identity_patterns = {
        "payload": Counter(
            {
                "^mapping-v1:[0-9a-f]{64}$": 1,
                "^mapping-unobserved-v1:mapping-v1:[0-9a-f]{64}$": 2,
            }
        ),
        "suggestions": Counter({"^mapping-v1:[0-9a-f]{64}$": 2}),
    }
    for field, document in (("payload", payload), ("suggestions", suggestions)):
        identity_patterns = Counter(
            pattern for pattern in json_patterns(document) if "mapping-" in pattern
        )
        if identity_patterns != expected_identity_patterns[field]:
            fail(
                f"schema mapping identity patterns in {field} do not match "
                f"{MAPPING_IDENTITY} and {MAPPING_CANDIDATE_IDENTITY}"
            )

    bundled_payload = read_regular_file(
        root,
        Path(
            "plugins/nvim-key-insights/skills/analyze-neovim-usage/"
            "references/payload.schema.json"
        ),
    )
    bundled_suggestions = read_regular_file(
        root,
        Path(
            "plugins/nvim-key-insights/skills/analyze-neovim-usage/"
            "references/suggestions.schema.json"
        ),
    )
    if bundled_payload != payload_bytes:
        fail("schema standalone payload copy differs from the canonical schema")
    if bundled_suggestions != suggestions_bytes:
        fail("schema standalone suggestions copy differs from the canonical schema")

    documentation_path = Path("docs/schema-compatibility.md")
    try:
        documentation = read_regular_file(root, documentation_path).decode("utf-8")
    except UnicodeDecodeError as error:
        fail(f"schema compatibility documentation is not UTF-8: {error}")
    for contract, version in SCHEMA_VERSIONS.items():
        if f"| {contract} | `{version}` |" not in documentation:
            fail(f"schema compatibility table is missing {contract} version {version}")
    if f"| Mapping identity | `{MAPPING_IDENTITY}` |" not in documentation:
        fail(
            "schema compatibility table is missing mapping identity "
            f"{MAPPING_IDENTITY}"
        )
    if (
        f"| Mapping-underuse candidate identity | `{MAPPING_CANDIDATE_IDENTITY}` |"
        not in documentation
    ):
        fail(
            "schema compatibility table is missing mapping candidate identity "
            f"{MAPPING_CANDIDATE_IDENTITY}"
        )
    required_policy = (
        "unknown versions fail closed",
        "Removing an event reader requires a package major release.",
        "Regenerate",
        "Do not reuse an existing schema number",
        "freshness check may recognize summary schemas 1 and 2",
    )
    normalized_documentation = " ".join(documentation.split())
    for statement in required_policy:
        if statement not in normalized_documentation:
            fail(f"schema compatibility policy is missing {statement!r}")


def decode_document(root: Path, relative: Path, field: str) -> str:
    try:
        return read_regular_file(root, relative).decode("utf-8")
    except UnicodeDecodeError as error:
        fail(f"{field} is not UTF-8: {error}")


def release_date(value: str) -> date:
    try:
        parsed = date.fromisoformat(value)
    except ValueError:
        fail(f"release date {value!r} must be a real YYYY-MM-DD date")
    if parsed.isoformat() != value:
        fail(f"release date {value!r} must use canonical YYYY-MM-DD form")
    return parsed


def version_order(value: str) -> tuple[int, int, int]:
    stable_version(value, "changelog release version")
    return tuple(int(component) for component in value.split("."))


def validate_changelog_text(contents: str, version: str, tag: str | None) -> None:
    if not contents.startswith("# Changelog\n"):
        fail("changelog must start with # Changelog")
    if contents.count("## [Unreleased]") != 1:
        fail("changelog must contain exactly one Unreleased section")
    heading_pattern = re.compile(
        r"^## \[((?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*))\]"
        r" - ([^\n]+)$",
        re.MULTILINE,
    )
    heading_matches = list(heading_pattern.finditer(contents))
    headings = [match.groups() for match in heading_matches]
    heading_versions = [heading_version for heading_version, _ in headings]
    all_h2 = list(re.finditer(r"^## ([^\n]+)$", contents, re.MULTILINE))
    if not all_h2 or all_h2[0].group(1) != "[Unreleased]":
        fail("changelog Unreleased section must be the first H2 section")
    dated_heading_starts = {match.start() for match in heading_matches}
    for heading in all_h2[1:]:
        if heading.start() not in dated_heading_starts:
            fail(f"changelog contains unexpected H2 {heading.group(1)!r}")
    if len(heading_versions) != len(set(heading_versions)):
        fail("changelog contains a duplicate release version")
    parsed_dates = [release_date(heading_date) for _, heading_date in headings]
    if tag is not None:
        matching = [heading_date for heading_version, heading_date in headings if heading_version == version]
        if len(matching) != 1:
            fail(f"changelog has no unique dated release entry for {version}")
        if not headings or headings[0][0] != version:
            fail(f"latest changelog release must be {version}")
    for newer, older in zip(heading_versions, heading_versions[1:]):
        if version_order(newer) <= version_order(older):
            fail("changelog release version order must be strictly descending")
    for newer, older in zip(parsed_dates, parsed_dates[1:]):
        if newer < older:
            fail("changelog release date order must be non-increasing")
    if tag is not None:
        current_index = heading_versions.index(version)
        section_start = heading_matches[current_index].end()
        section_end = (
            heading_matches[current_index + 1].start()
            if current_index + 1 < len(heading_matches)
            else len(contents)
        )
        release_notes = contents[section_start:section_end]
        if (
            re.search(r"^### [^\n]+$", release_notes, re.MULTILINE) is None
            or re.search(r"^- [^\n]+$", release_notes, re.MULTILINE) is None
        ):
            fail(f"changelog release notes for {version} must contain a section and entry")


def changelog_release_notes(contents: str, version: str) -> str:
    heading = re.search(
        rf"^## \[{re.escape(version)}\] - [^\n]+$",
        contents,
        re.MULTILINE,
    )
    if heading is None:
        fail(f"changelog has no release notes for {version}")
    next_heading = re.search(r"^## [^\n]+$", contents[heading.end() :], re.MULTILINE)
    end = heading.end() + next_heading.start() if next_heading is not None else len(contents)
    notes = contents[heading.end() : end].strip()
    if not notes:
        fail(f"changelog release notes for {version} are empty")
    return notes + "\n"


def validate_release_documentation(root: Path, version: str, tag: str | None) -> None:
    documents = {
        relative: decode_document(root, relative, str(relative))
        for relative in RELEASE_DOCUMENTATION_FILES
    }
    validate_changelog_text(documents[CHANGELOG_FILE], version, tag)
    if tag is not None:
        license_text = decode_document(root, LICENSE_FILE, str(LICENSE_FILE))
        if not license_text.strip():
            fail("LICENSE must be nonempty")
    releasing = documents[Path("docs/releasing.md")]
    normalized_releasing = " ".join(releasing.split())
    required_release_phrases = (
        "release.py prepare-changelog",
        "release.py build-artifacts",
        "pkf run --no-cache check",
        "nix flake check --no-update-lock-file",
        "git tag -a",
        "does not publish to crates.io",
        "rollback",
        "existing GitHub release",
        "contents: write",
        "SHA256SUMS",
    )
    for phrase in required_release_phrases:
        if phrase not in normalized_releasing:
            fail(f"release documentation is missing {phrase!r}")
    installation = documents[Path("docs/installation.md")]
    for phrase in (
        'version = "v0.1.0"',
        "?ref=v0.1.0#key-insights",
        "nvim-key-insights@v0.1.0",
        "schema-compatibility.md",
        "releasing.md",
    ):
        if phrase not in installation:
            fail(f"installation documentation is missing {phrase!r}")
    readme = documents[Path("README.md")]
    for link in ("CHANGELOG.md", "docs/schema-compatibility.md", "docs/releasing.md"):
        if link not in readme:
            fail(f"README is missing release documentation link {link}")


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


def write_single_update(root: Path, relative: Path, original: bytes, replacement: bytes) -> None:
    if read_regular_file(root, relative) != original:
        fail(f"{relative} changed while preparing the update")
    destination = root / relative
    staged = stage_file(destination, replacement, "new")
    try:
        if read_regular_file(root, relative) != original:
            fail(f"{relative} changed while installing the update")
        os.replace(staged, destination)
    except BaseException:
        remove_if_present(staged)
        raise


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


def prepare_changelog(root: Path, version: str, date_value: str) -> None:
    stable_version(version, "--version")
    release_date(date_value)
    current_version = validate_contract(root)
    if version != current_version:
        fail(f"--version {version} does not match current version {current_version}")
    original = read_regular_file(root, CHANGELOG_FILE)
    try:
        contents = original.decode("utf-8")
    except UnicodeDecodeError as error:
        fail(f"changelog is not UTF-8: {error}")
    validate_changelog_text(contents, version, None)
    if re.search(rf"^## \[{re.escape(version)}\] - ", contents, re.MULTILINE):
        fail(f"changelog already contains release {version}")
    marker = "## [Unreleased]\n\n"
    if contents.count(marker) != 1:
        fail("changelog Unreleased section must be followed by one blank line")
    updated = contents.replace(
        marker,
        f"{marker}## [{version}] - {date_value}\n\n",
        1,
    )
    validate_changelog_text(updated, version, f"v{version}")
    write_single_update(root, CHANGELOG_FILE, original, updated.encode("utf-8"))


def publish_new_file(destination: Path, data: bytes) -> None:
    parent = destination.parent
    try:
        parent_metadata = parent.lstat()
    except OSError as error:
        fail(f"cannot inspect output parent {parent}: {error}")
    if stat.S_ISLNK(parent_metadata.st_mode) or not stat.S_ISDIR(parent_metadata.st_mode):
        fail("output parent must be a directory and not a symlink")
    try:
        destination.lstat()
    except FileNotFoundError:
        pass
    except OSError as error:
        fail(f"cannot inspect output {destination}: {error}")
    else:
        fail(f"output {destination} already exists")

    staged_name: str | None = None
    try:
        with tempfile.NamedTemporaryFile(
            dir=parent,
            prefix=f".release-new-{destination.name}-",
            delete=False,
        ) as staged:
            staged_name = staged.name
            staged.write(data)
            staged.flush()
            os.fsync(staged.fileno())
        os.chmod(staged_name, 0o644)
        try:
            os.link(staged_name, destination)
        except FileExistsError:
            fail(f"output {destination} already exists")
    finally:
        if staged_name is not None:
            remove_if_present(Path(staged_name))


def write_release_notes(root: Path, tag: str, output: Path) -> None:
    commit = git_commit(root)
    version = git_release_version(root, commit)
    if tag != f"v{version}":
        fail(f"release tag {tag!r} must exactly match v{version}")
    try:
        changelog = git_path_blob(root, commit, CHANGELOG_FILE).decode("utf-8")
        license_text = git_path_blob(root, commit, LICENSE_FILE).decode("utf-8")
    except UnicodeDecodeError as error:
        fail(f"release documentation in Git is not UTF-8: {error}")
    validate_changelog_text(changelog, version, tag)
    if not license_text.strip():
        fail("LICENSE in Git must be nonempty")
    notes = changelog_release_notes(changelog, version).encode("utf-8")
    destination = output if output.is_absolute() else root / output
    publish_new_file(destination, notes)


def git_environment() -> dict[str, str]:
    environment = os.environ.copy()
    environment["GIT_NO_REPLACE_OBJECTS"] = "1"
    return environment


def git_output(
    root: Path,
    arguments: list[str],
    field: str,
    max_output_bytes: int = MAX_GIT_METADATA_BYTES,
) -> bytes:
    try:
        with tempfile.TemporaryFile() as diagnostic:
            process = subprocess.Popen(
                ["git", *arguments],
                cwd=root,
                env=git_environment(),
                stdout=subprocess.PIPE,
                stderr=diagnostic,
            )
            if process.stdout is None:
                process.kill()
                process.wait()
                fail(f"cannot inspect {field} in Git: stdout pipe is unavailable")
            try:
                output = process.stdout.read(max_output_bytes + 1)
                if len(output) > max_output_bytes:
                    process.kill()
                    process.wait()
                    fail(f"Git output for {field} exceeds the size limit")
                return_code = process.wait()
            finally:
                process.stdout.close()
            diagnostic.seek(0)
            error_output = diagnostic.read(4097)
    except OSError as error:
        fail(f"cannot inspect {field} in Git: {error}")
    if return_code != 0:
        error_text = error_output.decode("utf-8", errors="replace").strip()
        if len(error_output) > 4096:
            error_text += "…"
        fail(f"cannot inspect {field} in Git: {error_text or 'git command failed'}")
    return output


def require_clean_git_path(root: Path, commit: str, relative: Path) -> None:
    try:
        result = subprocess.run(
            [
                "git",
                "diff",
                "--quiet",
                "--no-ext-diff",
                commit,
                "--",
                relative.as_posix(),
            ],
            cwd=root,
            env=git_environment(),
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    except OSError as error:
        fail(f"cannot compare {relative} with Git commit {commit}: {error}")
    if result.returncode == 1:
        fail("plugin artifact working tree must match the resolved Git commit")
    if result.returncode != 0:
        fail(f"cannot compare {relative} with Git commit {commit}")
    untracked = git_output(
        root,
        [
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            relative.as_posix(),
        ],
        f"untracked {relative}",
    )
    if untracked:
        fail("plugin artifact working tree must match the resolved Git commit")


def git_commit(root: Path) -> str:
    output = git_output(
        root,
        ["rev-parse", "--verify", "HEAD^{commit}"],
        "release commit",
        max_output_bytes=65,
    )
    try:
        commit = output.decode("ascii").strip()
    except UnicodeDecodeError:
        fail("Git returned a non-ASCII release commit")
    if re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", commit) is None:
        fail("Git returned an invalid release commit")
    return commit


def git_blob(root: Path, object_id: str, size: int, field: str) -> bytes:
    if size < 0 or size > MAX_PLUGIN_ARTIFACT_SOURCE_BYTES:
        fail(f"{field} exceeds the plugin artifact source size limit")
    data = git_output(
        root,
        ["cat-file", "blob", object_id],
        field,
        max_output_bytes=size,
    )
    if len(data) != size:
        fail(f"{field} Git blob size changed unexpectedly")
    return data


def git_path_blob(root: Path, commit: str, relative: Path) -> bytes:
    output = git_output(
        root,
        ["ls-tree", "-z", "--full-tree", commit, "--", relative.as_posix()],
        str(relative),
    )
    entries = [entry for entry in output.split(b"\0") if entry]
    if len(entries) != 1:
        fail(f"Git HEAD must contain exactly one {relative}")
    metadata, path_bytes = entries[0].split(b"\t", 1)
    mode, object_type, object_id = metadata.decode("ascii").split(" ")
    if path_bytes.decode("utf-8") != relative.as_posix():
        fail(f"Git returned an unexpected path for {relative}")
    if mode != "100644" or object_type != "blob":
        fail(f"Git {relative} must be a non-executable regular file")
    size_output = git_output(root, ["cat-file", "-s", object_id], str(relative))
    try:
        size = int(size_output)
    except ValueError:
        fail(f"Git returned an invalid size for {relative}")
    return git_blob(root, object_id, size, str(relative))


def collect_plugin_artifact_files(root: Path, commit: str) -> dict[Path, bytes]:
    require_clean_git_path(root, commit, PLUGIN_ROOT)

    output = git_output(
        root,
        ["ls-tree", "-r", "-z", "--full-tree", commit, "--", str(PLUGIN_ROOT)],
        str(PLUGIN_ROOT),
    )
    entries = [entry for entry in output.split(b"\0") if entry]
    objects: dict[Path, tuple[str, int]] = {}
    total = 0
    for entry in entries:
        try:
            metadata, path_bytes = entry.split(b"\t", 1)
            mode, object_type, object_id = metadata.decode("ascii").split(" ")
            path = Path(path_bytes.decode("utf-8"))
            relative = path.relative_to(PLUGIN_ROOT)
        except (UnicodeDecodeError, ValueError):
            fail("Git returned a malformed plugin artifact tree entry")
        if mode == "120000":
            fail(f"plugin artifact contains symlink {relative}")
        if mode != "100644" or object_type != "blob":
            fail(f"plugin artifact contains executable or non-regular file {relative}")
        size_output = git_output(root, ["cat-file", "-s", object_id], str(path))
        try:
            size = int(size_output)
        except ValueError:
            fail(f"Git returned an invalid size for {path}")
        total += size
        if total > MAX_PLUGIN_ARTIFACT_SOURCE_BYTES:
            fail("plugin artifact sources exceed the size limit")
        objects[relative] = (object_id, size)

    expected = set(PLUGIN_ARTIFACT_FILES)
    actual = set(objects)
    if actual != expected:
        missing = sorted(str(path) for path in expected - actual)
        unexpected = sorted(str(path) for path in actual - expected)
        fail(
            "plugin artifact allowlist mismatch: "
            f"missing={missing}, unexpected={unexpected}"
        )
    return {
        relative: git_blob(
            root,
            objects[relative][0],
            objects[relative][1],
            str(PLUGIN_ROOT / relative),
        )
        for relative in PLUGIN_ARTIFACT_FILES
    }


def validate_plugin_artifact_files(
    root: Path, commit: str, files: dict[Path, bytes], version: str
) -> None:
    manifest = parse_json(files[Path(".codex-plugin/plugin.json")], "artifact plugin manifest")
    if not isinstance(manifest, dict) or manifest.get("name") != "nvim-key-insights":
        fail("artifact plugin manifest identity is invalid")
    if manifest.get("version") != version:
        fail("artifact plugin manifest version does not match the release version")
    schema_pairs = (
        (
            Path("skills/analyze-neovim-usage/references/payload.schema.json"),
            Path("codex/payload.schema.json"),
        ),
        (
            Path("skills/analyze-neovim-usage/references/suggestions.schema.json"),
            Path("codex/suggestions.schema.json"),
        ),
    )
    for bundled, canonical in schema_pairs:
        if files[bundled] != git_path_blob(root, commit, canonical):
            fail(f"artifact schema {bundled} does not match {canonical}")


def git_release_version(root: Path, commit: str) -> str:
    cargo = parse_toml(
        git_path_blob(root, commit, CANONICAL_MANIFEST),
        f"Git HEAD {CANONICAL_MANIFEST}",
    )
    package = cargo.get("package")
    if not isinstance(package, dict) or package.get("name") != "key-insights":
        fail("Git HEAD canonical Cargo package must be named key-insights")
    return stable_version(package.get("version"), "Git HEAD Cargo package version")


def render_plugin_archive(
    files: dict[Path, bytes], version: str, epoch: int
) -> bytes:
    prefix = f"nvim-key-insights-codex-plugin-v{version}/"
    uncompressed = io.BytesIO()
    with tarfile.open(
        fileobj=uncompressed,
        mode="w",
        format=tarfile.USTAR_FORMAT,
    ) as archive:
        for relative in PLUGIN_ARTIFACT_FILES:
            data = files[relative]
            entry = tarfile.TarInfo(prefix + relative.as_posix())
            entry.size = len(data)
            entry.mode = 0o644
            entry.uid = 0
            entry.gid = 0
            entry.uname = ""
            entry.gname = ""
            entry.mtime = epoch
            archive.addfile(entry, io.BytesIO(data))

    return uncompressed.getvalue()


def rename_directory_noreplace(source: Path, destination: Path) -> None:
    libc = ctypes.CDLL(None, use_errno=True)
    source_bytes = os.fsencode(source)
    destination_bytes = os.fsencode(destination)
    if sys.platform == "linux":
        try:
            rename = libc.renameat2
        except AttributeError:
            fail("atomic no-replace directory publication is unavailable")
        rename.argtypes = [
            ctypes.c_int,
            ctypes.c_char_p,
            ctypes.c_int,
            ctypes.c_char_p,
            ctypes.c_uint,
        ]
        rename.restype = ctypes.c_int
        result = rename(-100, source_bytes, -100, destination_bytes, 1)
    elif sys.platform == "darwin":
        try:
            rename = libc.renamex_np
        except AttributeError:
            fail("atomic no-replace directory publication is unavailable")
        rename.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint]
        rename.restype = ctypes.c_int
        result = rename(source_bytes, destination_bytes, 0x00000004)
    else:
        fail("atomic no-replace directory publication is unsupported on this platform")
    if result == 0:
        return
    error_number = ctypes.get_errno()
    if error_number in (errno.EEXIST, errno.ENOTEMPTY):
        fail(f"artifact output {destination} already exists")
    fail(f"cannot publish artifact output {destination}: {os.strerror(error_number)}")


def build_artifacts(root: Path, output_dir: Path, version: str, epoch: int) -> None:
    stable_version(version, "--version")
    if epoch < 0 or epoch > 0xFFFFFFFF:
        fail("--epoch must be between 0 and 4294967295")
    commit = git_commit(root)
    current_version = git_release_version(root, commit)
    if version != current_version:
        fail(f"--version {version} does not match Git HEAD version {current_version}")
    files = collect_plugin_artifact_files(root, commit)
    validate_plugin_artifact_files(root, commit, files, version)

    destination = output_dir if output_dir.is_absolute() else root / output_dir
    try:
        destination.lstat()
    except FileNotFoundError:
        pass
    except OSError as error:
        fail(f"cannot inspect artifact output {destination}: {error}")
    else:
        fail(f"artifact output {destination} already exists")

    parent = destination.parent
    try:
        parent_metadata = parent.lstat()
    except OSError as error:
        fail(f"cannot inspect artifact output parent {parent}: {error}")
    if stat.S_ISLNK(parent_metadata.st_mode) or not stat.S_ISDIR(parent_metadata.st_mode):
        fail("artifact output parent must be a directory and not a symlink")

    archive_name = f"nvim-key-insights-codex-plugin-v{version}.tar"
    archive_bytes = render_plugin_archive(files, version, epoch)
    digest = hashlib.sha256(archive_bytes).hexdigest()
    checksums = f"{digest}  {archive_name}\n".encode("ascii")

    staged = Path(tempfile.mkdtemp(prefix=".release-artifacts-", dir=parent))
    installed = False
    try:
        archive_path = staged / archive_name
        checksum_path = staged / "SHA256SUMS"
        archive_path.write_bytes(archive_bytes)
        checksum_path.write_bytes(checksums)
        os.chmod(archive_path, 0o644)
        os.chmod(checksum_path, 0o644)
        rename_directory_noreplace(staged, destination)
        installed = True
    finally:
        if not installed:
            shutil.rmtree(staged, ignore_errors=True)


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

    changelog = subcommands.add_parser(
        "prepare-changelog",
        help="move Unreleased notes under a dated package release",
    )
    changelog.add_argument("--version", required=True)
    changelog.add_argument("--date", required=True)

    artifacts = subcommands.add_parser(
        "build-artifacts",
        help="build the deterministic Codex plugin release archive",
    )
    artifacts.add_argument("--version", required=True)
    artifacts.add_argument("--epoch", required=True, type=int)
    artifacts.add_argument("--output-dir", required=True, type=Path)

    notes = subcommands.add_parser(
        "release-notes",
        help="extract validated changelog notes for a release tag",
    )
    notes.add_argument("--tag", required=True)
    notes.add_argument("--output", required=True, type=Path)
    return command


def main() -> int:
    arguments = parser().parse_args()
    root = arguments.root.resolve()
    try:
        if arguments.command == "check":
            version = validate_contract(root, arguments.tag)
            validate_schema_contract(root)
            validate_release_documentation(root, version, arguments.tag)
            validate_nix_versions(root, version, arguments.nix_system)
            print(f"release contract {version}: ok")
        elif arguments.command == "bump":
            bump(root, arguments.old_version, arguments.new_version)
            print(f"release version {arguments.old_version} -> {arguments.new_version}: updated")
        elif arguments.command == "prepare-changelog":
            prepare_changelog(root, arguments.version, arguments.date)
            print(f"changelog release {arguments.version} ({arguments.date}): prepared")
        elif arguments.command == "build-artifacts":
            build_artifacts(root, arguments.output_dir, arguments.version, arguments.epoch)
            print(f"release artifacts {arguments.version}: built in {arguments.output_dir}")
        else:
            write_release_notes(root, arguments.tag, arguments.output)
            print(f"release notes {arguments.tag}: written to {arguments.output}")
    except (ContractError, OSError) as error:
        print(f"release: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
