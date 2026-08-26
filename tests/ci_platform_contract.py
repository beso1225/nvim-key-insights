#!/usr/bin/env python3

import re
from pathlib import Path

import yaml


ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github/workflows/ci.yml"
FLAKE = ROOT / "flake.nix"
TASKFILE = ROOT / "Taskfile.pkl"
PINNED_ACTIONS = {
    "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
    "DeterminateSystems/determinate-nix-action@61cbfe2efc2d4e7a8a6d56967c3c1058e846c858",
}
EXPECTED_NATIVE_PLATFORMS = {
    ("ubuntu-24.04", "x86_64-linux"),
    ("ubuntu-24.04-arm", "aarch64-linux"),
    ("macos-15", "aarch64-darwin"),
}


def fail(message: str) -> None:
    raise SystemExit(message)


document = yaml.load(WORKFLOW.read_text(), Loader=yaml.BaseLoader)
if not isinstance(document, dict):
    fail("CI workflow must be a YAML mapping")
if document.get("permissions") != {"contents": "read"}:
    fail("CI workflow default permissions must be contents: read")

jobs = document.get("jobs")
if not isinstance(jobs, dict):
    fail("CI workflow must define jobs")
if "check" not in jobs or jobs["check"].get("name") != "Project checks":
    fail("the existing Project checks job must remain stable")
if jobs["check"].get("runs-on") != "ubuntu-24.04":
    fail("Project checks must remain the x86_64-linux native gate")

native_job = jobs.get("native-platforms")
if not isinstance(native_job, dict):
    fail("CI must define a native-platforms matrix job")
if native_job.get("continue-on-error") is not None:
    fail("native platform failures must not be ignored")
strategy = native_job.get("strategy")
if not isinstance(strategy, dict) or strategy.get("fail-fast") != "false":
    fail("native platform matrix must run every supported platform")
matrix = strategy.get("matrix")
if not isinstance(matrix, dict) or set(matrix) != {"include"}:
    fail("native platform matrix must use an explicit runner/system allowlist")
rows = matrix["include"]
if not isinstance(rows, list):
    fail("native platform matrix include must be a list")
actual_platforms = {
    (row.get("runner"), row.get("system")) for row in rows if isinstance(row, dict)
}
if actual_platforms != EXPECTED_NATIVE_PLATFORMS - {("ubuntu-24.04", "x86_64-linux")}:
    fail(f"native platform matrix does not cover the missing flake systems: {actual_platforms}")
if native_job.get("runs-on") != "${{ matrix.runner }}":
    fail("native platform job must use the allowlisted matrix runner")
if any("latest" in runner for runner, _ in actual_platforms):
    fail("native platform jobs must not use mutable latest runner labels")

flake_text = FLAKE.read_text()
match = re.search(r"systems\s*=\s*\[([^]]+)\]", flake_text)
if match is None:
    fail("flake supported systems must remain explicit")
flake_systems = set(re.findall(r'"([^"]+)"', match.group(1)))
covered_systems = {"x86_64-linux"} | {system for _, system in actual_platforms}
if flake_systems != covered_systems:
    fail(f"native CI and flake systems differ: flake={flake_systems}, CI={covered_systems}")

all_steps = [
    step
    for job in jobs.values()
    if isinstance(job, dict)
    for step in job.get("steps", [])
    if isinstance(step, dict)
]
uses = [step["uses"] for step in all_steps if "uses" in step]
if set(uses) != PINNED_ACTIONS:
    fail(f"CI actions must match the immutable allowlist: {uses}")
for action in uses:
    if re.fullmatch(r"[^@]+@[0-9a-f]{40}", action) is None:
        fail(f"CI action is not pinned to a commit: {action}")
for step in all_steps:
    if step.get("uses", "").startswith("actions/checkout@"):
        if step.get("with", {}).get("persist-credentials") != "false":
            fail("checkout credentials must not persist")

native_script = "\n".join(step.get("run", "") for step in native_job["steps"])
for required in (
    "builtins.currentSystem",
    "${{ matrix.system }}",
    "nix flake check --no-update-lock-file --print-build-logs",
    "nix develop --no-update-lock-file --command pkf run --no-cache check",
):
    if required not in native_script:
        fail(f"native platform gate is missing {required!r}")
if re.search(r"nix (?:develop|flake check)(?![^\n]*--no-update-lock-file)", native_script):
    fail("native platform gates must reject implicit flake.lock updates")

taskfile = TASKFILE.read_text()
for required in (
    'name = "test:resource:rust"',
    'name = "test:resource:lua"',
    'name = "test:resource"',
    "testResource",
):
    if required not in taskfile:
        fail(f"pkfire resource contract is missing {required!r}")

print("CI native-platform and resource contract: ok")
