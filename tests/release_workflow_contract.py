#!/usr/bin/env python3

import re
from pathlib import Path

import yaml


ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github/workflows/release.yml"
PINNED_ACTIONS = {
    "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
    "DeterminateSystems/determinate-nix-action@61cbfe2efc2d4e7a8a6d56967c3c1058e846c858",
    "actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a",
}


def fail(message: str) -> None:
    raise SystemExit(message)


if not WORKFLOW.is_file():
    fail("missing tag release workflow")
text = WORKFLOW.read_text()
document = yaml.load(text, Loader=yaml.BaseLoader)
if not isinstance(document, dict):
    fail("release workflow must be a YAML mapping")
if document.get("permissions") != {"contents": "read"}:
    fail("release workflow default permissions must be contents: read")
trigger = document.get("on")
if trigger != {"push": {"tags": ["v*.*.*"]}}:
    fail("release workflow must run only for v*.*.* tag pushes")
jobs = document.get("jobs")
if not isinstance(jobs, dict) or set(jobs) != {"validate", "ruleset", "publish"}:
    fail("release workflow must contain only validate, ruleset, and publish jobs")
validate = jobs["validate"]
ruleset = jobs["ruleset"]
publish = jobs["publish"]
if validate.get("permissions") != {"contents": "read"}:
    fail("validation job must remain read-only")
if publish.get("permissions") != {"actions": "read", "contents": "write"}:
    fail("only publication may receive contents: write")
if publish.get("needs") != ["validate", "ruleset"] or ruleset.get("needs") != "validate":
    fail("publication must depend on validation and immutable-tag verification")
if ruleset.get("permissions") != {"contents": "read"} or ruleset.get("environment") != "release":
    fail("ruleset verification must use the protected release environment")
if len(ruleset.get("steps", [])) != 1 or "uses" in ruleset["steps"][0]:
    fail("the administration token must reach only one repository-independent shell step")
for required in (
    "RELEASE_RULESET_TOKEN",
    "immutable-release-tags",
    'has("bypass_actors")',
    'index("deletion")',
    'index("non_fast_forward")',
):
    if required not in str(ruleset["steps"][0]):
        fail(f"ruleset verification is missing {required!r}")

all_steps = validate.get("steps", []) + ruleset.get("steps", []) + publish.get("steps", [])
uses = [step["uses"] for step in all_steps if isinstance(step, dict) and "uses" in step]
if set(uses) != PINNED_ACTIONS or len(uses) != len(PINNED_ACTIONS):
    fail(f"release actions must match the immutable allowlist: {uses}")
for action in uses:
    if re.fullmatch(r"[^@]+@[0-9a-f]{40}", action) is None:
        fail(f"release action is not pinned to a commit: {action}")

checkout = next(step for step in validate["steps"] if step.get("uses", "").startswith("actions/checkout@"))
if checkout.get("with", {}).get("persist-credentials") != "false":
    fail("release checkout credentials must not persist")
if checkout.get("with", {}).get("fetch-depth") != "0":
    fail("release validation must fetch history for the main ancestry check")
if len(publish["steps"]) != 1 or "uses" in publish["steps"][0]:
    fail("write-capable publication must be one repository-independent shell step")

validation_script = "\n".join(step.get("run", "") for step in validate["steps"])
for required in (
    "release.py check --tag",
    'git merge-base --is-ancestor "$GITHUB_SHA" origin/main',
    "--nix-system aarch64-darwin",
    "--nix-system aarch64-linux",
    "--nix-system x86_64-linux",
    "nix flake check --no-update-lock-file",
    "pkf run --no-cache check",
    "release.py build-artifacts",
    "release.py release-notes",
    "sha256sum --check SHA256SUMS",
):
    if required not in validation_script:
        fail(f"validation job is missing {required!r}")
if re.search(r"nix (?:develop|flake check)(?![^\n]*--no-update-lock-file)", validation_script):
    fail("release validation must reject implicit flake.lock updates")

upload = next(step for step in validate["steps"] if step.get("uses", "").startswith("actions/upload-artifact@"))
upload_with = upload.get("with", {})
if upload_with.get("if-no-files-found") != "error" or upload_with.get("overwrite") != "false":
    fail("release handoff must reject missing files and overwrites")
if not all(
    value in upload_with.get("name", "")
    for value in ("${{ github.sha }}", "${{ github.run_attempt }}")
):
    fail("release handoff name must bind the validated commit and run attempt")
if validate.get("outputs") != {
    "handoff_digest": "${{ steps.upload.outputs.artifact-digest }}",
    "handoff_id": "${{ steps.upload.outputs.artifact-id }}",
}:
    fail("publication must receive the exact handoff ID and digest")

publication_script = "\n".join(step.get("run", "") for step in publish["steps"])
for required in (
    "sha256sum --check SHA256SUMS",
    'commits/$RELEASE_TAG',
    "actions/artifacts/$EXPECTED_HANDOFF_ID",
    "GITHUB_RUN_ID",
    "EXPECTED_HANDOFF_DIGEST",
    "EXPECTED_HANDOFF_ID",
    "gh release view",
    "gh release create",
    "--verify-tag",
    "--notes-file",
):
    if required not in publication_script:
        fail(f"publication job is missing {required!r}")
if "--clobber" in publication_script:
    fail("release publication must never replace an existing asset")
if any("github.token" in str(step) for step in validate["steps"]):
    fail("the write-capable GitHub token must not reach validation")
if any("RELEASE_RULESET_TOKEN" in str(step) for step in validate["steps"] + publish["steps"]):
    fail("the administration token must remain isolated in ruleset verification")
token_steps = [step for step in publish["steps"] if "github.token" in str(step)]
if len(token_steps) != 1 or "gh release create" not in token_steps[0].get("run", ""):
    fail("only the final publication step may receive the GitHub token")

print("Release workflow security contract: ok")
