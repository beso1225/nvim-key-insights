#!/usr/bin/env python3

import json
import re
import subprocess
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
for job_name, job in jobs.items():
    if not isinstance(job, dict):
        fail(f"CI job {job_name!r} must be a mapping")
    if job.get("permissions") is not None:
        fail(f"CI job {job_name!r} must not override read-only workflow permissions")
    if job.get("continue-on-error") is not None:
        fail(f"CI job {job_name!r} failures must not be ignored")
    if job.get("if") is not None:
        fail(f"CI job {job_name!r} must not conditionally skip required gates")
    for step in job.get("steps", []):
        if isinstance(step, dict) and step.get("continue-on-error") is not None:
            fail(f"CI job {job_name!r} step failures must not be ignored")
        if isinstance(step, dict) and step.get("if") is not None:
            fail(f"CI job {job_name!r} steps must not conditionally skip required gates")
if "check" not in jobs or jobs["check"].get("name") != "Project checks":
    fail("the existing Project checks job must remain stable")
if jobs["check"].get("runs-on") != "ubuntu-24.04":
    fail("Project checks must remain the x86_64-linux native gate")
compatibility_steps = jobs.get("neovim-compatibility", {}).get("steps", [])
lower_bound_gate = [
    step for step in compatibility_steps if step.get("name") == "Run Lua tests on the supported lower bound"
]
if len(lower_bound_gate) != 1 or lower_bound_gate[0].get("run", "").strip() != (
    '"$RUNNER_TEMP/nvim-linux-x86_64/bin/nvim" --headless -u '
    "tests/lua/minimal_init.lua -l tests/lua/run.lua\n"
    '"$RUNNER_TEMP/nvim-linux-x86_64/bin/nvim" --headless -u '
    "tests/lua/minimal_init.lua -l tests/lua/resource_run.lua"
):
    fail("Neovim lower-bound CI must run both normal and resource Lua suites")
check_steps = jobs["check"].get("steps", [])
project_gate = [step for step in check_steps if step.get("name") == "Run project checks"]
if len(project_gate) != 1 or project_gate[0].get("run") != (
    "nix develop --no-update-lock-file --command pkf run --no-cache check"
):
    fail("Project checks must run the same uncached lock-preserving project gate")

native_job = jobs.get("native-platforms")
if not isinstance(native_job, dict):
    fail("CI must define a native-platforms matrix job")
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

expected_native_step_names = [
    "Check out repository",
    "Install Nix",
    "Verify native Nix system",
    "Validate Nix flake",
    "Run uncached project checks",
]
actual_native_step_names = [step.get("name") for step in native_job["steps"]]
if actual_native_step_names != expected_native_step_names:
    fail("native platform job must contain only the reviewed gate steps")
native_steps = {step["name"]: step for step in native_job["steps"]}
expected_commands = {
    "Verify native Nix system": (
        "actual_system=$(nix eval --raw --impure --expr builtins.currentSystem)\n"
        'test "$actual_system" = "${{ matrix.system }}"'
    ),
    "Validate Nix flake": "nix flake check --no-update-lock-file --print-build-logs",
    "Run uncached project checks": (
        "nix develop --no-update-lock-file --command pkf run --no-cache check"
    ),
}
for name, command in expected_commands.items():
    if native_steps[name].get("run", "").strip() != command:
        fail(f"native platform step {name!r} must run the reviewed command exactly")

pkfire = json.loads(
    subprocess.run(
        ["pkf", "info", "--json", "--all"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    ).stdout
)
tasks = {task["name"]: task for task in pkfire.get("tasks", [])}
resource_rust = tasks.get("test:resource:rust")
resource_lua = tasks.get("test:resource:lua")
resource = tasks.get("test:resource")
forward = tasks.get("test:forward")
test = tasks.get("test")
check = tasks.get("check")
if not all(
    isinstance(task, dict)
    for task in (resource_rust, resource_lua, resource, forward, test, check)
):
    fail("pkfire must evaluate resource, forward, and aggregate test/check tasks")
if resource.get("deps") != ["test:resource:rust", "test:resource:lua"]:
    fail("the resource aggregate must depend on both language contracts")
if "test:resource" not in check.get("deps", []):
    fail("the project check must execute the resource aggregate")
if forward.get("deps") != ["test:rust"] or forward.get("cmd") != (
    'UV_CACHE_DIR="${TMPDIR:-/tmp}/nvim-key-insights-uv-cache" '
    "KEY_INSIGHTS_BIN=target/debug/key-insights "
    "uv run --no-project --python-preference only-system python tests/forward_test_contract.py && "
    'UV_CACHE_DIR="${TMPDIR:-/tmp}/nvim-key-insights-uv-cache" '
    "KEY_INSIGHTS_BIN=target/debug/key-insights "
    "uv run --no-project --python-preference only-system python tests/local_forward_test_contract.py && "
    'UV_CACHE_DIR="${TMPDIR:-/tmp}/nvim-key-insights-uv-cache" '
    "KEY_INSIGHTS_BIN=target/debug/key-insights "
    "uv run --no-project --python-preference only-system python tests/local_performance_test_contract.py"
):
    fail("the forward-test task must run only after the Rust analyzer is built and cover all harness contracts")
if not {
    "scripts/forward_test.py",
    "tests/forward_test_contract.py",
    "scripts/local_forward_test.py",
    "tests/local_forward_test_contract.py",
    "scripts/local_performance_test.py",
    "tests/local_performance_test_contract.py",
    "docs/local-workflow.md",
    "docs/release-readiness.md",
} <= set(forward.get("inputs", [])):
    fail("the forward-test task is missing its harness contract inputs")
if test.get("deps") != ["test:lua", "test:e2e", "test:resource", "test:forward"]:
    fail("the all-tests entrypoint must execute Lua, E2E, resource, and forward suites")
if resource_rust.get("cmd") != (
    "cargo test -p key-insights --test deterministic_reporting --test jsonl_validation"
):
    fail("the Rust resource task must run only the dedicated resource suites")
if resource_lua.get("cmd") != (
    "nvim --headless -u tests/lua/minimal_init.lua -l tests/lua/resource_run.lua"
):
    fail("the Lua resource task must run the dedicated resource suite")
rust_inputs = set(resource_rust.get("inputs", []))
lua_inputs = set(resource_lua.get("inputs", []))
if not {"Cargo.toml", "Cargo.lock", "crates/**/*"} <= rust_inputs:
    fail("the Rust resource task is missing Cargo workspace inputs")
if any(pattern.startswith(("lua/", "plugin/", "tests/lua/")) for pattern in rust_inputs):
    fail("Lua changes must not invalidate the Rust resource task")
if not {
    "lua/**/*.lua",
    "plugin/**/*.lua",
    "tests/lua/callback_performance_spec.lua",
    "tests/lua/retention_spec.lua",
    "tests/lua/resource_run.lua",
} <= lua_inputs:
    fail("the Lua resource task is missing collector resource inputs")
if any(pattern.startswith(("crates/", "Cargo")) for pattern in lua_inputs):
    fail("Rust changes must not invalidate the Lua resource task")

print("CI native-platform and resource contract: ok")
